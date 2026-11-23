//! `%APPDATA%\tuna\config.toml` — load/save with a debounce owned by the app.

use std::path::PathBuf;

use crate::model::notes::{Spelling, A4_DEFAULT, MIDI_MAX, MIDI_MIN};

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Smoothing {
    Fast,
    Normal,
    Slow,
}

impl Smoothing {
    pub const ALL: [Smoothing; 3] = [Smoothing::Fast, Smoothing::Normal, Smoothing::Slow];

    /// EMA time constant for the displayed cents.
    pub fn tau_ms(self) -> f32 {
        match self {
            Smoothing::Fast => 60.0,
            Smoothing::Normal => 140.0,
            Smoothing::Slow => 300.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Smoothing::Fast => "Fast",
            Smoothing::Normal => "Normal",
            Smoothing::Slow => "Slow",
        }
    }
}

/// The three widths the UI offers for the in-tune dead band.
pub const TOLERANCES: [f32; 3] = [1.0, 3.0, 5.0];

#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// cpal `DeviceId` string; "" = system default input.
    pub device_id: String,
    /// 0-based index into the live stream's channels.
    pub input_channel: usize,
    /// Preset id, or "custom" / "chromatic".
    pub preset: String,
    /// MIDI numbers low → high, used when `preset == "custom"`.
    pub custom_tuning: Vec<u8>,
    /// Index into the current tuning; None = follow the measured note.
    pub pinned_string: Option<usize>,
    pub a4_hz: f32,
    pub spelling: Spelling,
    pub tolerance_cents: f32,
    pub smoothing: Smoothing,
    pub auto_gate: bool,
    pub gate_floor_db: f32,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            device_id: String::new(),
            input_channel: 0,
            preset: "guitar_standard".to_string(),
            custom_tuning: vec![40, 45, 50, 55, 59, 64],
            pinned_string: None,
            a4_hz: A4_DEFAULT,
            spelling: Spelling::Sharp,
            tolerance_cents: 3.0,
            smoothing: Smoothing::Normal,
            auto_gate: true,
            gate_floor_db: -60.0,
        }
    }
}

impl AppConfig {
    pub fn sanitize(&mut self) {
        for m in self.custom_tuning.iter_mut() {
            *m = (*m).clamp(MIDI_MIN, MIDI_MAX);
        }
        self.custom_tuning.sort_unstable();
        self.a4_hz = self.a4_hz.clamp(435.0, 455.0);
        self.gate_floor_db = self.gate_floor_db.clamp(-120.0, 0.0);
        self.tolerance_cents = self.tolerance_cents.clamp(1.0, 50.0);
        if crate::model::presets::preset_by_id(&self.preset).is_none() {
            self.preset = "chromatic".to_string();
        }
    }

    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("tuna").join("config.toml"))
    }

    /// Never fails startup: a missing or broken file yields defaults, and a
    /// broken file is kept aside as `config.toml.bak` with the reason returned
    /// for the diagnostics list.
    pub fn load() -> (AppConfig, Option<String>) {
        let Some(path) = Self::path() else {
            return (
                AppConfig::default(),
                Some("no config directory available".into()),
            );
        };
        match std::fs::read_to_string(&path) {
            Err(e) if path.exists() => (
                AppConfig::default(),
                Some(format!("could not read {}: {e}", path.display())),
            ),
            Err(_) => (AppConfig::default(), None),
            Ok(text) => match toml::from_str::<AppConfig>(&text) {
                Ok(mut cfg) => {
                    cfg.sanitize();
                    (cfg, None)
                }
                Err(e) => {
                    let bak = path.with_extension("toml.bak");
                    let _ = std::fs::rename(&path, &bak);
                    (
                        AppConfig::default(),
                        Some(format!(
                            "bad config, reset to defaults (kept as {}): {e}",
                            bak.display()
                        )),
                    )
                }
            },
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path().ok_or("no config directory available")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        // Windows rename fails onto an existing file only for some open modes;
        // remove first so a re-save never fails.
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
        std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_and_fields_round_trip_in_lowercase_keys() {
        let cfg = AppConfig {
            spelling: Spelling::Flat,
            smoothing: Smoothing::Slow,
            ..AppConfig::default()
        };
        let text = toml::to_string(&cfg).unwrap();
        assert!(text.contains("spelling = \"flat\""), "{text}");
        assert!(text.contains("smoothing = \"slow\""), "{text}");
        let back: AppConfig = toml::from_str(&text).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn unknown_keys_and_missing_fields_still_load() {
        let cfg: AppConfig = toml::from_str("spelling = \"flat\"\nfuture_option = 7\n")
            .expect("unknown keys must not break an older file");
        assert_eq!(cfg.spelling, Spelling::Flat);
        assert_eq!(cfg.a4_hz, A4_DEFAULT);
    }

    #[test]
    fn sanitize_clamps_and_sorts() {
        let mut cfg = AppConfig {
            custom_tuning: vec![64, 5, 40, 200],
            a4_hz: 500.0,
            preset: "nope".into(),
            ..Default::default()
        };
        cfg.sanitize();
        assert_eq!(cfg.custom_tuning, vec![21, 40, 64, 108]);
        assert_eq!(cfg.a4_hz, 455.0);
        assert_eq!(cfg.preset, "chromatic");
    }
}
