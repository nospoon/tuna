//! WASAPI capture. The cpal data callback does exactly two things — filter and
//! push into a lock-free ring — and touches no lock, no allocation (after the
//! scratch buffers have grown once) and no UI. A separate worker thread owns the
//! `Analyzer` and publishes the newest `Reading`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{
    Data, Device, FromSample, InputCallbackInfo, SampleFormat, SizedSample, Stream,
    SupportedStreamConfig,
};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::audio::device;
use crate::dsp::biquad::HighPassBank;
use crate::dsp::frame::{Analyzer, HOP};
use crate::dsp::track::Reading;

/// ~1.4 s at 48 kHz. The worker trims the backlog, so a slow UI never adds
/// audible latency and never blocks the callback.
pub const RING_CAPACITY: usize = 65_536;
/// Backlog above which the worker drops samples to stay real-time.
const BACKLOG_LIMIT_HOPS: usize = 4;
const DIAGNOSTIC_CAP: usize = 24;
const REPAINT: Duration = Duration::from_millis(16);

#[derive(Clone, Debug, PartialEq, Default)]
pub enum AudioStatus {
    #[default]
    Idle,
    Running,
    Failed(String),
}

#[derive(Default)]
pub struct Shared {
    pub latest: Mutex<Option<Reading>>,
    pub status: Mutex<AudioStatus>,
    pub diagnostics: Mutex<VecDeque<String>>,
    pub format_label: Mutex<String>,
    pub device_label: Mutex<String>,
    /// `(gate_db, floor_db, open)` exactly as the analyzer last saw it.
    pub gate: Mutex<(f32, f32, bool)>,
    /// Frames handed to the callback since the stream started: the UI's
    /// "is data still arriving" watchdog.
    pub frames: Arc<AtomicU64>,
}

impl Shared {
    pub fn diagnose(&self, msg: impl Into<String>) {
        let mut d = self.diagnostics.lock().unwrap_or_else(|e| e.into_inner());
        let msg = msg.into();
        if d.back().map(|last| last == &msg).unwrap_or(false) {
            return;
        }
        d.push_back(msg);
        while d.len() > DIAGNOSTIC_CAP {
            d.pop_front();
        }
    }

    pub fn diagnostics(&self) -> Vec<String> {
        self.diagnostics
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .rev()
            .cloned()
            .collect()
    }

