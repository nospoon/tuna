//! Fixture generator: writes deterministic 16-bit PCM mono WAVs that exercise
//! the exact failure modes the tuner must survive (low fundamentals, weak
//! fundamentals, 50 Hz hum, drift).
//!
//! ```text
//! tuna_wavgen --out fixtures/b0.wav --midi 23 --seed 7
//! ```

use std::path::PathBuf;

use tuna::audio::wav::Wav;
use tuna::synth::{render, SynthParams};

fn val(args: &[String], i: &mut usize, name: &str, bad: &mut Vec<String>) -> Option<String> {
    if *i + 1 < args.len() {
        let v = args[*i + 1].clone();
        *i += 1;
        Some(v)
    } else {
        bad.push(format!("{name} needs a value"));
        None
    }
}

fn num(args: &[String], i: &mut usize, name: &str, bad: &mut Vec<String>) -> Option<f64> {
    let s = val(args, i, name, bad)?;
    match s.parse::<f64>() {
        Ok(v) => Some(v),
        Err(_) => {
            bad.push(format!("{name} needs a number, got {s}"));
            None
        }
    }
}

const USAGE: &str = "\
usage: tuna_wavgen --out <path> [--fs 48000] [--seconds 6] [--onset 300]
                   [--midi 23] [--harmonics 8] [--rolloff 1.0] [--noise -60]
                   [--hum -40] [--fundamental-scale 0.25] [--drift 20]
                   [--amp-db -12] [--seed 7]";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut p = SynthParams::default();
    let mut out: Option<PathBuf> = None;
    let mut bad: Vec<String> = Vec::new();
    let mut i = 0;

    while i < args.len() {
        let key = args[i].as_str();
        match key {
            "--out" => out = val(&args, &mut i, "--out", &mut bad).map(PathBuf::from),
            "--fs" => {
                if let Some(v) = num(&args, &mut i, "--fs", &mut bad) {
                    p.fs = v as u32;
                }
            }
            "--seconds" => {
                if let Some(v) = num(&args, &mut i, "--seconds", &mut bad) {
                    p.seconds = v as f32;
                }
            }
            "--onset" => {
                if let Some(v) = num(&args, &mut i, "--onset", &mut bad) {
                    p.onset_ms = v as u32;
                }
            }
            "--midi" => {
                let s = val(&args, &mut i, "--midi", &mut bad);
                if let Some(s) = s {
                    match s.parse::<u8>() {
                        Ok(v) => p.midi = Some(v),
                        Err(_) => bad.push(format!("--midi needs a MIDI number 0..255, got {s}")),
                    }
                }
            }
            "--harmonics" => {
                if let Some(v) = num(&args, &mut i, "--harmonics", &mut bad) {
                    p.harmonics = v as usize;
                }
            }
            "--rolloff" => {
                if let Some(v) = num(&args, &mut i, "--rolloff", &mut bad) {
                    p.rolloff = v as f32;
                }
            }
            "--noise" => {
                if let Some(v) = num(&args, &mut i, "--noise", &mut bad) {
                    p.noise_db = v as f32;
                }
            }
            "--hum" => {
                let s = val(&args, &mut i, "--hum", &mut bad);
                if let Some(s) = s {
                    match s.parse::<f32>() {
                        Ok(v) => p.hum_db = Some(v),
                        Err(_) => bad.push(format!("--hum needs a number, got {s}")),
                    }
                }
            }
            "--fundamental-scale" => {
                if let Some(v) = num(&args, &mut i, "--fundamental-scale", &mut bad) {
                    p.fundamental_scale = v as f32;
                }
            }
            "--drift" => {
                if let Some(v) = num(&args, &mut i, "--drift", &mut bad) {
                    p.drift_cents = v as f32;
                }
            }
            "--amp-db" => {
                if let Some(v) = num(&args, &mut i, "--amp-db", &mut bad) {
                    p.amp_db = v as f32;
                }
            }
            "--seed" => {
                if let Some(v) = num(&args, &mut i, "--seed", &mut bad) {
                    p.seed = v as u64;
                }
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return;
            }
            other => bad.push(format!("unknown option {other}")),
        }
        i += 1;
    }

    if !bad.is_empty() {
        for e in bad {
            eprintln!("error: {e}");
        }
        std::process::exit(2);
    }
    let Some(out) = out else {
        eprintln!("error: --out <path> is required\n{USAGE}");
        std::process::exit(2);
    };

    let data = render(&p);
    if let Err(e) = Wav::write_mono_pcm16(&out, p.fs, &data) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
    println!(
        "wrote {} · {} Hz · {} samples · {:.2} s · midi {:?} · harmonics {} · noise {:+.0} dBFS · hum {:?} · fundamental x{:.2} · drift {:+.0} ¢ · seed {}",
        out.display(),
        p.fs,
        data.len(),
        data.len() as f32 / p.fs as f32,
        p.midi,
        p.harmonics,
        p.noise_db,
        p.hum_db,
        p.fundamental_scale,
        p.drift_cents,
        p.seed,
    );
}
