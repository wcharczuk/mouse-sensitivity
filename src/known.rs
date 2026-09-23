//! Factory DPI values for mice the tool can identify by vendor and product id.
//!
//! `calibrate` measures a mouse's effective DPI and `dpi` asks the mouse for
//! it, but both need the mouse in hand, and `dpi` needs the Input Monitoring
//! permission. Every mouse ships at a documented DPI, though, and most people
//! never change it, so the factory figure is a usable stand-in: `match` only
//! cares about the ratio between two mice, and a factory default is exact
//! whenever the DPI stage was left alone.
//!
//! Entries are keyed by the (vendor id, product id) pair macOS reports. A
//! Logitech mouse behind a Unifying or Bolt receiver reports the receiver's
//! ids and a generic name, so only direct Bluetooth and USB connections are
//! listed. Family rules catch models missing from the table: Razer ships
//! every mouse with onboard memory at 1800 DPI (Synapse's default stage) and
//! Logitech's MX line at 1000.

use crate::vendor::{VENDOR_LOGITECH, VENDOR_RAZER};

pub const VENDOR_APPLE: i64 = 0x05AC;

/// One model, identified by its product id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    pub vendor_id: i64,
    pub product_id: i64,
    pub name: &'static str,
    pub default_dpi: u32,
}

/// A rule for a whole family: any mouse from `vendor_id` whose product name
/// contains `name_contains` (case-insensitively; empty matches everything).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Family {
    pub vendor_id: i64,
    pub name_contains: &'static str,
    /// Noun phrase for messages, e.g. "Razer's factory default for every mouse".
    pub description: &'static str,
    pub default_dpi: u32,
}

const fn model(vendor_id: i64, product_id: i64, name: &'static str, default_dpi: u32) -> Model {
    Model { vendor_id, product_id, name, default_dpi }
}

