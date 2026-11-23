//! `Analyzer`: the one hop-in / reading-out chain shared by live capture, the
//! offline `--wav` path and the tests. Everything is pre-allocated: the only
//! work per hop is a `copy_within` shift, level detection, the grid correlation
//! and the tracker.

use crate::dsp::harmonics::{goertzel_mags, hann_into, hann_table, NH};
use crate::dsp::pitch::Pitch;
use crate::dsp::track::{Hint, Reading, TrackInput, TrackState, Tracker};
#[cfg(test)]
use crate::model::notes::PITCH_FLOOR_HZ;

/// Acquisition frame: 171 ms at 48 kHz, i.e. 5.5 periods of the search floor.
pub const N_MAX: usize = 8192;
/// Tracking frame once locked above `F_TRACK_SHORT_HZ`: halves display latency.
pub const N_TRACK: usize = 4096;
pub const HOP: usize = 512;

pub const YIN_THRESHOLD: f32 = 0.10;
pub const CLARITY_ACCEPT: f32 = 0.45;
pub const LOCK_CONFIRM: usize = 3;
pub const NOTE_CHANGE_CONFIRM: usize = 6;
pub const OUTLIER_CENTS: f32 = 120.0;
/// Spread the three acquisition medians must agree within, in cents.
pub const LOCK_CONFIRM_CENTS: f32 = 35.0;

pub const GATE_AUTO_MARGIN_DB: f32 = 10.0;
pub const GATE_HYSTERESIS_DB: f32 = 4.0;
/// Absolute floor of the gate: the auto gate never opens below this.
pub const GATE_FLOOR_DB: f32 = -60.0;
/// Noise-floor upward creep, dB per hop (~4.7 dB/s).
pub const FLOOR_RISE_DB_PER_HOP: f32 = 0.05;
pub const RMS_SMOOTH_S: f32 = 0.050;
pub const CLIP_HOLD_MS: u64 = 1000;

/// Above this frequency the short tracking frame is used (its 4096-sample
/// window still holds >2 periods of 55 Hz).
pub const F_TRACK_SHORT_HZ: f32 = 55.0;
/// Shortest correlation length a frame must be able to offer.
#[cfg(test)]
const W_MIN: usize = 1024;
pub const WEAK_FUND_RATIO: f32 = 0.25;
const DB_FLOOR: f32 = -160.0;

#[inline]
fn dbfs(x: f32) -> f32 {
    if x.is_finite() && x > 0.0 {
        (20.0f32 * x.log10()).max(DB_FLOOR)
    } else {
        DB_FLOOR
    }
}

pub struct Analyzer {
    fs: f32,
    hop_dur_ms: f32,

    hist: Vec<f32>,
    filled: usize,
    hops: u64,

    rms_db: f32,
    peak_db: f32,
    rms_db_smooth: f32,
    floor_db: f32,
    gate_db: f32,
    gate_open: bool,
    clip_hops: u64,
    clip_hold_len: u64,

    a4: f32,
    tuning: Vec<u8>,
    pinned: Option<usize>,
    tau_smooth_ms: f32,

    pitch: Pitch,
    hann: Vec<f32>,
    scratch: Vec<f32>,
    amps: [f32; NH],
    tracker: Tracker,
    f_display: f32,
    last: Option<Reading>,
}

impl Analyzer {
    pub fn new(fs: f32) -> Self {
        let hop_dur_ms = HOP as f32 * 1000.0 / fs;
        let mut a = Analyzer {
            fs,
            hop_dur_ms,
            hist: vec![0.0; N_MAX],
            filled: 0,
            hops: 0,
            rms_db: DB_FLOOR,
            peak_db: DB_FLOOR,
            rms_db_smooth: DB_FLOOR,
            floor_db: GATE_FLOOR_DB,
            gate_db: GATE_FLOOR_DB,
            gate_open: false,
            clip_hops: 0,
            clip_hold_len: (CLIP_HOLD_MS as f32 / hop_dur_ms).ceil() as u64,
            a4: crate::model::notes::A4_DEFAULT,
            tuning: Vec::new(),
            pinned: None,
            tau_smooth_ms: crate::model::config::Smoothing::Normal.tau_ms(),
            pitch: Pitch::new(fs),
            hann: hann_table(N_MAX),
            scratch: vec![0.0; N_MAX],
            amps: [0.0; NH],
            tracker: Tracker::new(),
            f_display: 0.0,
            last: None,
        };
        a.gate_db = a.compute_gate();
        a
    }

