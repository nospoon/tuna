//! WASAPI device enumeration through cpal. Every input device is listed,
//! including ones whose format query fails — those are marked with the cpal
//! error string so the UI can say why they are unusable instead of hiding them.

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, DeviceId, SupportedStreamConfig};

#[derive(Clone, Debug, PartialEq)]
pub struct DeviceInfo {
    /// cpal `DeviceId` in its `Display` form; persistable.
    pub id: String,
    pub name: String,
    pub channels: u16,
    pub fs: u32,
    pub format: String,
    pub error: Option<String>,
    pub is_default: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Inventory {
    pub default_id: Option<String>,
    pub devices: Vec<DeviceInfo>,
}

pub fn describe(dev: &Device) -> (String, Option<String>) {
    match dev.description() {
        Ok(d) => (d.name().to_string(), None),
        Err(e) => ("unknown device".to_string(), Some(format!("{e}"))),
    }
}

pub fn describe_name(dev: &Device) -> String {
    describe(dev).0
}

fn id_of(dev: &Device) -> String {
    match dev.id() {
        Ok(id) => id.to_string(),
        Err(e) => format!("!id-error:{e}"),
    }
}

/// Never fails: an enumeration error lands in `devices[0].error`-style rows or
/// in the returned diagnostics.
pub fn inventory(diagnostics: &mut Vec<String>) -> Inventory {
    let host = cpal::default_host();
    let mut inv = Inventory::default();
    let default = host.default_input_device();
    let default_id = default.as_ref().map(id_of);
    inv.default_id = default_id.clone();

    let devices: Vec<Device> = match host.input_devices() {
        Err(e) => {
            diagnostics.push(format!("device enumeration failed: {e}"));
            if let Some(d) = default {
                vec![d]
            } else {
                return inv;
            }
        }
        Ok(it) => it.collect(),
    };

    for dev in devices {
        let id = id_of(&dev);
        let (name, mut err) = describe(&dev);
        let (channels, fs, format) = match dev.default_input_config() {
            Err(e) => {
                let msg = format!("{name}: default input config failed: {e}");
                err = match err {
                    Some(prev) => Some(format!("{prev}; {e}")),
                    None => Some(e.to_string()),
                };
                diagnostics.push(msg);
                (0, 0, String::new())
            }
            Ok(c) => (
                c.channels(),
                c.sample_rate(),
                format!("{:?}", c.sample_format()),
            ),
        };
        let is_default = inv.default_id.as_deref() == Some(id.as_str());
        inv.devices.push(DeviceInfo {
            id,
            name,
            channels,
            fs,
            format,
            error: err,
            is_default,
        });
    }
    inv
}

/// Resolve a persisted device id (or a case-insensitive name substring for
/// `--device`); empty id means the system default.
pub fn open(id_or_substring: &str) -> Result<(Device, SupportedStreamConfig, String), String> {
    let host = cpal::default_host();
    let wanted = id_or_substring.trim();
    let mut names: Vec<String> = Vec::new();

    let dev: Option<Device> = if wanted.is_empty() {
        host.default_input_device()
    } else if let Ok(parsed) = wanted.parse::<DeviceId>() {
        host.device_by_id(&parsed)
    } else {
        let lower = wanted.to_lowercase();
        match host.input_devices() {
            Ok(it) => it.filter(|d| d.supports_input()).find(|d| {
                let n = describe(d).0.to_lowercase();
                names.push(describe(d).0.clone());
                n.contains(&lower) || id_of(d).to_lowercase().contains(&lower)
            }),
            Err(e) => return Err(format!("could not enumerate devices: {e}")),
        }
    };

    let Some(dev) = dev else {
        return Err(format!(
            "input device {:?} not found{}",
            wanted,
            if names.is_empty() {
                String::new()
            } else {
                format!(" (available: {})", names.join(", "))
            }
        ));
    };
    let label = describe(&dev).0;
    let cfg = dev
        .default_input_config()
        .map_err(|e| format!("{label}: no default input config: {e}"))?;
    Ok((dev, cfg, label))
}

/// cpal 0.18 needs an explicit `StreamConfig`; take channels and rate from the
/// device's own default and let WASAPI choose the buffer size.
pub fn stream_config(cfg: &SupportedStreamConfig) -> cpal::StreamConfig {
    cpal::StreamConfig {
        channels: cfg.channels(),
        sample_rate: cfg.sample_rate(),
        buffer_size: cpal::BufferSize::Default,
    }
}
