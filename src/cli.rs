//! Hand-rolled flags: no argument-parsing dependency. `main.rs` folds them
//! into `AppConfig`, then either runs `analyse_wav` offline, prints a headless
//! CSV, or hands the merged config to the GUI.

use std::path::{Path, PathBuf};

use crate::audio::device;
use crate::audio::wav::Wav;
use crate::dsp::biquad::HighPassBank;
use crate::dsp::frame::{Analyzer, HOP};
use crate::dsp::track::Reading;
use crate::model::config::AppConfig;
use crate::model::notes::Spelling;
use crate::model::presets::preset_by_id;

pub const CSV_HEADER: &str = "t_ms,state,f_hz,cents,clarity,target,rms_db,gate_db";

pub const USAGE: &str = "\
tuna — guitar/bass tuner

  tuna                       start the GUI on the default input
  --list-devices             print capture devices and exit
  --list-presets             print tuning ids and exit
  --device <substr>          device id or case-insensitive name fragment
  --channel <n>              0-based input channel
  --preset <id>              see --list-presets
  --spelling sharp|flat      accidental spelling (display only)
  --a4 <hz>                  reference pitch, 435..455
  --wav <path>               analyse a file instead of the live input
  --csv                      print analysis rows to stdout (headless)
  --seconds <n>              live capture duration for --csv
  --help                     this text
";

#[derive(Clone, Debug, Default)]
pub struct Flags {
    pub list_devices: bool,
    pub list_presets: bool,
    pub device: Option<String>,
    pub channel: Option<usize>,
    pub preset: Option<String>,
    pub spelling: Option<Spelling>,
    pub a4: Option<f32>,
    pub wav: Option<PathBuf>,
    pub csv: bool,
    pub seconds: Option<f32>,
    pub help: bool,
    pub errors: Vec<String>,
}

/// Takes the value of an option: either the inline `--k=v` text or the next
/// argument. A missing value is recorded, never guessed.
fn value(
    args: &[String],
    i: &mut usize,
    key: &str,
    inline: Option<String>,
    errors: &mut Vec<String>,
) -> Option<String> {
    if let Some(v) = inline {
        return Some(v);
    }
    if *i < args.len() {
        let v = args[*i].clone();
        *i += 1;
        Some(v)
    } else {
        errors.push(format!("{key} needs a value"));
        None
    }
}

/// `--flag value` and `--flag=value` both work.
pub fn parse(args: &[String]) -> Flags {
    let mut f = Flags::default();
    let mut i = 0;
    while i < args.len() {
        let (key, mut inline) = match args[i].split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (args[i].clone(), None),
        };
        i += 1;
        let key = key.as_str();
        match key {
            "--list-devices" => f.list_devices = true,
            "--list-presets" => f.list_presets = true,
            "--csv" => f.csv = true,
            "--help" | "-h" => f.help = true,
            "--device" => f.device = value(args, &mut i, key, inline.take(), &mut f.errors),
            "--preset" => f.preset = value(args, &mut i, key, inline.take(), &mut f.errors),
            "--wav" => {
                f.wav = value(args, &mut i, key, inline.take(), &mut f.errors).map(PathBuf::from)
            }
            "--channel" => {
                let v = value(args, &mut i, key, inline.take(), &mut f.errors);
                if let Some(v) = v {
                    match v.parse::<usize>() {
                        Ok(n) => f.channel = Some(n),
                        Err(_) => f.errors.push(format!("--channel needs a number, got {v}")),
                    }
                }
            }
            "--spelling" => {
                let v = value(args, &mut i, key, inline.take(), &mut f.errors);
                if let Some(v) = v {
                    match v.to_lowercase().as_str() {
                        "sharp" | "#" => f.spelling = Some(Spelling::Sharp),
                        "flat" | "b" => f.spelling = Some(Spelling::Flat),
                        _ => f
                            .errors
                            .push(format!("--spelling must be sharp or flat, got {v}")),
                    }
                }
            }
            "--a4" => {
                let v = value(args, &mut i, key, inline.take(), &mut f.errors);
                if let Some(v) = v {
                    match v.parse::<f32>() {
                        Ok(n) => f.a4 = Some(n),
                        Err(_) => f.errors.push(format!("--a4 needs a number, got {v}")),
                    }
                }
            }
            "--seconds" => {
                let v = value(args, &mut i, key, inline.take(), &mut f.errors);
                if let Some(v) = v {
                    match v.parse::<f32>() {
                        Ok(n) => f.seconds = Some(n),
                        Err(_) => f.errors.push(format!("--seconds needs a number, got {v}")),
                    }
                }
            }
            other => f.errors.push(format!("unknown option {other}")),
        }
    }
    f
}