    pub fn fs(&self) -> f32 {
        self.fs
    }
    pub fn hop_dur_ms(&self) -> f32 {
        self.hop_dur_ms
    }
    pub fn reading(&self) -> Option<Reading> {
        self.last
    }

    pub fn set_a4(&mut self, a4: f32) {
        self.a4 = a4;
    }
    pub fn set_tuning(&mut self, tuning: &[u8]) {
        self.tuning = tuning.to_vec();
        if let Some(p) = self.pinned {
            if p >= self.tuning.len() {
                self.pinned = None;
            }
        }
        self.tracker.retarget();
    }
    pub fn set_pinned(&mut self, pinned: Option<usize>) {
        let pinned = match pinned {
            Some(p) if p < self.tuning.len() => Some(p),
            _ => None,
        };
        if pinned != self.pinned {
            self.pinned = pinned;
            self.tracker.retarget();
        }
    }
    pub fn set_smoothing(&mut self, tau_ms: f32) {
        self.tau_smooth_ms = tau_ms.max(10.0);
    }
    pub fn gate_state(&self) -> (f32, f32, bool) {
        (self.gate_db, self.floor_db, self.gate_open)
    }

    /// Full reset: new device / sample rate.
    pub fn reset(&mut self, fs: f32) {
        if fs != self.fs {
            self.fs = fs;
            self.hop_dur_ms = HOP as f32 * 1000.0 / fs;
            self.clip_hold_len = (CLIP_HOLD_MS as f32 / self.hop_dur_ms).ceil() as u64;
            self.pitch = Pitch::new(fs);
            self.hann = hann_table(N_MAX);
        }
        self.filled = 0;
        self.hops = 0;
        self.hist.iter_mut().for_each(|v| *v = 0.0);
        self.rms_db = DB_FLOOR;
        self.peak_db = DB_FLOOR;
        self.rms_db_smooth = DB_FLOOR;
        self.floor_db = GATE_FLOOR_DB;
        self.gate_open = false;
        self.clip_hops = 0;
        self.tracker = Tracker::new();
        self.f_display = 0.0;
        self.last = None;
        self.gate_db = self.compute_gate();
    }

    /// One gate policy: auto. The tracked noise floor plus a margin, never
    /// allowed below `GATE_FLOOR_DB` so a quiet room cannot drop the gate
    /// under the noise it is meant to reject. There is no manual gate: nothing
    /// in the UI or the CLI ever set one, and an unreachable second policy is
    /// a second set of numbers to keep honest.
    fn compute_gate(&self) -> f32 {
        (self.floor_db + GATE_AUTO_MARGIN_DB).max(GATE_FLOOR_DB)
    }

    /// Analysis frame for this hop: long while acquiring or while locked below
    /// 55 Hz, short once locked above it.
    fn window(&self) -> usize {
        if !self.tracker.is_locked() {
            return N_MAX;
        }
        if self.tracker.f_center >= F_TRACK_SHORT_HZ {
            N_TRACK
        } else {
            N_MAX
        }
    }

