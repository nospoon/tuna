//! Application state and the capture lifecycle: which device is open, what the
//! analyzer last decided, when the config file gets written.
//!
//! The analyzer is owned by the audio worker, so anything that changes its
//! parameters (device, channel, A4, tuning, pinned string, smoothing, gate)
//! rebuilds the stream instead of sharing a mutex-protected analyzer with the
//! UI thread. Rebuilds are debounced so dragging A4 does not thrash WASAPI.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::audio::capture::{AudioStatus, Capture, Shared, StartParams};
use crate::audio::device::{self, Inventory};
use crate::cli::Offline;
use crate::dsp::frame::N_MAX;
use crate::dsp::track::{Reading, TrackState};
use crate::model::config::AppConfig;
use crate::model::presets::{preset_by_id, tuning_for};
use crate::ui::panels::{self, Changed, Controls};
use crate::ui::theme::SURFACE;
use crate::ui::tuner_view::{self, View};
use crate::ui::TargetMode;

/// Leaving the dead band needs this much more error than entering it, so the
/// needle sitting on the boundary cannot strobe the in-tune colour.
pub const IN_TUNE_RELEASE_CENTS: f32 = 3.0;
const REPAINT_LIVE: Duration = Duration::from_millis(16);
const REPAINT_IDLE: Duration = Duration::from_millis(250);
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);
const RETRY_DELAY: Duration = Duration::from_millis(250);
const ENUM_INTERVAL: Duration = Duration::from_secs(2);
const STALL_TIMEOUT: Duration = Duration::from_millis(600);
const CLIP_HOLD: Duration = Duration::from_millis(1000);
const RESTART_DEBOUNCE: Duration = Duration::from_millis(250);
const PEAK_DECAY_DB_PER_S: f32 = 12.0;
const FLOOR_DB: f32 = -120.0;

/// Everything the running stream was built from; a change means rebuild.
#[derive(Clone, Debug, PartialEq)]
struct Sig {
    device_id: String,
    channel: usize,
    a4: f32,
    tuning: Vec<u8>,
    pinned: Option<usize>,
    smoothing_ms: f32,
    auto_gate: bool,
    gate_floor_db: f32,
}

pub struct TunaApp {
    pub cfg: AppConfig,
    pub shared: Arc<Shared>,
    pub inventory: Inventory,
    pub capture: Option<Capture>,
    /// Offline readings from `--wav`: shown in the same view, capture off.
    pub offline: Option<Offline>,
    pub target: TargetMode,
    in_tune: bool,
    in_tune_run: usize,
    dirty: bool,
    last_save: Instant,
    sig: Sig,
    sig_since: Option<Instant>,
    last_status: AudioStatus,
    retry_at: Option<Instant>,
    next_enum: Instant,
    last_frames: u64,
    frames_seen_at: Instant,
    clip_until: Option<Instant>,
    peak_db: f32,
    last_tick: Option<Instant>,
    /// Analysis window length, quoted in the `Locked 171 ms` chip.
    frame_ms: u32,
    /// Dev-only paint counter behind `TUNA_SCREENSHOT_TO`.
    #[cfg(feature = "screenshot")]
    paints: u32,
}

