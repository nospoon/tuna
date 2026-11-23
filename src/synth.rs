//! Deterministic test-signal generator, shared by `tests/dsp.rs` and the
//! `wavgen` binary so fixtures and tests measure the same thing.
//!
//! Everything is reproducible from `seed`: no `rand`, no time, no locale.

use crate::model::notes::midi_to_freq;

#[derive(Clone, Debug)]
pub struct SynthParams {
    pub fs: u32,
    pub seconds: f32,
    /// Silence before the note starts, ms.
    pub onset_ms: u32,
    pub midi: Option<u8>,
    /// 0 = pure sine; n>0 = n harmonics.
    pub harmonics: usize,
    /// Harmonic amplitude ladder `1/h^rolloff`: 1 is a plucked string, 0 is a
    /// flat spectrum.
    pub rolloff: f32,
    /// White-noise floor, dBFS (-160 disables it).
    pub noise_db: f32,
    /// Continuous 50 Hz hum, dBFS (None disables it).
    pub hum_db: Option<f32>,
    /// Amplitude of h=1 relative to the others; < 1 builds the weak-fundamental
    /// octave-trap.
    pub fundamental_scale: f32,
    /// Peak-to-peak sinusoidal drift in cents (period 4 s).
    pub drift_cents: f32,
    /// Peak level of the tone, dBFS.
    pub amp_db: f32,
    pub seed: u64,
}

impl Default for SynthParams {
    fn default() -> Self {
        SynthParams {
            fs: 48_000,
            seconds: 3.0,
            onset_ms: 500,
            midi: Some(40),
            harmonics: 0,
            rolloff: 1.0,
            noise_db: -70.0,
            hum_db: None,
            fundamental_scale: 1.0,
            drift_cents: 0.0,
            amp_db: -12.0,
            seed: 1,
        }
    }
}

const TAU: f64 = std::f32::consts::PI as f64 * 2.0;

pub fn render(p: &SynthParams) -> Vec<f32> {
    let fs = p.fs as f64;
    let n = (p.seconds as f64 * fs).round() as usize;
    let mut out = vec![0.0f32; n];
    let f0 = midi_to_freq(p.midi.unwrap_or(40), crate::model::notes::A4_DEFAULT) as f64;
    let onset = (p.onset_ms as f64 * fs / 1000.0) as usize;
    let attack = (0.005 * fs) as usize;
    let decay_tau = 0.6 * fs;
    let nh = if p.harmonics == 0 {
        1
    } else {
        p.harmonics.max(1)
    };

    // a. tone: 5 ms attack, 600 ms exponential decay, drift via phase integration
    let mut phase = 0.0f64;
    for (i, o) in out.iter_mut().enumerate() {
        if i < onset {
            continue;
        }
        let t = (i - onset) as f64 / fs;
        let inst = if p.drift_cents == 0.0 {
            f0
        } else {
            f0 * 2f64.powf(f64::from(p.drift_cents) * (TAU * t / 4.0).sin() / 1200.0)
        };
        phase += TAU * inst / fs;
        let env = if i - onset < attack {
            (i - onset) as f64 / attack as f64
        } else {
            (-(((i - onset) - attack) as f64) / decay_tau).exp()
        };
        let mut s = 0.0f64;
        for h in 1..=nh {
            let amp = if h == 1 {
                f64::from(p.fundamental_scale)
            } else {
                1.0 / (h as f64).powf(f64::from(p.rolloff))
            };
            s += amp * (phase * h as f64).sin();
        }
        *o = (s * env) as f32;
    }

    // b. normalise the tone peak to `amp_db`
    let peak = out.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
    let want = 10f32.powf(p.amp_db / 20.0);
    if peak > 1e-9 {
        let g = want / peak;
        for v in out.iter_mut() {
            *v *= g;
        }
    }

    // c. hum is continuous: it must not be gated by the note envelope
    if let Some(db) = p.hum_db {
        let amp = 10f32.powf(db / 20.0);
        let mut ph = 0.0f64;
        let step = TAU * 50.0 / fs;
        for v in out.iter_mut() {
            ph += step;
            *v += amp * ph.sin() as f32;
        }
    }

    // d. noise floor: uniform, scaled so its rms equals the requested dBFS
    if p.noise_db > -160.0 {
        let rms = 10f32.powf(p.noise_db / 20.0) * 3f32.sqrt();
        let mut st = p.seed;
        for v in out.iter_mut() {
            st = st
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let u = ((st >> 33) as f64 / (1u64 << 31) as f64) as f32 * 2.0 - 1.0;
            *v += u * rms;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_gives_the_same_samples() {
        let a = render(&SynthParams {
            seconds: 0.2,
            seed: 7,
            noise_db: -50.0,
            ..Default::default()
        });
        let b = render(&SynthParams {
            seconds: 0.2,
            seed: 7,
            noise_db: -50.0,
            ..Default::default()
        });
        assert_eq!(a, b);
        let c = render(&SynthParams {
            seconds: 0.2,
            seed: 8,
            noise_db: -50.0,
            ..Default::default()
        });
        assert_ne!(a, c);
    }

    #[test]
    fn peak_level_and_onset_are_honest() {
        let p = SynthParams {
            fs: 48_000,
            seconds: 1.0,
            onset_ms: 300,
            amp_db: -12.0,
            noise_db: -160.0,
            midi: Some(40),
            ..Default::default()
        };
        let x = render(&p);
        let peak = x.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
        assert!(
            (peak - 10f32.powf(-12.0 / 20.0)).abs() < 1e-4,
            "peak {peak}"
        );
        let onset_sample = 0.3 * 48_000.0;
        assert!(
            x[..(onset_sample - 100.0) as usize]
                .iter()
                .all(|&v| v == 0.0),
            "signal before onset"
        );
    }
}
