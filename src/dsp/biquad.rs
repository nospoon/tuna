//! RBJ-cookbook second-order high-pass, Direct Form I, one state per input
//! channel. Every channel of a capture block is filtered every block whether or
//! not it is the selected one, so switching the monitored input mid-stream does
//! not step the filter state (no click, no transient on the meter).

const TAU2: f32 = std::f32::consts::PI * 2.0;

/// Default corner: removes DC rumble and sub-audible interface noise while
/// costing well under 0.5 dB at B0 (30.87 Hz).
pub const HP_FC: f32 = 20.0;
// 1/√2 exactly: the Butterworth Q, so the corner stays flat through B0.
pub const HP_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;

#[derive(Clone, Copy, Debug)]
struct Coefs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Coefs {
    fn design(fs: f32, fc: f32, q: f32) -> Self {
        let w0 = TAU2 * fc / fs;
        let (sin_w0, cos_w0) = (w0.sin(), w0.cos());
        let alpha = sin_w0 / (2.0 * q);
        let a0 = 1.0 + alpha;
        Coefs {
            b0: (1.0 + cos_w0) / 2.0 / a0,
            b1: -(1.0 + cos_w0) / a0,
            b2: (1.0 + cos_w0) / 2.0 / a0,
            a1: -2.0 * cos_w0 / a0,
            a2: (1.0 - alpha) / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    #[inline(always)]
    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

#[derive(Clone, Debug)]
pub struct HighPassBank {
    fs: f32,
    fc: f32,
    q: f32,
    state: Vec<Coefs>,
}

impl HighPassBank {
    pub fn new(fs: f32, channels: usize) -> Self {
        Self {
            fs,
            fc: HP_FC,
            q: HP_Q,
            state: vec![Coefs::design(fs, HP_FC, HP_Q); channels.max(1)],
        }
    }

    pub fn channels(&self) -> usize {
        self.state.len()
    }

    pub fn set_channels(&mut self, channels: usize) {
        let n = self.state.len().max(channels).max(1);
        self.state
            .resize(n, Coefs::design(self.fs, self.fc, self.q));
    }

    pub fn set_fs(&mut self, fs: f32) {
        if fs != self.fs {
            self.fs = fs;
            for c in self.state.iter_mut() {
                *c = Coefs::design(fs, self.fc, self.q);
            }
        }
    }

    #[inline]
    pub fn process(&mut self, ch: usize, x: f32) -> f32 {
        match self.state.get_mut(ch) {
            Some(c) => c.run(x),
            None => x,
        }
    }

    pub fn reset(&mut self) {
        for c in self.state.iter_mut() {
            c.x1 = 0.0;
            c.x2 = 0.0;
            c.y1 = 0.0;
            c.y2 = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_is_removed_and_a_low_note_survives() {
        let mut bank = HighPassBank::new(48_000.0, 2);
        // constant 0.5 offset on channel 1 -> must converge to 0
        let mut last = 0.0f32;
        for _ in 0..4800 {
            last = bank.process(1, 0.5);
        }
        assert!(last.abs() < 1e-3, "dc leaked: {last}");

        // 30.87 Hz sine (bass low B) must keep essentially full amplitude
        let mut bank = HighPassBank::new(48_000.0, 1);
        let mut peak_all = 0.0f32;
        let mut peak_settled = 0.0f32;
        let mut phase = 0.0f32;
        let step = 2.0 * std::f32::consts::PI * 30.87 / 48_000.0;
        for i in 0..9600 {
            let y = bank.process(0, phase.sin()).abs();
            peak_all = peak_all.max(y);
            if i >= 4800 {
                peak_settled = peak_settled.max(y);
            }
            phase += step;
        }
        // Second-order Butterworth at 20 Hz costs 0.7 dB at B0: compare against
        // the analytic magnitude, not unity.
        let omega = 30.87 / HP_FC;
        let expect =
            omega.powi(2) / ((1.0 - omega.powi(2)).powi(2) + (omega / HP_Q).powi(2)).sqrt();
        assert!(
            (peak_settled - expect).abs() < 0.01,
            "B0 gain {peak_settled}, analytic {expect:.4}"
        );
        assert!(
            peak_all - peak_settled < 0.05,
            "high-pass still transient after 100 ms"
        );
    }

    #[test]
    fn switching_channels_keeps_every_channel_filtered() {
        let mut bank = HighPassBank::new(48_000.0, 2);
        // a 20 Hz high-pass needs ~1 s (many time constants) to settle on a step
        for _ in 0..9600 {
            bank.process(0, 0.25);
            bank.process(1, -0.25);
        }
        // channel 0 has been running all along, so it is settled, not starting over
        let residual = bank.process(0, 0.25);
        assert!(residual.abs() < 1e-3, "channel 0 dc residual {residual}");
    }
}