impl Flags {
    /// Command line wins over the saved config; the merged result is what both
    /// the live path and the `--wav` path use, so they cannot diverge.
    pub fn apply(&self, cfg: &mut AppConfig) {
        if let Some(d) = &self.device {
            cfg.device_id = d.clone();
        }
        if let Some(c) = self.channel {
            cfg.input_channel = c;
        }
        if let Some(p) = &self.preset {
            cfg.preset = p.clone();
            // A preset suggests a spelling, exactly as picking it in the UI
            // does. An explicit `--spelling` still wins: it is applied below.
            if let Some(p) = preset_by_id(p) {
                cfg.spelling = p.spelling;
            }
        }
        if let Some(s) = self.spelling {
            cfg.spelling = s;
        }
        if let Some(a) = self.a4 {
            cfg.a4_hz = a;
        }
        cfg.sanitize();
    }
}

#[derive(Clone, Debug)]
pub struct Offline {
    pub fs: u32,
    pub channels: u16,
    pub format: String,
    pub duration_s: f32,
    /// `(gate_db, floor_db, open)` as the file's last frame left it.
    pub gate: (f32, f32, bool),
    pub readings: Vec<Reading>,
}

/// Whole-file analysis: same `Analyzer`, same gate, same tracker as live.
pub fn analyse_wav(path: &Path, cfg: &AppConfig, tuning: &[u8]) -> Result<Offline, String> {
    let w = Wav::read(path)?;
    let ch = w.channels as usize;
    let sel = cfg.input_channel.min(ch - 1);
    let fs = w.fs;
    let mut an = Analyzer::new(fs as f32);
    an.set_a4(cfg.a4_hz);
    an.set_tuning(tuning);
    an.set_pinned(cfg.pinned_string);
    an.set_smoothing(cfg.smoothing.tau_ms());
    let mut hp = HighPassBank::new(fs as f32, 1);
    let mut buf = vec![0.0f32; HOP];
    let mut readings = Vec::new();
    let deint: Vec<f32> = w.data.chunks(ch).map(|frame| frame[sel]).collect();
    for block in deint.chunks(HOP) {
        if block.len() != HOP {
            break;
        }
        for i in 0..HOP {
            buf[i] = hp.process(0, block[i]);
        }
        if let Some(r) = an.process_hop(&buf) {
            readings.push(r);
        }
    }
    Ok(Offline {
        fs,
        channels: w.channels,
        format: w.format.to_string(),
        duration_s: deint.len() as f32 / fs as f32,
        gate: an.gate_state(),
        readings,
    })
}

pub fn csv_row(r: &Reading) -> String {
    format!(
        "{},{},{:.3},{:.2},{:.3},{},{:.1},{:.1}",
        r.t_ms,
        r.state.name(),
        r.f_hz,
        r.cents,
        r.clarity,
        r.target_midi.map(|m| m.to_string()).unwrap_or_default(),
        r.rms_db,
        r.gate_db
    )
}

