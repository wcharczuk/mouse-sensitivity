//! Persistent configuration for the daemon.
//!
//! Settings are matched to devices by (vendor_id, product_id) so they survive
//! device index reordering across reconnects.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;

/// Pointer settings for a single device. Mirrors LinearMouse's three controls:
/// the disable-acceleration toggle, the acceleration slider (0-20), and the
/// speed slider (0-1, mapped to HIDPointerResolution).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceSettings {
    pub disable_acceleration: bool,
    pub acceleration: f64,
    pub speed: f64,
}

impl Default for DeviceSettings {
    fn default() -> Self {
        Self {
            disable_acceleration: true,
            acceleration: 0.6875,
            speed: 0.5,
        }
    }
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

    pub fn upsert(
        &mut self,
        vendor_id: Option<i64>,
        product_id: Option<i64>,
        name: &str,
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
        let mut settings = self.default.clone().unwrap_or_default();
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
