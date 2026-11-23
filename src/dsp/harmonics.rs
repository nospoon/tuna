//! Harmonic diagnostic: Goertzel magnitudes at h*f for h = 1..=12 on the
//! analysis frame. Twelve Goertzel passes are cheaper than pulling in an FFT
//! crate, and they are only used for the strip + the weak-fundamental hint, so
//! absolute level does not matter — the caller normalises to the maximum.

pub const NH: usize = 12;

/// Magnitudes (0..~1 of full-scale sine), one per harmonic. Frequencies at or
/// above Nyquist come back 0.
pub fn goertzel_mags(x: &[f32], fs: f32, f0: f32, out: &mut [f32; NH]) {
    let n = x.len() as f32;
    for (h, o) in out.iter_mut().enumerate() {
        let f = f0 * (h as f32 + 1.0);
        if f <= 0.0 || f >= fs * 0.5 || n <= 1.0 {
            *o = 0.0;
            continue;
        }
        let w = 2.0 * std::f32::consts::PI * f / fs;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for &v in x {
            let s0 = coeff * s1 - s2 + v;
            s2 = s1;
            s1 = s0;
        }
        let power = s1 * s1 + s2 * s2 - coeff * s1 * s2;
        *o = power.max(0.0).sqrt() / n;
    }
}

/// Hann window into `dst` (len == src.len()), used so the harmonic bars are not
/// smeared by the frame edges.
pub fn hann_into(src: &[f32], table: &[f32], dst: &mut [f32]) {
    for i in 0..src.len() {
        dst[i] = src[i] * table[i];
    }
}

pub fn hann_table(n: usize) -> Vec<f32> {
    if n < 2 {
        return vec![1.0; n];
    }
    (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (n as f32 - 1.0)).cos())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saw_shows_a_decaying_harmonic_ladder() {
        let fs = 48_000.0;
        let f = 100.0;
        let n = 4096;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / fs;
                let mut s = 0.0f32;
                for h in 1..=12 {
                    s += (2.0 * std::f32::consts::PI * f * h as f32 * t).sin() / h as f32;
                }
                s
            })
            .collect();
        let table = hann_table(n);
        let mut win = vec![0.0f32; n];
        hann_into(&x, &table, &mut win);
        let mut amps = [0.0f32; NH];
        goertzel_mags(&win, fs, f, &mut amps);
        assert!(amps[0] > 0.0);
        let norm: Vec<f32> = amps.iter().map(|a| a / amps[0]).collect();
        assert!(norm[1] < 0.75 && norm[1] > 0.35, "h2 = {}", norm[1]);
        assert!(norm[4] < norm[1] && norm[4] > 0.05, "h5 = {}", norm[4]);
        assert!(norm[11] < norm[4]);
    }
}
