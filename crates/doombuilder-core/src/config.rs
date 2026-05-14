// ABOUTME: Game-specific configuration: action specials, sector effects, thing
// ABOUTME: types, and flag bit names. Bundles vanilla Doom; the structure is
// ABOUTME: format-agnostic so Hexen/Boom/ZDoom can ship as additional TOML files.

use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct GameConfig {
    pub meta: Meta,
    #[serde(default)]
    pub linedef_specials: Vec<LinedefSpecial>,
    #[serde(default)]
    pub sector_specials: HashMap<String, String>,
    #[serde(default)]
    pub thing_types: Vec<ThingType>,
    #[serde(default)]
    pub linedef_flags: HashMap<String, String>,
    #[serde(default)]
    pub thing_flags: HashMap<String, String>,

    #[serde(skip)]
    linedef_lookup: HashMap<u16, usize>,
    #[serde(skip)]
    thing_lookup: HashMap<u16, usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Meta {
    pub name: String,
    pub format: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LinedefSpecial {
    pub id: u16,
    pub title: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub category: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ThingType {
    pub id: u16,
    pub title: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub sprite: String,
}

impl GameConfig {
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        let mut cfg: GameConfig = toml::from_str(text)?;
        cfg.linedef_lookup = cfg
            .linedef_specials
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id, i))
            .collect();
        cfg.thing_lookup = cfg
            .thing_types
            .iter()
            .enumerate()
            .map(|(i, t)| (t.id, i))
            .collect();
        Ok(cfg)
    }

    pub fn vanilla_doom() -> Self {
        Self::from_toml(VANILLA_DOOM_TOML).expect("bundled doom.toml is valid")
    }

    pub fn vanilla_doom2() -> Self {
        Self::from_toml(VANILLA_DOOM2_TOML).expect("bundled doom2.toml is valid")
    }

    pub fn heretic() -> Self {
        Self::from_toml(HERETIC_TOML).expect("bundled heretic.toml is valid")
    }

    pub fn hexen() -> Self {
        Self::from_toml(HEXEN_TOML).expect("bundled hexen.toml is valid")
    }

    /// Built-in named configs available without disk I/O.
    pub fn builtin(name: &str) -> Option<Self> {
        match name {
            "Doom" => Some(Self::vanilla_doom()),
            "Doom 2" => Some(Self::vanilla_doom2()),
            "Heretic" => Some(Self::heretic()),
            "Hexen" => Some(Self::hexen()),
            _ => None,
        }
    }

    pub fn builtin_names() -> &'static [&'static str] {
        &["Doom", "Doom 2", "Heretic", "Hexen"]
    }

    /// Best-effort guess at which built-in matches a WAD. Returns one of
    /// `builtin_names()`. Detection priority:
    ///   1. Any `BEHAVIOR` lump in the directory ⇒ Hexen (binary Hexen-format
    ///      maps carry an ACS bytecode lump per map).
    ///   2. Any `ExMy`-style map marker ⇒ Doom (episode-style). We don't
    ///      try to distinguish Heretic here: Heretic PWADs are rare and
    ///      the user can switch the dropdown manually if needed.
    ///   3. Any `MAPxx` marker ⇒ Doom 2.
    ///   4. Fallback ⇒ Doom 2 (most modern PWADs).
    pub fn detect_for_wad(wad: &crate::wad::Wad) -> &'static str {
        let dir = wad.directory();
        if dir
            .iter()
            .any(|e| e.name_str().eq_ignore_ascii_case("BEHAVIOR"))
        {
            return "Hexen";
        }
        let markers = wad.map_markers();
        let has_em = markers.iter().any(|n| {
            let b = n.as_bytes();
            b.len() == 4
                && (b[0] == b'E' || b[0] == b'e')
                && (b[2] == b'M' || b[2] == b'm')
                && b[1].is_ascii_digit()
                && b[3].is_ascii_digit()
        });
        if has_em {
            return "Doom";
        }
        let has_mapxx = markers.iter().any(|n| {
            let upper = n.to_ascii_uppercase();
            upper.starts_with("MAP")
        });
        if has_mapxx {
            return "Doom 2";
        }
        "Doom 2"
    }

    pub fn linedef_special(&self, id: u16) -> Option<&LinedefSpecial> {
        self.linedef_lookup
            .get(&id)
            .and_then(|i| self.linedef_specials.get(*i))
    }

    pub fn sector_special(&self, id: u16) -> Option<&str> {
        self.sector_specials.get(&id.to_string()).map(String::as_str)
    }

    pub fn thing_type(&self, id: u16) -> Option<&ThingType> {
        self.thing_lookup
            .get(&id)
            .and_then(|i| self.thing_types.get(*i))
    }

    /// Format a flag bitmask as a comma-separated list of named bits.
    /// Unknown bits appear as `0xNN`.
    pub fn format_linedef_flags(&self, flags: u16) -> String {
        format_flags(&self.linedef_flags, flags as u32)
    }

    pub fn format_thing_flags(&self, flags: u16) -> String {
        format_flags(&self.thing_flags, flags as u32)
    }
}

fn format_flags(table: &HashMap<String, String>, flags: u32) -> String {
    if flags == 0 {
        return "(none)".into();
    }
    let mut named: Vec<(u32, &str)> = Vec::new();
    let mut leftover = flags;
    for (k, v) in table {
        if let Ok(bit) = k.parse::<u32>() {
            if bit != 0 && (flags & bit) != 0 {
                named.push((bit, v.as_str()));
                leftover &= !bit;
            }
        }
    }
    named.sort_by_key(|(b, _)| *b);
    let mut parts: Vec<String> = named.into_iter().map(|(_, v)| v.to_string()).collect();
    if leftover != 0 {
        parts.push(format!("0x{leftover:X}"));
    }
    parts.join(", ")
}

const VANILLA_DOOM_TOML: &str = include_str!("../configs/doom.toml");
const VANILLA_DOOM2_TOML: &str = include_str!("../configs/doom2.toml");
const HERETIC_TOML: &str = include_str!("../configs/heretic.toml");
const HEXEN_TOML: &str = include_str!("../configs/hexen.toml");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vanilla_doom_loads() {
        let cfg = GameConfig::vanilla_doom();
        assert!(cfg.linedef_special(1).is_some(), "linedef 1 should exist");
        assert_eq!(cfg.sector_special(0), Some("Normal"));
    }

    #[test]
    fn linedef_flag_formatting() {
        let cfg = GameConfig::vanilla_doom();
        let formatted = cfg.format_linedef_flags(1 | 4); // Impassable + Two-sided
        assert!(formatted.contains("Impassable"));
        assert!(formatted.contains("Two-sided"));
    }
}