    pub fn status(&self) -> AudioStatus {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn set_status(&self, s: AudioStatus) {
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = s;
    }

    pub fn latest(&self) -> Option<Reading> {
        *self.latest.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn labels(&self) -> (String, String) {
        (
            self.device_label
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            self.format_label
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        )
    }
}

#[derive(Clone, Debug)]
pub struct StartParams {
    pub channel: usize,
    pub a4: f32,
    pub tuning: Vec<u8>,
    pub pinned: Option<usize>,
    pub smoothing_ms: f32,
    /// Requested repaint after publishing a reading; None for headless runs.
    pub ctx: Option<egui::Context>,
}

struct Sink {
    prod: Producer<f32>,
    hp: HighPassBank,
    sel: usize,
    channels: usize,
    scratch: Vec<f32>,
    frames: Arc<AtomicU64>,
}

impl Sink {
    /// Real-time path: filter every channel (so switching the monitored input
    /// mid-stream never steps the filter state), publish the selected one.
    fn push<T>(&mut self, interleaved: &[T])
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        let ch = self.channels;
        let frames = interleaved.len() / ch;
        if frames == 0 {
            return;
        }
        if self.scratch.len() < frames {
            self.scratch.resize(frames, 0.0);
        }
        for f in 0..frames {
            let base = f * ch;
            let mut v = 0.0f32;
            for c in 0..ch {
                let y = self
                    .hp
                    .process(c, FromSample::from_sample_(interleaved[base + c]));
                if c == self.sel {
                    v = y;
                }
            }
            self.scratch[f] = v;
        }
        // A full ring is not an error: the worker trims the backlog, so what
        // falls off the end here is audio the analyser already chose to skip.
        // Its evidence is the "dropped N samples to keep up" log line in the
        // worker, so there is no second counter to keep in sync.
        let _ = self.prod.push_partial_slice(&self.scratch[..frames]);
        self.frames.fetch_add(frames as u64, Ordering::Relaxed);
    }
}

pub struct Capture {
    stream: Option<Stream>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Capture {
    pub fn start(
        shared: &Arc<Shared>,
        dev: &Device,
        cfg: &SupportedStreamConfig,
        p: &StartParams,
    ) -> Result<Capture, String> {
        let sc = device::stream_config(cfg);
        let channels = sc.channels as usize;
        if channels == 0 {
            return Err("device reports zero input channels".into());
        }
        let sel = p.channel.min(channels - 1);
        let fs = sc.sample_rate as f32;
        // the worker thread owns its copy: the callback thread never sees config
        let p = p.clone();

        let (prod, cons) = RingBuffer::<f32>::new(RING_CAPACITY);
        let sink = Sink {
            prod,
            hp: HighPassBank::new(fs, channels),
            sel,
            channels,
            scratch: Vec::new(),
            frames: Arc::clone(&shared.frames),
        };

        {
            let err_shared = Arc::clone(shared);
            let err = move |e: cpal::Error| {
                err_shared.set_status(AudioStatus::Failed(e.to_string()));
                err_shared.diagnose(format!("input stream error: {e}"));
            };
            let stream = match cfg.sample_format() {
                SampleFormat::F32 => open_raw::<f32>(dev, sc, cfg.sample_format(), sink, err)?,
                SampleFormat::I16 => open_raw::<i16>(dev, sc, cfg.sample_format(), sink, err)?,
                SampleFormat::I24 => {
                    open_raw::<cpal::I24>(dev, sc, cfg.sample_format(), sink, err)?
                }
                SampleFormat::I32 => open_raw::<i32>(dev, sc, cfg.sample_format(), sink, err)?,
                SampleFormat::U8 => open_raw::<u8>(dev, sc, cfg.sample_format(), sink, err)?,
                SampleFormat::F64 => open_raw::<f64>(dev, sc, cfg.sample_format(), sink, err)?,
                other => return Err(format!("unsupported sample format {other:?}")),
            };
            // `stream` is returned below; sink was moved into the callback.
            stream
                .play()
                .map_err(|e| format!("could not start input stream: {e}"))?;

            *shared
                .device_label
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = crate::audio::device::describe_name(dev);
            *shared
                .format_label
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = format!(
                "{} Hz · {} ch · {}",
                sc.sample_rate,
                sc.channels,
                sample_format_name(cfg.sample_format())
            );

            let stop = Arc::new(AtomicBool::new(false));
            let worker = {
                let shared = Arc::clone(shared);
                let stop = Arc::clone(&stop);
                let ctx = p.ctx.clone();
                std::thread::Builder::new()
                    .name("tuna-analyser".into())
                    .spawn(move || worker(shared, cons, stop, fs, &p, ctx))
                    .map_err(|e| format!("could not start analyser thread: {e}"))?
            };
            shared.set_status(AudioStatus::Running);
            Ok(Capture {
                stream: Some(stream),
                stop,
                worker: Some(worker),
            })
        }
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(s) = self.stream.take() {
            let _ = s.pause();
            drop(s);
        }
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// cpal's own spelling, lower case: the on-screen format label reads
/// `48 000 Hz · 2 ch · f32`.
pub(crate) fn sample_format_name(f: SampleFormat) -> &'static str {
    match f {
        SampleFormat::F32 => "f32",
        SampleFormat::F64 => "f64",
        SampleFormat::I16 => "i16",
        SampleFormat::I32 => "i32",
        SampleFormat::I24 => "i24",
        SampleFormat::U8 => "u8",
        _ => "unknown",
    }
}

fn open_raw<T>(
    dev: &Device,
    sc: cpal::StreamConfig,
    sf: SampleFormat,
    mut sink: Sink,
    err: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    dev.build_input_stream_raw(
        sc,
        sf,
        move |d: &Data, _: &InputCallbackInfo| {
            if let Some(s) = d.as_slice::<T>() {
                sink.push(s);
            }
        },
        err,
        None,
    )
    .map_err(|e| format!("could not open input stream: {e}"))
}

fn worker(
    shared: Arc<Shared>,
    mut cons: Consumer<f32>,
    stop: Arc<AtomicBool>,
    fs: f32,
    p: &StartParams,
    ctx: Option<egui::Context>,
) {
    let mut an = Analyzer::new(fs);
    an.set_a4(p.a4);
    an.set_tuning(&p.tuning);
    an.set_pinned(p.pinned);
    an.set_smoothing(p.smoothing_ms);

    let mut hop = vec![0.0f32; HOP];
    let mut trash = vec![0.0f32; 1024];
    let mut last_repaint = Instant::now();
    let mut last_trim = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        let avail = cons.slots();
        if avail > BACKLOG_LIMIT_HOPS * HOP {
            let mut left = avail - HOP;
            while left > 0 {
                let k = left.min(trash.len());
                let (got, _) = cons.pop_partial_slice(&mut trash[..k]);
                let n = got.len();
                if n == 0 {
                    break;
                }
                left -= n;
            }
            if last_trim.elapsed() > Duration::from_secs(2) {
                last_trim = Instant::now();
                shared.diagnose(format!(
                    "dropped {} samples to keep up (consumer fell behind)",
                    avail - HOP
                ));
            }
        }
        if cons.slots() < HOP {
            std::thread::sleep(Duration::from_millis(3));
            continue;
        }
        let (got, _) = cons.pop_partial_slice(&mut hop);
        if got.len() != HOP {
            continue;
        }
        if let Some(r) = an.process_hop(&hop) {
            *shared.latest.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
            *shared.gate.lock().unwrap_or_else(|e| e.into_inner()) = an.gate_state();
            if let Some(ctx) = &ctx {
                if last_repaint.elapsed() >= REPAINT {
                    last_repaint = Instant::now();
                    ctx.request_repaint();
                }
            }
        }
    }
    shared.set_status(AudioStatus::Idle);
}
