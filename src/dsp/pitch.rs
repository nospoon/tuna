//! Grid YIN + cumulative-mean normalisation + integer-lag refinement.
//!
//! This is the octave-error locus. The candidate grid is generated with
//! **ascending lag** (= descending frequency), the cumulative mean is taken over
//! that same ascending order, and the accepted estimate is the **first local
//! minimum whose normalised value is under `threshold`**. "Smallest lag wins" is
//! what stops a harmonic-rich bass signal (strong h=2, h=3) from locking an
//! octave up. Reverse the scan direction and the whole app octave-ups.

use crate::model::notes::{cents_between, PITCH_CEIL_HZ, PITCH_FLOOR_HZ};

/// Coarse grid: 1200 cents / 100 cents => 67 steps, plus the endpoints.
const GRID_K: usize = 67;

/// Distance (in lag samples) from a 100-cent grid point to the true lag, worst
/// case: 1 - 2^(-50/1200) = 0.0283, rounded up with a guard sample.
const GRID_REL_ERR: f32 = 0.0293;

/// Fallback ceiling on the normalised difference when no local minimum clears
/// the threshold. Above this the frame is treated as unvoiced.
const FALLBACK_LIMIT: f32 = 0.25;

#[derive(Clone, Copy, Debug)]
pub struct Est {
    pub f_hz: f32,
    /// 1 - CMNDF at the chosen lag, clamped to 0..1.
    pub clarity: f32,
    /// Interpolated lag in samples.
    pub tau: f32,
    /// Normalised difference at the chosen lag (pre-clamp).
    pub cmndf: f32,
}

pub struct Pitch {
    fs: f32,
    tau_min: usize,
    tau_max: usize,
    /// Ascending, deduplicated candidate lags.
    taus: Vec<usize>,
    /// fs / tau for each candidate, for the pinned-string window filter.
    f_of_tau: Vec<f32>,
    d: Vec<f32>,
    dnorm: Vec<f32>,
    cand: Vec<usize>,
    refine: Vec<f32>,
}

/// `d(tau) = sum_{n<W} (x[n] - x[n+tau])^2`. Plain indexed loops over a
/// contiguous slice so LLVM vectorises it.
#[inline]
fn diff(x: &[f32], w: usize, tau: usize) -> f32 {
    let mut s = 0.0f32;
    for i in 0..w {
        let dv = x[i] - x[i + tau];
        s += dv * dv;
    }
    s
}

impl Pitch {
    pub fn new(fs: f32) -> Self {
        let tau_min = (fs / PITCH_CEIL_HZ).round() as usize;
        let tau_max = (fs / PITCH_FLOOR_HZ).round() as usize;
        let mut taus: Vec<usize> = Vec::with_capacity(GRID_K);
        for k in 0..GRID_K {
            let f = PITCH_CEIL_HZ * 2f32.powf(-(k as f32) / 12.0);
            let mut t = (fs / f).round() as usize;
            t = t.clamp(tau_min, tau_max);
            if taus.last() != Some(&t) {
                taus.push(t);
            }
        }
        // the bottom of the search window must be an evaluated grid point, not
        // just a bound: 27.5 Hz (the pitch floor) has to be reachable
        if taus.last() != Some(&tau_max) {
            taus.push(tau_max);
        }
        let f_of_tau = taus.iter().map(|&t| fs / t as f32).collect();
        let n = taus.len();
        let delta_max = (GRID_REL_ERR * tau_max as f32).ceil() as usize + 1;
        Pitch {
            fs,
            tau_min,
            tau_max,
            taus,
            f_of_tau,
            d: vec![0.0; n],
            dnorm: vec![0.0; n],
            cand: Vec::with_capacity(n),
            refine: vec![0.0; 2 * delta_max + 1],
        }
    }

    pub fn tau_range(&self) -> (usize, usize) {
        (self.tau_min, self.tau_max)
    }