impl TunaApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        cfg: AppConfig,
        cfg_note: Option<String>,
        offline: Option<Offline>,
    ) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());

        let shared = Arc::new(Shared::default());
        let mut app = TunaApp {
            shared,
            inventory: Inventory::default(),
            capture: None,
            cfg,
            offline,
            target: TargetMode::Auto,
            in_tune: false,
            in_tune_run: 0,
            dirty: false,
            last_save: Instant::now(),
            sig: Sig {
                device_id: String::new(),
                channel: 0,
                a4: 0.0,
                tuning: Vec::new(),
                pinned: None,
                smoothing_ms: -1.0,
                auto_gate: false,
                gate_floor_db: 0.0,
            },
            sig_since: None,
            last_status: AudioStatus::Idle,
            retry_at: None,
            next_enum: Instant::now(),
            last_frames: 0,
            frames_seen_at: Instant::now(),
            clip_until: None,
            peak_db: FLOOR_DB,
            last_tick: None,
            frame_ms: frame_ms(48_000.0),
            #[cfg(feature = "screenshot")]
            paints: 0,
        };
        if let Some(note) = cfg_note {
            app.shared.diagnose(note);
        }
        app.shared.diagnose(format!(
            "config: preset {} · spelling {} · A4 {:.0} Hz",
            app.cfg.preset,
            match app.cfg.spelling {
                crate::model::notes::Spelling::Sharp => "sharp",
                crate::model::notes::Spelling::Flat => "flat",
            },
            app.cfg.a4_hz
        ));
        app.target = app
            .cfg
            .pinned_string
            .map(TargetMode::Pinned)
            .unwrap_or_default();
        match &app.offline {
            Some(o) => {
                app.frame_ms = frame_ms(o.fs as f32);
                app.shared.diagnose(format!(
                    "offline file: {} Hz {} ch {}",
                    o.fs, o.channels, o.format
                ));
            }
            None => {
                app.refresh_devices();
                app.start_capture(&cc.egui_ctx);
            }
        }
        app
    }

    // ------------------------------------------------------- state helpers --

    fn reading(&self) -> Option<Reading> {
        match &self.offline {
            Some(o) => o.readings.last().copied(),
            None => self.shared.latest(),
        }
    }

    fn gate(&self) -> (f32, f32, bool) {
        match &self.offline {
            Some(o) => o.gate,
            None => *self.shared.gate.lock().unwrap_or_else(|e| e.into_inner()),
        }
    }

    fn signature(&self) -> Sig {
        let tuning = tuning_for(&self.cfg.preset, &self.cfg.custom_tuning);
        let pinned = self.target.index().filter(|i| *i < tuning.len());
        Sig {
            device_id: self.cfg.device_id.clone(),
            channel: self.cfg.input_channel,
            a4: self.cfg.a4_hz,
            tuning,
            pinned,
            smoothing_ms: self.cfg.smoothing.tau_ms(),
            auto_gate: self.cfg.auto_gate,
            gate_floor_db: self.cfg.gate_floor_db,
        }
    }

    fn refresh_devices(&mut self) {
        let mut diag = Vec::new();
        self.inventory = device::inventory(&mut diag);
        for d in diag {
            self.shared.diagnose(d);
        }
        if self.cfg.device_id.is_empty() {
            self.cfg.device_id = self.inventory.default_id.clone().unwrap_or_default();
        }
    }

    /// Stop, re-open, and hand the new stream the current settings.
    fn start_capture(&mut self, ctx: &egui::Context) {
        // Dropping the old capture signals its worker and joins it.
        self.capture = None;
        if self.offline.is_some() {
            return;
        }
        let p = StartParams {
            channel: self.cfg.input_channel,
            a4: self.cfg.a4_hz,
            tuning: tuning_for(&self.cfg.preset, &self.cfg.custom_tuning),
            pinned: self.target.index(),
            smoothing_ms: self.cfg.smoothing.tau_ms(),
            auto_gate: self.cfg.auto_gate,
            gate_floor_db: self.cfg.gate_floor_db,
            ctx: Some(ctx.clone()),
        };
        match self.open_stream(&p) {
            Ok((label, rate, channels, format)) => {
                self.sig = self.signature();
                self.last_frames = self.shared.frames.load(Ordering::Relaxed);
                self.frames_seen_at = Instant::now();
                self.shared
                    .diagnose(format!("opened {label} {rate} Hz {channels} ch {format}"));
            }
            Err(e) => {
                self.shared.diagnose(format!("could not open stream: {e}"));
                self.shared.set_status(AudioStatus::Failed(e));
                self.retry_at = Some(Instant::now());
            }
        }
    }

    /// Opens the device and starts the worker. The tuple is what the log line
    /// needs to name the format WASAPI actually granted.
    fn open_stream(&mut self, p: &StartParams) -> Result<(String, u32, u16, String), String> {
        let (dev, scfg, label) = device::open(&self.cfg.device_id)?;
        self.frame_ms = frame_ms(scfg.sample_rate() as f32);
        let info = (
            scfg.sample_rate(),
            scfg.channels(),
            format!("{:?}", scfg.sample_format()),
        );
        match Capture::start(&self.shared, &dev, &scfg, p) {
            Ok(c) => {
                self.capture = Some(c);
                Ok((label, info.0, info.1, info.2))
            }
            Err(e) => Err(format!("{label}: {e}")),
        }
    }

    fn watch_devices(&mut self) {
        let before: Vec<String> = self
            .inventory
            .devices
            .iter()
            .map(|d| d.id.clone())
            .collect();
        let gone_name = self
            .inventory
            .devices
            .iter()
            .find(|d| d.id == self.cfg.device_id)
            .map(|d| d.name.clone());
        self.refresh_devices();
        let after: Vec<String> = self
            .inventory
            .devices
            .iter()
            .map(|d| d.id.clone())
            .collect();

        if self.cfg.device_id.is_empty() {
            return;
        }
        let here = after.iter().any(|id| id == &self.cfg.device_id);
        if before.iter().any(|id| id == &self.cfg.device_id) && !here {
            // Deliberately does not fall back to another interface: playing a
            // tuning off the wrong converter is worse than an honest error.
            self.shared.diagnose(format!(
                "input device {} disappeared",
                gone_name.unwrap_or_else(|| self.cfg.device_id.clone())
            ));
            self.capture = None;
            self.shared
                .set_status(AudioStatus::Failed("input device disappeared".into()));
            self.retry_at = Some(Instant::now());
        }
        for id in after.iter().filter(|id| !before.contains(id)) {
            if self.capture.is_some() {
                let name = self
                    .inventory
                    .devices
                    .iter()
                    .find(|d| &d.id == id)
                    .map(|d| d.name.clone())
                    .unwrap_or_default();
                self.shared
                    .diagnose(format!("{name} came back; staying on the current device"));
            }
        }
    }

    /// Status, watchdog, device re-enumeration, peak hold, clip latch,
    /// in-tune hysteresis and the repaint policy.
    fn poll(&mut self, ctx: &egui::Context, r: Option<Reading>) {
        let dt = self
            .last_tick
            .map(|t| t.elapsed().as_secs_f32())
            .unwrap_or(0.0)
            .clamp(0.0, 0.25);
        self.last_tick = Some(Instant::now());

        let status = self.shared.status();
        if status != self.last_status {
            if let AudioStatus::Failed(e) = &status {
                self.shared.diagnose(format!("capture failed: {e}"));
                self.retry_at = Some(Instant::now());
            }
            self.shared.diagnose(format!("capture status: {status:?}"));
            self.last_status = status.clone();
        }

        if self.offline.is_none() {
            // A stream that stops delivering samples is dead even when cpal
            // never reports an error.
            let frames = self.shared.frames.load(Ordering::Relaxed);
            if frames != self.last_frames {
                self.last_frames = frames;
                self.frames_seen_at = Instant::now();
            } else if self.capture.is_some()
                && status == AudioStatus::Running
                && self.frames_seen_at.elapsed() >= STALL_TIMEOUT
            {
                self.shared
                    .diagnose("no samples for 600 ms, but no error: reopening");
                self.capture = None;
                self.shared.set_status(AudioStatus::Failed(
                    "input stopped delivering samples".into(),
                ));
            }

            if self.next_enum.elapsed() >= ENUM_INTERVAL {
                self.next_enum = Instant::now();
                self.watch_devices();
            }
            if self.retry_at.map(|t| t.elapsed() >= RETRY_DELAY) == Some(true)
                || (self.retry_at.is_some()
                    && self.capture.is_none()
                    && self.shared.status() == AudioStatus::Idle)
            {
                self.retry_at = None;
                self.refresh_devices();
                self.start_capture(ctx);
            }
        }

        // peak hold
        let now_peak = r.map(|r| r.peak_db).unwrap_or(FLOOR_DB);
        self.peak_db = now_peak.max(self.peak_db - PEAK_DECAY_DB_PER_S * dt);
        if r.map(|r| r.clipped).unwrap_or(false) {
            self.clip_until = Some(Instant::now());
        }

        // in-tune hysteresis: two frames inside the band to light up, a clear
        // excursion to let go
        if let Some(r) = r {
            let off = r.cents.abs();
            let tol = self.cfg.tolerance_cents;
            if r.state == TrackState::Locked && off <= tol {
                self.in_tune_run += 1;
            } else {
                self.in_tune_run = 0;
            }
            if self.in_tune {
                if r.state != TrackState::Locked || off > tol + IN_TUNE_RELEASE_CENTS {
                    self.in_tune = false;
                }
            } else if self.in_tune_run >= 2 {
                self.in_tune = true;
            }
        }

        let live = self.offline.is_none()
            && self.capture.is_some()
            && r.map(|r| r.state != TrackState::NoSignal && r.state != TrackState::Idle)
                .unwrap_or(false);
        ctx.request_repaint_after(if live { REPAINT_LIVE } else { REPAINT_IDLE });
    }

    fn save_if_dirty(&mut self) {
        if self.dirty && self.last_save.elapsed() >= SAVE_DEBOUNCE {
            match self.cfg.save() {
                Ok(()) => {
                    self.dirty = false;
                    self.last_save = Instant::now();
                }
                Err(e) => {
                    self.shared.diagnose(format!("could not save config: {e}"));
                    self.dirty = false;
                    self.last_save = Instant::now();
                }
            }
        }
    }

    // --------------------------------------------------------------- paint --

    fn status_caption(&self) -> Option<&'static str> {
        if self.offline.is_some() {
            return Some("offline file");
        }
        match self.shared.status() {
            AudioStatus::Running => None,
            AudioStatus::Idle => Some("capture stopped"),
            AudioStatus::Failed(_) => Some("Disconnected"),
        }
    }
}