const MODELS: &[Model] = &[
    // Razer: 1800 DPI out of the box across the range. Product ids as in openrazer.
    model(VENDOR_RAZER, 0x0016, "Razer DeathAdder 3.5G", 1800),
    model(VENDOR_RAZER, 0x0037, "Razer DeathAdder 2013", 1800),
    model(VENDOR_RAZER, 0x0038, "Razer DeathAdder 1800", 1800),
    model(VENDOR_RAZER, 0x0043, "Razer DeathAdder Chroma", 1800),
    model(VENDOR_RAZER, 0x005C, "Razer DeathAdder Elite", 1800),
    model(VENDOR_RAZER, 0x006E, "Razer DeathAdder Essential", 1800),
    model(VENDOR_RAZER, 0x0071, "Razer DeathAdder Essential (White Edition)", 1800),
    model(VENDOR_RAZER, 0x0098, "Razer DeathAdder Essential (2021)", 1800),
    model(VENDOR_RAZER, 0x0084, "Razer DeathAdder V2", 1800),
    model(VENDOR_RAZER, 0x007C, "Razer DeathAdder V2 Pro (wired)", 1800),
    model(VENDOR_RAZER, 0x007D, "Razer DeathAdder V2 Pro (wireless)", 1800),
    model(VENDOR_RAZER, 0x008C, "Razer DeathAdder V2 Mini", 1800),
    model(VENDOR_RAZER, 0x009C, "Razer DeathAdder V2 X HyperSpeed", 1800),
    model(VENDOR_RAZER, 0x00B2, "Razer DeathAdder V3", 1800),
    model(VENDOR_RAZER, 0x00B6, "Razer DeathAdder V3 Pro (wired)", 1800),
    model(VENDOR_RAZER, 0x00B7, "Razer DeathAdder V3 Pro (wireless)", 1800),
    model(VENDOR_RAZER, 0x0078, "Razer Viper", 1800),
    model(VENDOR_RAZER, 0x008A, "Razer Viper Mini", 1800),
    model(VENDOR_RAZER, 0x007A, "Razer Viper Ultimate (wired)", 1800),
    model(VENDOR_RAZER, 0x007B, "Razer Viper Ultimate (wireless)", 1800),
    model(VENDOR_RAZER, 0x0091, "Razer Viper 8KHz", 1800),
    model(VENDOR_RAZER, 0x00A5, "Razer Viper V2 Pro (wired)", 1800),
    model(VENDOR_RAZER, 0x00A6, "Razer Viper V2 Pro (wireless)", 1800),
    model(VENDOR_RAZER, 0x0064, "Razer Basilisk", 1800),
    model(VENDOR_RAZER, 0x0083, "Razer Basilisk X HyperSpeed", 1800),
    model(VENDOR_RAZER, 0x0085, "Razer Basilisk V2", 1800),
    model(VENDOR_RAZER, 0x0099, "Razer Basilisk V3", 1800),
    model(VENDOR_RAZER, 0x0067, "Razer Naga Trinity", 1800),
    model(VENDOR_RAZER, 0x008F, "Razer Naga Pro (wired)", 1800),
    model(VENDOR_RAZER, 0x0090, "Razer Naga Pro (wireless)", 1800),
    model(VENDOR_RAZER, 0x0096, "Razer Naga X", 1800),
    model(VENDOR_RAZER, 0x0094, "Razer Orochi V2 (receiver)", 1800),
    model(VENDOR_RAZER, 0x0095, "Razer Orochi V2 (Bluetooth)", 1800),
    // Logitech MX line: 1000 DPI out of the box. Bluetooth product ids.
    model(VENDOR_LOGITECH, 0xB012, "Logitech MX Master", 1000),
    model(VENDOR_LOGITECH, 0xB019, "Logitech MX Master 2S", 1000),
    model(VENDOR_LOGITECH, 0xB023, "Logitech MX Master 3", 1000),
    model(VENDOR_LOGITECH, 0xB034, "Logitech MX Master 3S", 1000),
    model(VENDOR_LOGITECH, 0xB013, "Logitech MX Anywhere 2", 1000),
    model(VENDOR_LOGITECH, 0xB01A, "Logitech MX Anywhere 2S", 1000),
    model(VENDOR_LOGITECH, 0xB025, "Logitech MX Anywhere 3", 1000),
    model(VENDOR_LOGITECH, 0xB037, "Logitech MX Anywhere 3S", 1000),
    model(VENDOR_LOGITECH, 0xB020, "Logitech MX Vertical", 1000),
    model(VENDOR_LOGITECH, 0xB015, "Logitech M720 Triathlon", 1000),
    // Apple: the Magic Mouse's laser sensor is specified at 1300 DPI.
    model(VENDOR_APPLE, 0x030D, "Apple Magic Mouse", 1300),
    model(VENDOR_APPLE, 0x0269, "Apple Magic Mouse 2", 1300),
];

const FAMILIES: &[Family] = &[
    Family {
        vendor_id: VENDOR_RAZER,
        name_contains: "",
        description: "Razer's factory default for every mouse",
        default_dpi: 1800,
    },
    Family {
        vendor_id: VENDOR_LOGITECH,
        name_contains: "MX Master",
        description: "Logitech's factory default for the MX Master line",
        default_dpi: 1000,
    },
    Family {
        vendor_id: VENDOR_LOGITECH,
        name_contains: "MX Anywhere",
        description: "Logitech's factory default for the MX Anywhere line",
        default_dpi: 1000,
    },
    Family {
        vendor_id: VENDOR_LOGITECH,
        name_contains: "MX Vertical",
        description: "Logitech's factory default for the MX Vertical",
        default_dpi: 1000,
    },
    Family {
        vendor_id: VENDOR_APPLE,
        name_contains: "Magic Mouse",
        description: "Apple's specified resolution for the Magic Mouse",
        default_dpi: 1300,
    },
];

/// What a lookup matched on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    Model(&'static Model),
    Family(&'static Family),
}

/// A factory DPI figure and the table entry it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownDpi {
    pub dpi: u32,
    pub basis: Basis,
}

impl KnownDpi {
    /// Where the figure comes from, as a noun phrase: "the factory default of
    /// the Razer DeathAdder Essential (2021)" or "Razer's factory default for
    /// every mouse".
    pub fn describe(&self) -> String {
        match self.basis {
            Basis::Model(m) => format!("the factory default of the {}", m.name),
            Basis::Family(f) => f.description.to_string(),
        }
    }
}

