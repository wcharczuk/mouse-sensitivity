//! Persistent configuration for the daemon.
//!
//! Settings are matched to devices by (vendor_id, product_id) so they survive
//! device index reordering across reconnects.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;

use crate::hid::PointerDevice;

/// Pointer settings for a single device. Mirrors LinearMouse's three controls:
/// the disable-acceleration toggle, the tracking-speed slider (0-40), and the
/// speed slider (0-1, mapped to HIDPointerResolution), plus the sensor DPI
/// that lets `match` translate tracking speed between different mice.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceSettings {
    pub disable_acceleration: bool,
    /// LinearMouse's "Tracking speed", 0–40. In linear mode this is the flat
    /// cursor-speed multiplier; with the curve on it's the curve steepness. It
    /// is written to the `HIDMouseAcceleration` property, which is why older
    /// configs call it `acceleration`; that name is still read.
    #[serde(alias = "acceleration")]
    pub tracking_speed: f64,
    pub speed: f64,
    /// Effective sensor resolution in counts per inch: cursor points per inch
    /// of hand movement at tracking speed 1.0. Measured by `calibrate` or set by
    /// hand with `set --dpi`. Absent until one of those has run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dpi: Option<f64>,
}

impl Default for DeviceSettings {
    fn default() -> Self {
        Self {
            disable_acceleration: true,
            tracking_speed: 0.6875,
            speed: 0.5,
            dpi: None,
        }
    }
}

/// Snapshot of what a device is doing right now. Used to seed a new config
/// entry so a partial `set` (or a calibration) records the values the user
/// hasn't touched instead of silently replacing them with defaults.
pub fn live_settings(dev: &PointerDevice) -> DeviceSettings {
    DeviceSettings {
        disable_acceleration: dev.get_linear_scaling() == Some(1),
        tracking_speed: dev.get_acceleration().unwrap_or(0.6875),
        speed: dev.get_speed().unwrap_or(0.5),
        dpi: None,
    }
}

pub const DPI_MIN: f64 = 1.0;
pub const DPI_MAX: f64 = 100_000.0;

/// Reject NaN/inf/non-positive DPI values and clamp the rest to a sane range.
pub fn sanitize_dpi(value: f64) -> Option<f64> {
    if value.is_finite() && value > 0.0 {
        Some(value.clamp(DPI_MIN, DPI_MAX))
    } else {
        None
    }
}

/// Tracking speed that makes a mouse with `dst_dpi` travel the same cursor
/// distance per centimetre of hand movement as a mouse with `src_dpi` at
/// `src_speed`. In linear mode cursor travel per inch is `dpi × tracking speed`,
/// so hold that product constant.
pub fn match_tracking_speed(src_speed: f64, src_dpi: f64, dst_dpi: f64) -> f64 {
    (src_speed * src_dpi / dst_dpi).clamp(0.0, 40.0)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct DeviceMatch {
    pub vendor_id: Option<i64>,
    pub product_id: Option<i64>,
}

impl DeviceMatch {
    pub fn matches(&self, vendor_id: Option<i64>, product_id: Option<i64>) -> bool {
        self.vendor_id == vendor_id && self.product_id == product_id
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    #[serde(rename = "match")]
    pub matcher: DeviceMatch,
    pub name: String,
    pub settings: DeviceSettings,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub devices: Vec<DeviceConfig>,
    #[serde(default)]
    pub default: Option<DeviceSettings>,
}

impl Config {
    pub fn settings_for(
        &self,
        vendor_id: Option<i64>,
        product_id: Option<i64>,
    ) -> Option<&DeviceSettings> {
        self.devices
            .iter()
            .find(|d| d.matcher.matches(vendor_id, product_id))
            .map(|d| &d.settings)
            .or(self.default.as_ref())
    }

    /// Config entries whose name contains `needle`, case-insensitively.
    pub fn find_by_name(&self, needle: &str) -> Vec<&DeviceConfig> {
        let needle = needle.to_lowercase();
        self.devices
            .iter()
            .filter(|d| d.name.to_lowercase().contains(&needle))
            .collect()
    }

    /// Update the entry for a device, creating it if needed. A new entry starts
    /// from the config's `default` if there is one, otherwise from `seed`
    /// (normally the device's live values), and then has `update` applied.
    pub fn upsert(
        &mut self,
        vendor_id: Option<i64>,
        product_id: Option<i64>,
        name: &str,
        seed: impl FnOnce() -> DeviceSettings,
        update: impl FnOnce(&mut DeviceSettings),
    ) {
        if let Some(d) = self
            .devices
            .iter_mut()
            .find(|d| d.matcher.matches(vendor_id, product_id))
        {
            update(&mut d.settings);
            d.name = name.to_string();
            return;
        }
        let mut settings = self.default.clone().unwrap_or_else(seed);
        update(&mut settings);
        self.devices.push(DeviceConfig {
            matcher: DeviceMatch { vendor_id, product_id },
            name: name.to_string(),
            settings,
        });
    }
}

pub fn support_dir() -> PathBuf {
    dirs::home_dir()
        .expect("home directory")
        .join("Library/Application Support/mouse-sensitivity")
}

pub fn config_path() -> PathBuf {
    support_dir().join("config.json")
}

pub fn socket_path() -> PathBuf {
    support_dir().join("daemon.sock")
}

pub fn load() -> io::Result<Config> {
    let path = config_path();
    match fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e),
    }
}