impl eframe::App for TunaApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        #[cfg(feature = "screenshot")]
        self.screenshot_tick(&ctx);
        let reading = self.reading();
        self.poll(&ctx, reading);

        let status = self.shared.status();
        let (_, format_label) = self.shared.labels();
        let log = self.shared.diagnostics();
        let gate = self.gate();
        let clipped = self
            .clip_until
            .map(|t| t.elapsed() < CLIP_HOLD)
            .unwrap_or(false);
        let status_line = self.status_caption();

        let mut changed = Changed::default();
        let frame = egui::Frame::new()
            .fill(SURFACE)
            .corner_radius(6.0)
            .inner_margin(12.0);
        let clicked = {
            let mut controls = Controls {
                cfg: &mut self.cfg,
                inventory: &self.inventory,
                status: &status,
                format: if format_label.is_empty() {
                    "no stream"
                } else {
                    &format_label
                },
                log: &log,
                reading,
                gate,
                peak_db: self.peak_db,
                clipped,
                target: self.target,
            };
            egui::Panel::top("top_bar").frame(frame).show(ui, |ui| {
                changed.merge(panels::top_bar(ui, &mut controls));
            });
            egui::Panel::left("io_panel")
                .resizable(false)
                .exact_size(250.0)
                .frame(frame)
                .show(ui, |ui| {
                    changed.merge(panels::side_panel(ui, &mut controls));
                });
            // The edits are read back through `controls`: the borrow of `cfg`
            // ends with this block, so the central panel shows exactly what the
            // top bar now says and the bookkeeping below may write again.
            let tuning = tuning_for(&controls.cfg.preset, &controls.cfg.custom_tuning);
            let view = View {
                reading,
                tuning: &tuning,
                spelling: controls.cfg.spelling,
                a4: controls.cfg.a4_hz,
                target: controls.target,
                tolerance: controls.cfg.tolerance_cents,
                in_tune: self.in_tune,
                frame_ms: self.frame_ms,
                status: status_line,
            };
            egui::CentralPanel::default()
                .frame(frame)
                .show(ui, |ui| tuner_view::show(ui, &view))
                .inner
        };
        if let Some(t) = clicked {
            self.target = t;
            self.cfg.pinned_string = self.target.index();
            changed.capture = true;
            changed.config = true;
        }
        if changed.preset {
            // The preset suggests a spelling; the toggle still wins afterwards.
            if let Some(p) = preset_by_id(&self.cfg.preset) {
                self.cfg.spelling = p.spelling;
            }
            self.target = TargetMode::Auto;
            self.cfg.pinned_string = None;
        }
        self.dirty |= changed.config;
        self.cfg.sanitize();
        self.sig_since = None;
        if changed.capture {
            self.restart_debounced(&ctx);
        }
        self.save_if_dirty();

        // An offline `--wav` run must not rewrite the live setup: the flags that
        // shaped this view are not something the user chose in the UI.
        if self.offline.is_none() && ctx.input(|i| i.viewport().close_requested()) {
            self.cfg.save().ok();
        }
    }
}

