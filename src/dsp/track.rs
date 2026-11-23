//! The anti-floating layer. A good `f_raw` still flickers without this file:
//! low clarity freezes the display instead of sliding toward noise, garbage
//! within ±120 cents of the target is clamped rather than allowed to drag the
//! needle, a note change has to survive `NOTE_CHANGE_CONFIRM` frames and then
//! snaps (EMA reset) instead of sweeping across the dial, and both readouts are
//! derived from one smoothed cents value.

use crate::dsp::frame::{CLARITY_ACCEPT, LOCK_CONFIRM, NOTE_CHANGE_CONFIRM, OUTLIER_CENTS};
use crate::dsp::harmonics::NH;
use crate::model::notes::{cents_between, freq_to_midi, midi_to_freq, nearest_target};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TrackState {
    /// Nothing analysed yet.
    Idle,
    /// Gate closed.
    NoSignal,
    /// Measuring, waiting for a stable estimate.
    Acquiring,
    /// Periodicity is too weak to display; the meter is frozen at its last value.
    Unstable,
    /// Locked: the displayed cents are trustworthy.
    Locked,
}

impl TrackState {
    pub fn name(self) -> &'static str {
        match self {
            TrackState::Idle => "Idle",
            TrackState::NoSignal => "NoSignal",
            TrackState::Acquiring => "Acquiring",
            TrackState::Unstable => "Unstable",
            TrackState::Locked => "Locked",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hint {
    LowSignal,
    Clipping,
    /// `strongest_h` is 1-based.
    WeakFundamental {
        strongest_h: usize,
    },
    /// Measured pitch sits `semitones` away from the selected target.
    WrongPreset {
        semitones: i32,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct Reading {
    pub t_ms: u64,
    /// Smoothed frequency, derived from `cents` so the two can never disagree.
    pub f_hz: f32,
    pub cents: f32,
    pub clarity: f32,
    pub state: TrackState,
    pub target_midi: Option<u8>,
    pub rms_db: f32,
    pub peak_db: f32,
    pub gate_db: f32,
    pub clipped: bool,
    /// h = 1..=12, normalised so the loudest harmonic is 1.0.
    pub harmonic_amps: [f32; NH],
    pub hint: Option<Hint>,
}

#[derive(Clone, Copy, Debug)]
pub struct TrackInput<'a> {
    pub gate_open: bool,
    pub clarity: f32,
    pub f_raw: f32,
    pub a4: f32,
    /// Target strings (low → high). Empty in chromatic mode.
    pub tuning: &'a [u8],
    /// Index into `tuning`; None = follow the measured note.
    pub pinned: Option<usize>,
    pub tau_smooth_ms: f32,
    pub hop_dur_ms: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct TrackOut {
    pub state: TrackState,
    pub f_hz: f32,
    pub cents: f32,
    pub target_midi: Option<u8>,
    pub wrong_preset_semitones: Option<i32>,
}

#[derive(Clone, Copy, Debug, Default)]
struct Med3 {
    buf: [f32; 3],
    len: usize,
}

impl Med3 {
    fn push(&mut self, v: f32) -> f32 {
        if self.len < 3 {
            self.buf[self.len] = v;
            self.len += 1;
        } else {
            self.buf[0] = self.buf[1];
            self.buf[1] = self.buf[2];
            self.buf[2] = v;
        }
        let mut s = self.buf[..self.len].to_vec();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        s[s.len() / 2]
    }
}

#[derive(Clone, Debug)]
pub struct Tracker {
    pub state: TrackState,
    pub locked: bool,
    pub target_midi: u8,
    /// Last stable centre estimate, used to pick the analysis window.
    pub f_center: f32,
    med_f: Med3,
    med_c: Med3,
    confirm: [f32; LOCK_CONFIRM],
    confirm_len: usize,
    outlier_run: usize,
    ema: Option<f32>,
    last_f: f32,
    last_cents: f32,
    last_target: Option<u8>,
}

impl Default for Tracker {
    fn default() -> Self {
        Self {
            state: TrackState::Idle,
            locked: false,
            target_midi: 0,
            f_center: 0.0,
            med_f: Med3::default(),
            med_c: Med3::default(),
            confirm: [0.0; LOCK_CONFIRM],
            confirm_len: 0,
            outlier_run: 0,
            ema: None,
            last_f: 0.0,
            last_cents: 0.0,
            last_target: None,
        }
    }
}

impl Tracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    pub fn target(&self) -> Option<u8> {
        self.last_target
    }

    /// Tuning or pinned-string changed: throw away the lock so the target is
    /// re-chosen, but keep nothing stale on screen.
    pub fn retarget(&mut self) {
        self.locked = false;
        self.state = TrackState::Acquiring;
        self.confirm_len = 0;
        self.outlier_run = 0;
        self.ema = None;
        self.med_f = Med3::default();
        self.med_c = Med3::default();
    }

    fn silence_reset(&mut self) {
        self.locked = false;
        self.confirm_len = 0;
        self.outlier_run = 0;
        self.ema = None;
        self.med_f = Med3::default();
        self.med_c = Med3::default();
    }

    fn frozen(&self, state: TrackState) -> TrackOut {
        TrackOut {
            state,
            f_hz: self.last_f,
            cents: self.last_cents,
            target_midi: self.last_target,
            wrong_preset_semitones: None,
        }
    }

    pub fn run(&mut self, i: &TrackInput) -> TrackOut {
        // 1. gate closed
        if !i.gate_open {
            self.silence_reset();
            self.state = TrackState::NoSignal;
            return self.frozen(TrackState::NoSignal);
        }
        // 2. too aperiodic to trust: freeze instead of drifting
        if i.clarity < CLARITY_ACCEPT {
            self.state = TrackState::Unstable;
            return self.frozen(TrackState::Unstable);
        }
        // 3. reject single-frame spikes before anything else
        let med = self.med_f.push(i.f_raw);

        // 4. acquisition: three consecutive medians that agree
        if !self.locked {
            self.state = TrackState::Acquiring;
            if self.confirm_len < LOCK_CONFIRM {
                self.confirm[self.confirm_len] = med;
                self.confirm_len += 1;
            } else {
                // Slide the window with `copy_within`: element counts stay
                // aligned. `split_at_mut(LOCK_CONFIRM - 1)` returned a tail
                // one element shorter than the head, which panicked on the
                // first frame after the window filled without locking.
                self.confirm.copy_within(1.., 0);
                self.confirm[LOCK_CONFIRM - 1] = med;
            }
            if self.confirm_len == LOCK_CONFIRM {
                let lo = self.confirm.iter().cloned().fold(f32::INFINITY, f32::min);
                let hi = self
                    .confirm
                    .iter()
                    .cloned()
                    .fold(f32::NEG_INFINITY, f32::max);
                if cents_between(hi, lo) <= crate::dsp::frame::LOCK_CONFIRM_CENTS {
                    self.locked = true;
                    self.f_center = med;
                    self.target_midi =
                        pinned_midi(i).unwrap_or_else(|| nearest_target(med, i.a4, i.tuning));
                    let tf = midi_to_freq(self.target_midi, i.a4);
                    self.ema = Some(cents_between(med, tf).clamp(-OUTLIER_CENTS, OUTLIER_CENTS));
                    self.outlier_run = 0;
                    self.med_c = Med3::default();
                    self.last_target = Some(self.target_midi);
                    self.state = TrackState::Locked;
                }
            }
            if !self.locked {
                self.last_f = med;
                return TrackOut {
                    state: self.state,
                    f_hz: med,
                    cents: 0.0,
                    target_midi: None,
                    wrong_preset_semitones: None,
                };
            }
        }

        // 5. locked: outliers are clamped, and only a sustained run changes note
        let mut tf = midi_to_freq(self.target_midi, i.a4);
        let mut f_used = med;
        let dev = cents_between(med, tf);
        if dev.abs() > OUTLIER_CENTS {
            self.outlier_run += 1;
            if self.outlier_run >= NOTE_CHANGE_CONFIRM {
                let new_note =
                    pinned_midi(i).unwrap_or_else(|| nearest_target(med, i.a4, i.tuning));
                // Different note: switch target. Same note: a genuinely detuned
                // string, so re-centre without renaming it. Either way the
                // smoother restarts at the fresh median -> snap, no sweep.
                self.target_midi = new_note;
                tf = midi_to_freq(self.target_midi, i.a4);
                self.f_center = med;
                self.med_c = Med3::default();
                self.ema = Some(cents_between(med, tf).clamp(-OUTLIER_CENTS, OUTLIER_CENTS));
                self.outlier_run = 0;
            } else {
                let sign = if dev > 0.0 { 1.0 } else { -1.0 };
                f_used = tf * 2f32.powf(sign * OUTLIER_CENTS / 1200.0);
            }
        } else {
            self.outlier_run = 0;
        }

        // 6. median then EMA
        let cents_raw = cents_between(f_used, tf);
        let med_c = self.med_c.push(cents_raw);
        let alpha = 1.0 - (-(i.hop_dur_ms / i.tau_smooth_ms)).exp();
        let ema = match self.ema {
            Some(e) => e + alpha * (med_c - e),
            None => med_c,
        };
        self.ema = Some(ema);

        // 7. Hz and cents from the one smoothed value
        let f_hz = tf * 2f32.powf(ema / 1200.0);
        self.last_f = f_hz;
        self.last_cents = ema;
        self.last_target = Some(self.target_midi);
        self.state = TrackState::Locked;

        // 9. measured pitch far from the selected target
        let off = (freq_to_midi(med, i.a4) - self.target_midi as f32).abs();
        TrackOut {
            state: self.state,
            f_hz,
            cents: ema,
            target_midi: Some(self.target_midi),
            wrong_preset_semitones: if off >= 1.5 {
                Some(off.round() as i32)
            } else {
                None
            },
        }
    }
}

fn pinned_midi(i: &TrackInput) -> Option<u8> {
    match i.pinned {
        Some(p) => i.tuning.get(p).copied(),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(f: f32, clarity: f32, tuning: &'a [u8]) -> TrackInput<'a> {
        TrackInput {
            gate_open: true,
            clarity,
            f_raw: f,
            a4: 440.0,
            tuning,
            pinned: None,
            tau_smooth_ms: 140.0,
            hop_dur_ms: 10.7,
        }
    }

    #[test]
    fn locks_then_holds_still() {
        let tuning = [23u8, 28, 33];
        let mut t = Tracker::new();
        let mut out = TrackOut {
            state: TrackState::Idle,
            f_hz: 0.0,
            cents: 0.0,
            target_midi: None,
            wrong_preset_semitones: None,
        };
        for _ in 0..40 {
            out = t.run(&input(30.87, 0.95, &tuning));
        }
        assert_eq!(out.state, TrackState::Locked);
        assert_eq!(out.target_midi, Some(23));
        assert!(out.cents.abs() < 0.2, "cents {}", out.cents);
    }

    #[test]
    fn low_clarity_freezes_the_display() {
        let tuning: [u8; 0] = [];
        let mut t = Tracker::new();
        let prev = loop_once(&mut t, 30.87, 0.95, &tuning, 40);
        // Garbage arrives with low clarity: the needle must not move at all.
        let out = t.run(&input(900.0, 0.2, &tuning));
        assert_eq!(out.state, TrackState::Unstable);
        assert_eq!(
            out.cents, prev.cents,
            "needle moved while clarity was too low"
        );
        assert_eq!(
            out.f_hz, prev.f_hz,
            "frequency moved while clarity was too low"
        );
        assert!(
            out.cents.abs() < 1.0,
            "needle not sitting on B0: {}",
            out.cents
        );
        assert!((out.f_hz - 30.87).abs() < 0.05, "moved to {}", out.f_hz);
    }

    fn loop_once(t: &mut Tracker, f: f32, clarity: f32, tuning: &[u8], n: usize) -> TrackOut {
        let mut out = TrackOut {
            state: TrackState::Idle,
            f_hz: 0.0,
            cents: 0.0,
            target_midi: None,
            wrong_preset_semitones: None,
        };
        for _ in 0..n {
            out = t.run(&input(f, clarity, tuning));
        }
        out
    }

    #[test]
    fn a_single_spike_cannot_drag_the_needle_but_a_sustained_change_snaps() {
        let tuning: [u8; 0] = [];
        let mut t = Tracker::new();
        for _ in 0..40 {
            t.run(&input(30.87, 0.95, &tuning));
        }
        // one wild frame: clamped, so the needle stays inside the window
        let one = t.run(&input(300.0, 0.95, &tuning));
        assert!(one.cents.abs() <= OUTLIER_CENTS + 1.0, "{}", one.cents);
        // sustained: after NOTE_CHANGE_CONFIRM frames the target switches
        let mut out = one;
        for _ in 0..NOTE_CHANGE_CONFIRM + 6 {
            out = t.run(&input(41.20, 0.95, &tuning));
        }
        assert_eq!(out.target_midi, Some(28));
        assert!(out.cents.abs() < 1.0, "{}", out.cents);
    }

    /// Acquisition that never converges has to keep sliding the confirm
    /// window, not walk off the end of it. The old `split_at_mut` shift
    /// panicked on frame `LOCK_CONFIRM + 1`.
    #[test]
    fn a_stalled_acquisition_slides_the_window_and_can_still_lock() {
        let tuning: [u8; 0] = [];
        let mut t = Tracker::new();
        // Alternating medians ~500 cents apart: no lock forms, so every frame
        // from the third on takes the shift path.
        for k in 0..4 * LOCK_CONFIRM {
            let f = if k % 2 == 0 { 30.87 } else { 41.20 };
            let out = t.run(&input(f, 0.95, &tuning));
            assert_eq!(out.state, TrackState::Acquiring, "frame {k}");
        }
        // Steady note: the stale alternation must have aged out of the window,
        // so the lock forms without a reset.
        let out = loop_once(&mut t, 41.20, 0.95, &tuning, 2 * LOCK_CONFIRM);
        assert_eq!(out.state, TrackState::Locked);
        assert_eq!(out.target_midi, Some(28));
        assert!(out.cents.abs() < 1.0, "{}", out.cents);
    }
}