    /// One hop of mono, already high-pass filtered device samples. `None` only
    /// while the frame is not yet full (warm-up); a closed gate still produces
    /// a reading so the UI and the CSV show `NoSignal` with live levels.
    pub fn process_hop(&mut self, hop: &[f32]) -> Option<Reading> {
        if hop.len() != HOP {
            return None;
        }
        self.hist.copy_within(HOP..N_MAX, 0);
        self.hist[N_MAX - HOP..].copy_from_slice(hop);
        self.filled = (self.filled + HOP).min(N_MAX);
        self.hops += 1;
        let t_ms = (self.hops as f64 * HOP as f64 * 1000.0 / self.fs as f64) as u64;

        // b. level + gate
        let mut peak = 0.0f32;
        let mut acc = 0.0f32;
        let mut clipped_now = false;
        for &s in hop {
            let a = s.abs();
            if a > peak {
                peak = a;
            }
            acc += s * s;
            if a >= 0.999 {
                clipped_now = true;
            }
        }
        self.rms_db = dbfs((acc / hop.len() as f32).sqrt());
        self.peak_db = dbfs(peak);
        if clipped_now {
            self.clip_hops = self.clip_hold_len;
        } else if self.clip_hops > 0 {
            self.clip_hops -= 1;
        }
        if self.hops == 1 {
            self.rms_db_smooth = self.rms_db;
        } else {
            let alpha = 1.0 - (-self.hop_dur_ms / 1000.0 / RMS_SMOOTH_S).exp();
            self.rms_db_smooth += alpha * (self.rms_db - self.rms_db_smooth);
        }
        // The floor follows the level down instantly and creeps back up at
        // 0.05 dB/hop (≈ 4.7 dB/s), so a pluck can never lift the gate under
        // itself, and a room louder than the -60 dBFS default floor is gated
        // out after a couple of seconds of steady-state.
        if self.rms_db < self.floor_db {
            self.floor_db = self.rms_db;
        } else {
            self.floor_db = self.rms_db.min(self.floor_db + FLOOR_RISE_DB_PER_HOP);
        }
        self.gate_db = self.compute_gate();
        if !self.gate_open && self.rms_db_smooth >= self.gate_db {
            self.gate_open = true;
        } else if self.gate_open && self.rms_db_smooth < self.gate_db - GATE_HYSTERESIS_DB {
            self.gate_open = false;
        }

        if self.filled < N_MAX {
            return None;
        }

        // c. pitch
        let n = self.window();
        // Correlate the largest window the frame allows: YIN compares x[i] with
        // x[i+tau], so this consumes the newest `w + tau_max` samples. Sizing
        // the window from the current estimate feeds the estimate back into
        // itself, and a competing tone (mains hum) then drags the reading off
        // the note instead of being averaged out.
        let w = n - self.pitch.tau_range().1;
        let win_ms = (n as f32 * 1000.0 / self.fs).round() as u32;
        let frame = &self.hist[N_MAX - n..];
        let pinned_f = self
            .pinned
            .and_then(|p| self.tuning.get(p))
            .map(|&m| crate::model::notes::midi_to_freq(m, self.a4));
        let est = if self.gate_open {
            self.pitch.estimate(frame, w, pinned_f)
        } else {
            None
        };
        let clarity = est.map(|e| e.clarity).unwrap_or(0.0);

        let out = self.tracker.run(&TrackInput {
            gate_open: self.gate_open,
            clarity,
            f_raw: est.map(|e| e.f_hz).unwrap_or(0.0),
            a4: self.a4,
            tuning: &self.tuning,
            pinned: self.pinned,
            tau_smooth_ms: self.tau_smooth_ms,
            hop_dur_ms: self.hop_dur_ms,
        });

        // d. harmonics, always at the frozen/locked display pitch
        if out.state == TrackState::Locked || out.state == TrackState::Acquiring {
            self.f_display = out.f_hz;
        }
        let f_ref = self.f_display;
        if self.gate_open && f_ref > 0.0 {
            let x = &self.hist[N_MAX - n..];
            hann_into(x, &self.hann[..n], &mut self.scratch[..n]);
            goertzel_mags(&self.scratch[..n], self.fs, f_ref, &mut self.amps);
        } else {
            self.amps = [0.0; NH];
        }
        let mut amps = self.amps;
        let mx = amps.iter().cloned().fold(0.0f32, f32::max);
        if mx > 1e-9 {
            for a in amps.iter_mut() {
                *a /= mx;
            }
        } else {
            amps = [0.0; NH];
        }
        let strongest = amps
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i + 1)
            .unwrap_or(1);
        let weak = self.gate_open && mx > 1e-9 && amps[0] < WEAK_FUND_RATIO;

        let hint = if !self.gate_open {
            Some(Hint::LowSignal)
        } else if self.clip_hops > 0 {
            Some(Hint::Clipping)
        } else if weak {
            Some(Hint::WeakFundamental {
                strongest_h: strongest,
            })
        } else {
            out.wrong_preset_semitones
                .map(|semitones| Hint::WrongPreset { semitones })
        };

        let r = Reading {
            t_ms,
            window_ms: win_ms,
            f_hz: out.f_hz,
            cents: out.cents,
            clarity,
            state: out.state,
            target_midi: out.target_midi,
            rms_db: self.rms_db_smooth,
            peak_db: self.peak_db,
            gate_db: self.gate_db,
            clipped: self.clip_hops > 0,
            harmonic_amps: amps,
            hint,
        };
        self.last = Some(r);
        Some(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(fs: f32, f: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| 0.25 * (2.0 * std::f32::consts::PI * f * i as f32 / fs).sin())
            .collect()
    }