/// Dev capture: `cargo build --release --features screenshot` plus
/// `TUNA_SCREENSHOT_TO=shots/x.png tuna --wav …` writes a PNG of a settled
/// frame and closes the window. Not eframe's `EFRAME_SCREENSHOT_TO` hook: eframe
/// 0.37 panics on that variable unless it was built with its own `__screenshot`
/// feature, and pass 12 leaves the font atlas finished growing before we read it.
#[cfg(feature = "screenshot")]
impl TunaApp {
    const SHOT_FRAME: u32 = 12;

    fn screenshot_tick(&mut self, ctx: &egui::Context) {
        self.paints += 1;
        let Ok(path) = std::env::var("TUNA_SCREENSHOT_TO") else {
            return;
        };
        if self.paints == Self::SHOT_FRAME {
            let path = std::path::PathBuf::from(path);
            ctx.request_screenshot(move |img| {
                if let Err(e) = write_png(&path, &img) {
                    eprintln!("tuna: screenshot {}: {e}", path.display());
                } else {
                    println!("screenshot written to {}", path.display());
                }
            });
        } else if self.paints > Self::SHOT_FRAME + 3 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

#[cfg(feature = "screenshot")]
fn write_png(path: &std::path::Path, img: &egui::ColorImage) -> Result<(), String> {
    let [w, h] = img.size;
    let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(file, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .map_err(|e| e.to_string())?
        .write_image_data(&rgba)
        .map_err(|e| e.to_string())
}

/// Stream rebuilds are coalesced: a DragValue drag changes A4 many times a
/// second and each change would otherwise tear down WASAPI.
impl TunaApp {
    fn restart_debounced(&mut self, ctx: &egui::Context) {
        let sig = self.signature();
        if sig == self.sig {
            self.sig_since = None;
            return;
        }
        match self.sig_since {
            None => self.sig_since = Some(Instant::now()),
            Some(t) if t.elapsed() >= RESTART_DEBOUNCE => {
                self.sig_since = None;
                self.start_capture(ctx);
            }
            Some(_) => {}
        }
    }
}

fn frame_ms(fs: f32) -> u32 {
    (N_MAX as f32 * 1000.0 / fs.max(1.0)).round() as u32
}