pub fn save(config: &Config) -> io::Result<()> {
    let dir = support_dir();
    fs::create_dir_all(&dir)?;
    let path = config_path();
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(config)?)?;
    fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, vid: i64, pid: i64, settings: DeviceSettings) -> DeviceConfig {
        DeviceConfig {
            matcher: DeviceMatch { vendor_id: Some(vid), product_id: Some(pid) },
            name: name.to_string(),
            settings,
        }
    }

    #[test]
    fn match_tracking_speed_holds_cursor_travel_constant() {
        // 0.25 on an 1800 DPI sensor should feel like 0.45 on a 1000 DPI one.
        let a = match_tracking_speed(0.25, 1800.0, 1000.0);
        assert!((a - 0.45).abs() < 1e-9, "{a}");
        assert!((0.25 * 1800.0 - a * 1000.0).abs() < 1e-6);
        // Same sensor, same value.
        assert!((match_tracking_speed(0.6875, 1000.0, 1000.0) - 0.6875).abs() < 1e-12);
    }

    #[test]
    fn match_tracking_speed_clamps_to_hid_range() {
        assert_eq!(match_tracking_speed(30.0, 8000.0, 100.0), 40.0);
        assert_eq!(match_tracking_speed(0.0, 8000.0, 100.0), 0.0);
    }

    #[test]
    fn sanitize_dpi_rejects_garbage() {
        assert_eq!(sanitize_dpi(f64::NAN), None);
        assert_eq!(sanitize_dpi(f64::INFINITY), None);
        assert_eq!(sanitize_dpi(-5.0), None);
        assert_eq!(sanitize_dpi(0.0), None);
        assert_eq!(sanitize_dpi(1e9), Some(DPI_MAX));
        assert_eq!(sanitize_dpi(1000.0), Some(1000.0));
    }

    #[test]
    fn config_without_dpi_still_parses_and_omits_it_when_saved() {
        let raw = r#"{"devices":[{"match":{"vendor_id":5426,"product_id":152},
            "name":"Razer DeathAdder Essential",
            "settings":{"disable_acceleration":true,"acceleration":0.25,"speed":0.069}}],
            "default":null}"#;
        let cfg: Config = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.devices[0].settings.dpi, None);
        // Older configs called tracking speed "acceleration": still read, written back renamed.
        assert_eq!(cfg.devices[0].settings.tracking_speed, 0.25);
        let out = serde_json::to_string(&cfg).unwrap();
        assert!(!out.contains("dpi"), "{out}");
        assert!(out.contains(r#""tracking_speed":0.25"#), "{out}");
        assert!(!out.contains(r#""acceleration""#), "{out}");

        let mut cfg = cfg;
        cfg.devices[0].settings.dpi = Some(1800.0);
        let out = serde_json::to_string(&cfg).unwrap();
        assert!(out.contains(r#""dpi":1800.0"#), "{out}");
    }

    #[test]
    fn find_by_name_is_case_insensitive_substring() {
        let cfg = Config {
            devices: vec![
                entry("Razer DeathAdder Essential", 1, 1, DeviceSettings::default()),
                entry("MX Master 3S", 2, 2, DeviceSettings::default()),
            ],
            default: None,
        };
        assert_eq!(cfg.find_by_name("razer").len(), 1);
        assert_eq!(cfg.find_by_name("MX MASTER")[0].name, "MX Master 3S");
        assert_eq!(cfg.find_by_name("e").len(), 2);
        assert!(cfg.find_by_name("logitech").is_empty());
    }

    #[test]
    fn upsert_seeds_new_entries_from_seed_not_defaults() {
        let mut cfg = Config::default();
        let seed = DeviceSettings { disable_acceleration: true, tracking_speed: 0.5, speed: 0.069, dpi: None };
        cfg.upsert(Some(2), Some(2), "MX Master 3S", || seed.clone(), |s| s.dpi = Some(1000.0));
        let s = &cfg.devices[0].settings;
        assert_eq!(s.tracking_speed, 0.5);
        assert_eq!(s.speed, 0.069);
        assert_eq!(s.dpi, Some(1000.0));

        // An explicit config default still wins over the seed.
        let mut cfg = Config { devices: vec![], default: Some(DeviceSettings::default()) };
        cfg.upsert(Some(2), Some(2), "MX Master 3S", || seed.clone(), |_| {});
        assert_eq!(cfg.devices[0].settings.tracking_speed, 0.6875);

        // Existing entries are updated in place and renamed.
        cfg.upsert(Some(2), Some(2), "MX Master 3S (renamed)", || seed.clone(), |s| s.tracking_speed = 0.3);
        assert_eq!(cfg.devices.len(), 1);
        assert_eq!(cfg.devices[0].name, "MX Master 3S (renamed)");
        assert_eq!(cfg.devices[0].settings.tracking_speed, 0.3);
    }
}