    #[test]
    fn warm_up_emits_nothing_then_locks_and_silence_gates() {
        let fs = 48_000.0;
        let mut a = Analyzer::new(fs);
        let mut rows = 0;
        for chunk in tone(fs, 82.41, 48_000).chunks(HOP) {
            if a.process_hop(chunk).is_some() {
                rows += 1;
            }
        }
        assert_eq!(
            rows,
            48_000 / HOP - (N_MAX / HOP - 1),
            "93 chunks, the first 15 only fill the frame"
        );
        let r = a.reading().unwrap();
        assert_eq!(r.state, TrackState::Locked);
        assert_eq!(r.target_midi, Some(40));

        // Silence: the 50 ms level smoothing has to drain before the gate
        // shuts; after that nothing may stay Locked.
        let before = a.reading().unwrap().state;
        assert_eq!(before, TrackState::Locked);
        let mut states = Vec::new();
        for chunk in vec![0.0f32; 24_000].chunks(HOP) {
            if let Some(r) = a.process_hop(chunk) {
                states.push(r.state);
            }
        }
        assert_eq!(states.len(), 24_000 / HOP);
        let drained = 4_800 / HOP;
        assert!(
            states[drained..].iter().all(|s| *s == TrackState::NoSignal),
            "{states:?}"
        );
        assert_eq!(a.reading().unwrap().state, TrackState::NoSignal);
    }

    #[test]
    fn the_auto_floor_rises_above_a_steady_noise_floor() {
        let fs = 48_000.0;
        let mut a = Analyzer::new(fs);
        // White noise at about -55 dBFS: above the -60 default floor, so only a
        // floor tracker that actually rises can keep the gate shut on it.
        let mut st = 9_999u64;
        let amp = 10f32.powf(-55.0 / 20.0) * 3f32.sqrt();
        let noise: Vec<f32> = (0..48_000 * 2)
            .map(|_| {
                st = st
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                ((((st >> 33) as f64 / (1u64 << 31) as f64) as f32) * 2.0 - 1.0) * amp
            })
            .collect();
        let mut rows = Vec::new();
        for chunk in noise.chunks(HOP) {
            if let Some(r) = a.process_hop(chunk) {
                rows.push(r);
            }
        }
        let r = a.reading().unwrap();
        assert!(r.rms_db > -58.0 && r.rms_db < -52.0, "rms {}", r.rms_db);
        assert!(
            r.gate_db > r.rms_db + 5.0,
            "gate {} did not rise above rms {}",
            r.gate_db,
            r.rms_db
        );
        let tail = &rows[rows.len() - 10..];
        assert!(
            tail.iter().all(|r| r.state == TrackState::NoSignal),
            "{:?}",
            tail.iter().map(|r| r.state).collect::<Vec<_>>()
        );
    }

    /// The reading carries the window that produced it so the display can say
    /// what actually ran. Acquiring, and any lock below `F_TRACK_SHORT_HZ`,
    /// correlate `N_MAX` (171 ms at 48 kHz); a lock above it drops to
    /// `N_TRACK` (85 ms) to halve display latency. The chip used to print a
    /// constant derived from `N_MAX` and lied for every note above A1.
    #[test]
    fn the_reading_reports_the_window_it_actually_used() {
        let fs = 48_000.0;
        let long_ms = (N_MAX as f32 * 1000.0 / fs).round() as u32;
        let short_ms = (N_TRACK as f32 * 1000.0 / fs).round() as u32;

        let mut a = Analyzer::new(fs);
        let mut seen = Vec::new();
        for chunk in tone(fs, 110.0, 4_800 * 3).chunks(HOP) {
            if let Some(r) = a.process_hop(chunk) {
                seen.push((r.state, r.window_ms));
            }
        }
        assert_eq!(
            seen[0].1,
            long_ms,
            "acquiring runs the long frame: {:?}",
            seen[0]
        );
        let last = seen[seen.len() - 1];
        assert_eq!(last.0, TrackState::Locked, "{last:?}");
        assert_eq!(
            last.1, short_ms,
            "locked at 110 Hz must run the short frame: {last:?}"
        );

        // F1 stays on the long frame: a short one cannot hold enough periods
        // of it, which is the whole reason two frame lengths exist.
        let mut b = Analyzer::new(fs);
        for chunk in tone(fs, 43.65, 4_800 * 3).chunks(HOP) {
            b.process_hop(chunk);
        }
        let r = b.reading().unwrap();
        assert_eq!(r.state, TrackState::Locked, "43.65 Hz must lock");
        assert_eq!(r.window_ms, long_ms, "43.65 Hz keeps the long frame");
    }

    #[test]
    fn the_search_floor_is_reachable_inside_the_frame() {
        let a = Analyzer::new(48_000.0);
        let (_tmin, tmax) = a.pitch.tau_range();
        assert!(
            W_MIN + tmax <= N_TRACK,
            "short frame {N_TRACK} cannot hold a {W_MIN}-sample correlation plus tau_max {tmax}"
        );
        assert!((a.fs / PITCH_FLOOR_HZ).round() as usize == tmax);
    }
}
