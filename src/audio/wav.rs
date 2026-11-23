//! Minimal RIFF/WAVE reader (16/24/32-bit int, 32-bit float, mono or stereo)
//! and a 16-bit PCM mono writer for `wavgen`. No new dependency: the app already
//! depends on nothing else that can do this.

use std::path::Path;

#[derive(Clone, Debug)]
pub struct Wav {
    pub fs: u32,
    pub channels: u16,
    pub bits: u16,
    pub format: &'static str,
    /// Interleaved frames.
    pub data: Vec<f32>,
}

fn u16le(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn u32le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

impl Wav {
    pub fn read(path: &Path) -> Result<Wav, String> {
        let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if b.len() < 44 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
            return Err(format!("{} is not a RIFF/WAVE file", path.display()));
        }
        let mut pos = 12;
        let mut fmt: Option<(u16, u16, u32, u16)> = None; // (tag, channels, fs, bits)
        let mut data: Option<(usize, usize)> = None; // (start, len)
        while pos + 8 <= b.len() {
            let id = &b[pos..pos + 4];
            let size = u32le(&b, pos + 4) as usize;
            let body = (pos + 8).min(b.len());
            let end = (body + size).min(b.len());
            match id {
                b"fmt " if end - body >= 16 => {
                    let tag = u16le(&b, body);
                    let channels = u16le(&b, body + 2);
                    let fs = u32le(&b, body + 4);
                    let bits = u16le(&b, body + 14);
                    // WAVE_FORMAT_EXTENSIBLE: the sub-format GUID's Data1 holds the real tag
                    let tag = if tag == 0xFFFE && end - body >= 40 {
                        u32le(&b, body + 24) as u16
                    } else {
                        tag
                    };
                    fmt = Some((tag, channels, fs, bits));
                }
                b"data" => data = Some((body, end - body)),
                _ => {}
            }
            if data.is_some() && fmt.is_some() && id == b"data" {
                break;
            }
            // chunks are word-aligned
            pos = body + size + (size & 1);
        }
        let (tag, channels, fs, bits) = fmt.ok_or("WAVE has no fmt chunk")?;
        let (start, len) = data.ok_or("WAVE has no data chunk")?;
        if channels == 0 || fs == 0 {
            return Err("WAVE header has zero channels or sample rate".into());
        }
        let bytes_per = bits / 8;
        let count = len / bytes_per.max(1) as usize;
        let mut out = vec![0.0f32; count];
        let format;
        match (tag, bits) {
            (1, 16) => {
                format = "s16pcm";
                for (i, s) in out.iter_mut().enumerate() {
                    let o = start + i * 2;
                    if o + 2 > b.len() {
                        break;
                    }
                    *s = i16::from_le_bytes([b[o], b[o + 1]]) as f32 / 32768.0;
                }
            }
            (1, 24) => {
                format = "s24pcm";
                for (i, s) in out.iter_mut().enumerate() {
                    let o = start + i * 3;
                    if o + 3 > b.len() {
                        break;
                    }
                    let v = (b[o] as u32) | (b[o + 1] as u32) << 8 | (b[o + 2] as u32) << 16;
                    let v = ((v as i32) << 8) >> 8;
                    *s = v as f32 / 8_388_608.0;
                }
            }
            (1, 32) => {
                format = "s32pcm";
                for (i, s) in out.iter_mut().enumerate() {
                    let o = start + i * 4;
                    if o + 4 > b.len() {
                        break;
                    }
                    *s = i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as f32
                        / 2_147_483_648.0;
                }
            }
            (3, 32) => {
                format = "f32";
                for (i, s) in out.iter_mut().enumerate() {
                    let o = start + i * 4;
                    if o + 4 > b.len() {
                        break;
                    }
                    *s = f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
                }
            }
            _ => return Err(format!("unsupported WAVE format tag {tag}, {bits} bits")),
        }
        Ok(Wav {
            fs,
            channels,
            bits,
            format,
            data: out,
        })
    }

    /// 16-bit PCM mono, the format `wavgen` writes and the tuner reads.
    pub fn write_mono_pcm16(path: &Path, fs: u32, data: &[f32]) -> Result<(), String> {
        let bytes = data.len() * 2;
        let mut out = Vec::with_capacity(44 + bytes);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((36 + bytes) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&1u16.to_le_bytes()); // mono
        out.extend_from_slice(&fs.to_le_bytes());
        out.extend_from_slice(&(fs * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(bytes as u32).to_le_bytes());
        for &s in data {
            let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            out.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(path, out).map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_pcm16_mono() {
        let dir = std::env::temp_dir().join(format!("tuna-wav-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.wav");
        let sig: Vec<f32> = (0..1000)
            .map(|i| 0.5f32 * (i as f32 * 0.01).sin())
            .collect();
        Wav::write_mono_pcm16(&p, 48_000, &sig).unwrap();
        let w = Wav::read(&p).unwrap();
        assert_eq!(w.fs, 48_000);
        assert_eq!(w.channels, 1);
        assert_eq!(w.format, "s16pcm");
        assert_eq!(w.data.len(), 1000);
        let err = w
            .data
            .iter()
            .zip(sig.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(err < 1e-3, "quantisation error {err}");
        let _ = std::fs::remove_file(&p);
    }
}