    /// Estimate the fundamental of the frame `x` (length `n`), correlating over
    /// the leading `w` samples. `pinned_f` restricts the search to lags within a
    /// semitone of a known target (pinned-string mode).
    pub fn estimate(&mut self, x: &[f32], w: usize, pinned_f: Option<f32>) -> Option<Est> {
        let n = x.len();
        if n < 4 || w == 0 || w + self.tau_min > n {
            return None;
        }
        let max_tau = n - w;

        // Candidates, ascending lag, respecting the frame length and, in pinned
        // mode, the ±120 cent target window (one extra grid step of slack so a
        // minimum sitting on the window edge is still found).
        self.cand.clear();
        if let Some(pf) = pinned_f {
            for (i, &t) in self.taus.iter().enumerate() {
                if t <= max_tau && cents_between(self.f_of_tau[i], pf).abs() <= 220.0 {
                    self.cand.push(i);
                }
            }
        }
        if self.cand.is_empty() {
            for (i, &t) in self.taus.iter().enumerate() {
                if t <= max_tau {
                    self.cand.push(i);
                }
            }
        }
        if self.cand.is_empty() {
            return None;
        }

        // Difference function over the candidate grid.
        for (j, &gi) in self.cand.iter().enumerate() {
            self.d[j] = diff(x, w, self.taus[gi]);
        }

        // Cumulative-mean normalisation, ascending lag.
        let mut cum = 0.0f32;
        for (j, _) in self.cand.iter().enumerate() {
            cum += self.d[j];
            let mean = cum / (j as f32 + 1.0);
            self.dnorm[j] = if mean > 1e-20 { self.d[j] / mean } else { 1.0 };
        }

        // First local minimum under the threshold.
        let thresh = crate::dsp::frame::YIN_THRESHOLD;
        let m = self.cand.len();
        let mut pick = None;
        for j in 1..m {
            if self.dnorm[j] < thresh && (j + 1 == m || self.dnorm[j] <= self.dnorm[j + 1]) {
                pick = Some(j);
                break;
            }
        }
        // m == 1 (extremely narrow pinned window): accept the single point.
        if pick.is_none() && m == 1 && self.dnorm[0] < thresh {
            pick = Some(0);
        }
        // Otherwise the global minimum, only if the frame is periodic at all.
        let chosen = pick
            .or_else(|| {
                let mut g = 0usize;
                for j in 1..m {
                    if self.dnorm[j] < self.dnorm[g] {
                        g = j;
                    }
                }
                (self.dnorm[g] < FALLBACK_LIMIT).then_some(g)
            })?;

        let tau_k = self.taus[self.cand[chosen]];
        let cmndf = self.dnorm[chosen];

        // Integer refinement around the coarse lag, then parabolic interpolation.
        let delta = ((GRID_REL_ERR * tau_k as f32).ceil() as usize + 1).max(3);
        let lo = tau_k.saturating_sub(delta).max(self.tau_min);
        let hi = (tau_k + delta).min(max_tau).min(self.tau_max);
        if lo > hi {
            return Some(Est {
                f_hz: self.fs / tau_k as f32,
                clarity: (1.0f32 - cmndf).clamp(0.0, 1.0),
                tau: tau_k as f32,
                cmndf,
            });
        }
        let span = hi - lo + 1;
        for i in 0..span {
            self.refine[i] = diff(x, w, lo + i);
        }
        let mut b = 0usize;
        for i in 1..span {
            if self.refine[i] < self.refine[b] {
                b = i;
            }
        }
        let mut tau_hat = (lo + b) as f32;
        if b > 0 && b + 1 < span {
            let dm = self.refine[b - 1];
            let d0 = self.refine[b];
            let dp = self.refine[b + 1];
            let den = dm - 2.0 * d0 + dp;
            if den.abs() > 1e-12 {
                let s = 0.5 * (dm - dp) / den;
                if s.abs() <= 1.0 {
                    tau_hat += s;
                }
            }
        }
        if tau_hat < 1.0 {
            tau_hat = 1.0;
        }
        Some(Est {
            f_hz: self.fs / tau_hat,
            clarity: (1.0f32 - cmndf).clamp(0.0, 1.0),
            tau: tau_hat,
            cmndf,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(fs: f32, f: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * f * i as f32 / fs).sin())
            .collect()
    }

    /// Harmonic stack with an intentionally weak fundamental: the smallest-lag
    /// rule must still return the true fundamental, not an octave up.
    fn stack(fs: f32, f: f32, n: usize, h1scale: f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let mut s = 0.0f32;
                for h in 1..=8 {
                    let amp = if h == 1 { h1scale } else { 1.0 / h as f32 };
                    s += amp
                        * (2.0 * std::f32::consts::PI * f * h as f32 * i as f32 / fs
                            + 0.25 * h as f32)
                            .sin();
                }
                s
            })
            .collect()
    }

    #[test]
    fn grid_is_ascending_lag() {
        let p = Pitch::new(48_000.0);
        let (lo, hi) = p.tau_range();
        assert_eq!(lo, 37);
        assert_eq!(hi, 1745);
        assert!(p.taus.windows(2).all(|w2| w2[0] < w2[1]));
        assert_eq!(p.taus.last().copied(), Some(hi));
    }

    #[test]
    fn sine_lags_are_found_exactly() {
        let mut p = Pitch::new(48_000.0);
        for &f in &[30.87f32, 43.65, 82.41, 329.63] {
            let x = sine(48_000.0, f, 8192);
            let e = p.estimate(&x, 4096, None).expect("voiced");
            let cents = cents_between(e.f_hz, f);
            assert!(cents.abs() < 2.0, "{f} Hz -> {} Hz ({cents} cents)", e.f_hz);
            assert!(e.clarity > 0.9, "clarity {}", e.clarity);
        }
    }

    #[test]
    fn weak_fundamental_does_not_octave_up() {
        let mut p = Pitch::new(48_000.0);
        let x = stack(48_000.0, 30.87, 8192, 0.25);
        let e = p.estimate(&x, 4096, None).expect("voiced");
        let cents = cents_between(e.f_hz, 30.87);
        assert!(
            cents.abs() < 50.0,
            "octave error: {} Hz ({cents} cents)",
            e.f_hz
        );
    }

    #[test]
    fn pinned_window_rejects_distant_lags() {
        let mut p = Pitch::new(48_000.0);
        // Signal at 82.4 Hz, search pinned to 43.65 Hz: the result must stay
        // inside the pinned window instead of tracking the real note.
        let x = sine(48_000.0, 82.41, 8192);
        let e = p.estimate(&x, 4096, Some(43.65)).expect("voiced");
        assert!(
            cents_between(e.f_hz, 43.65).abs() <= 150.0,
            "escaped window: {}",
            e.f_hz
        );
    }
}