/// The factory DPI for a mouse, by product id first and family rule second.
/// `name` is the HID product string, used only by family rules.
pub fn lookup(vendor_id: Option<i64>, product_id: Option<i64>, name: &str) -> Option<KnownDpi> {
    let vendor_id = vendor_id?;
    if let Some(pid) = product_id {
        if let Some(m) = MODELS.iter().find(|m| m.vendor_id == vendor_id && m.product_id == pid) {
            return Some(KnownDpi { dpi: m.default_dpi, basis: Basis::Model(m) });
        }
    }
    let lower = name.to_lowercase();
    FAMILIES
        .iter()
        .filter(|f| f.vendor_id == vendor_id && lower.contains(&f.name_contains.to_lowercase()))
        // The most specific rule wins when several apply.
        .max_by_key(|f| f.name_contains.len())
        .map(|f| KnownDpi { dpi: f.default_dpi, basis: Basis::Family(f) })
}

pub fn models() -> &'static [Model] {
    MODELS
}

pub fn families() -> &'static [Family] {
    FAMILIES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_id_beats_family_rule() {
        let k = lookup(Some(VENDOR_RAZER), Some(0x0098), "Razer DeathAdder Essential").unwrap();
        assert_eq!(k.dpi, 1800);
        assert!(matches!(k.basis, Basis::Model(m) if m.name == "Razer DeathAdder Essential (2021)"));
        assert_eq!(k.describe(), "the factory default of the Razer DeathAdder Essential (2021)");

        let k = lookup(Some(VENDOR_LOGITECH), Some(0xB034), "MX Master 3S").unwrap();
        assert_eq!(k.dpi, 1000);
        assert!(matches!(k.basis, Basis::Model(m) if m.name == "Logitech MX Master 3S"));
    }

    #[test]
    fn family_rules_cover_unlisted_models() {
        // Any Razer mouse, whatever it's called.
        let k = lookup(Some(VENDOR_RAZER), Some(0x7FFF), "Razer Something New").unwrap();
        assert_eq!(k.dpi, 1800);
        assert!(matches!(k.basis, Basis::Family(_)));
        assert_eq!(k.describe(), "Razer's factory default for every mouse");
        // Logitech only by name, case-insensitively.
        let k = lookup(Some(VENDOR_LOGITECH), Some(0x7FFF), "mx master 3s for mac").unwrap();
        assert_eq!(k.dpi, 1000);
        assert!(matches!(k.basis, Basis::Family(f) if f.name_contains == "MX Master"));
        assert_eq!(lookup(Some(VENDOR_LOGITECH), Some(0x7FFF), "G Pro X Superlight"), None);
        // A receiver reports its own id and a generic name.
        assert_eq!(lookup(Some(VENDOR_LOGITECH), Some(0xC548), "USB Receiver"), None);
    }

    #[test]
    fn unknown_vendor_or_missing_ids() {
        assert_eq!(lookup(None, Some(0x0098), "Razer DeathAdder Essential"), None);
        assert_eq!(lookup(Some(0x1234), Some(0x0098), "Razer DeathAdder Essential"), None);
        // No product id still gets the family rule.
        assert_eq!(lookup(Some(VENDOR_RAZER), None, "Razer Viper").unwrap().dpi, 1800);
    }

    #[test]
    fn table_is_well_formed() {
        for (i, a) in MODELS.iter().enumerate() {
            assert!(a.default_dpi > 0, "{}", a.name);
            assert!(!a.name.is_empty());
            for b in &MODELS[i + 1..] {
                assert!(
                    !(a.vendor_id == b.vendor_id && a.product_id == b.product_id),
                    "duplicate id {:04X}:{:04X} ({} / {})",
                    a.vendor_id,
                    a.product_id,
                    a.name,
                    b.name
                );
            }
        }
        for f in FAMILIES {
            assert!(f.default_dpi > 0, "{}", f.description);
            assert!(!f.description.is_empty());
        }
    }
}