/// `--csv` without `--wav`: headless live capture, no window.
pub fn run_live_csv(cfg: &AppConfig, tuning: &[u8], seconds: f32) -> Result<(), String> {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crate::audio::capture::{sample_format_name, Capture, Shared, StartParams};

    let (dev, scfg, label) = device::open(&cfg.device_id)?;
    let shared = Arc::new(Shared::default());
    *shared
        .device_label
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = label.clone();
    let p = StartParams {
        channel: cfg.input_channel,
        a4: cfg.a4_hz,
        tuning: tuning.to_vec(),
        pinned: cfg.pinned_string,
        smoothing_ms: cfg.smoothing.tau_ms(),
        ctx: None,
    };
    let mut cap = Capture::start(&shared, &dev, &scfg, &p)?;
    // The headless run has no on-screen log, so the format WASAPI actually
    // granted goes to stderr; stdout stays a clean CSV.
    eprintln!(
        "opened {label} {} Hz {} ch {}",
        scfg.sample_rate(),
        scfg.channels(),
        sample_format_name(scfg.sample_format())
    );
    let limit = Duration::from_secs_f32(seconds.max(0.5));
    let start = Instant::now();
    let mut last_t = 0u64;
    while start.elapsed() < limit {
        if let Some(r) = shared.latest() {
            if r.t_ms > last_t {
                last_t = r.t_ms;
                println!("{}", csv_row(&r));
            }
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    let status = shared.status();
    cap.stop();
    if let crate::audio::capture::AudioStatus::Failed(e) = status {
        return Err(format!("capture failed: {e}"));
    }
    Ok(())
}

pub fn list_devices_text() -> String {
    let mut diag = Vec::new();
    let inv = device::inventory(&mut diag);
    let mut out = String::new();
    out.push_str(&format!(
        "{:<46} {:>3} {:>7}  {:<22} {}\n",
        "DEVICE", "CH", "RATE", "FORMAT", "ID / NOTE"
    ));
    for d in &inv.devices {
        let flag = if d.is_default { "default" } else { "" };
        let note = match &d.error {
            Some(e) => format!("UNUSABLE: {e}"),
            None => d.id.clone(),
        };
        out.push_str(&format!(
            "{:<46} {:>3} {:>7}  {:<22} {}{note}\n",
            d.name,
            d.channels,
            if d.fs == 0 {
                "-".to_string()
            } else {
                d.fs.to_string()
            },
            d.format,
            if flag.is_empty() {
                String::new()
            } else {
                format!("[{flag}] ")
            }
        ));
    }
    if inv.devices.is_empty() {
        out.push_str("no capture devices found\n");
    }
    for d in diag {
        out.push_str(&format!("note: {d}\n"));
    }
    out
}

pub fn list_presets_text() -> String {
    let mut out = String::new();
    for p in crate::model::presets::PRESETS {
        out.push_str(&format!("{:<18} {}\n", p.id, p.label));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn flags_parse_both_forms_and_report_bad_values() {
        let f = parse(&v(&[
            "--device",
            "Focusrite",
            "--channel=1",
            "--spelling",
            "flat",
            "--a4",
            "442",
            "--csv",
            "--seconds",
            "2",
        ]));
        assert!(f.errors.is_empty(), "{:?}", f.errors);
        assert_eq!(f.device.as_deref(), Some("Focusrite"));
        assert_eq!(f.channel, Some(1));
        assert_eq!(f.spelling, Some(Spelling::Flat));
        assert_eq!(f.a4, Some(442.0));
        assert!(f.csv);
        assert_eq!(f.seconds, Some(2.0));

        let bad = parse(&v(&["--channel", "left", "--bogus"]));
        assert_eq!(bad.errors.len(), 2, "{:?}", bad.errors);
    }

    #[test]
    fn flags_override_config_without_touching_other_fields() {
        let mut cfg = AppConfig {
            spelling: Spelling::Flat,
            tolerance_cents: 5.0,
            ..Default::default()
        };
        let f = parse(&v(&["--preset", "guitar_8_f", "--a4", "438"]));
        f.apply(&mut cfg);
        assert_eq!(cfg.preset, "guitar_8_f");
        assert_eq!(cfg.a4_hz, 438.0);
        assert_eq!(cfg.spelling, Spelling::Flat);
        assert_eq!(cfg.tolerance_cents, 5.0);
    }

    #[test]
    fn a_preset_suggests_its_spelling_but_the_flag_wins() {
        let mut cfg = AppConfig {
            spelling: Spelling::Sharp,
            ..Default::default()
        };
        parse(&v(&["--preset", "guitar_8_f"])).apply(&mut cfg);
        assert_eq!(cfg.preset, "guitar_8_f");
        assert_eq!(
            cfg.spelling,
            Spelling::Flat,
            "flat-key tuning reads in flats"
        );

        let mut cfg = AppConfig {
            spelling: Spelling::Flat,
            ..Default::default()
        };
        parse(&v(&["--preset", "bass_5"])).apply(&mut cfg);
        assert_eq!(cfg.spelling, Spelling::Sharp);

        let mut cfg = AppConfig {
            spelling: Spelling::Flat,
            ..Default::default()
        };
        parse(&v(&["--preset", "guitar_8_f", "--spelling", "sharp"])).apply(&mut cfg);
        assert_eq!(cfg.spelling, Spelling::Sharp);
    }
}
