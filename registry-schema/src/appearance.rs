//! Appearance settings — the registry home of the old `config.appearance`.

use serde::{Deserialize, Serialize};

use icedtea_registry::{KeySpec, Registry, RegistryError, Tag, Value};

use crate::{as_opt_str, as_str, as_u64, spec, spec_choices, spec_nullable, spec_range};

pub const BAR_POSITION: &str = "/org/icedtea/appearance/bar_position";
pub const BAR_HEIGHT: &str = "/org/icedtea/appearance/bar_height";
pub const CORNER_RADIUS: &str = "/org/icedtea/appearance/corner_radius";
pub const SNAP_GAP: &str = "/org/icedtea/appearance/snap_gap";
pub const PALETTE_BACKGROUND: &str = "/org/icedtea/appearance/palette/background";
pub const PALETTE_FOREGROUND: &str = "/org/icedtea/appearance/palette/foreground";
pub const PALETTE_ACCENT: &str = "/org/icedtea/appearance/palette/accent";
pub const WALLPAPER: &str = "/org/icedtea/appearance/wallpaper";

pub fn specs() -> Vec<KeySpec> {
    vec![
        spec_choices(
            BAR_POSITION,
            Tag::Str,
            Value::Str("bottom".into()),
            "Which screen edge the bar is anchored to.",
            vec![Value::Str("top".into()), Value::Str("bottom".into())],
        ),
        spec_range(
            BAR_HEIGHT,
            Tag::Uint,
            Value::Uint(42),
            "Bar height in logical pixels.",
            Value::Uint(1),
            Value::Uint(4096),
        ),
        spec_range(
            CORNER_RADIUS,
            Tag::Uint,
            Value::Uint(8),
            "Window corner radius in logical pixels.",
            Value::Uint(0),
            Value::Uint(256),
        ),
        spec_range(
            SNAP_GAP,
            Tag::Uint,
            Value::Uint(8),
            "Gap left between snapped windows and the screen edge.",
            Value::Uint(0),
            Value::Uint(256),
        ),
        spec(
            PALETTE_BACKGROUND,
            Tag::Str,
            Value::Str("#1e1e2e".into()),
            "Palette background colour (CSS colour string).",
        ),
        spec(
            PALETTE_FOREGROUND,
            Tag::Str,
            Value::Str("#cdd6f4".into()),
            "Palette foreground colour (CSS colour string).",
        ),
        spec(
            PALETTE_ACCENT,
            Tag::Str,
            Value::Str("#89b4fa".into()),
            "Palette accent colour (CSS colour string).",
        ),
        spec_nullable(WALLPAPER, Tag::Str, "Wallpaper path, or null for none."),
    ]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Palette {
    pub background: String,
    pub foreground: String,
    pub accent: String,
}

/// Mirror of the old `contract::Appearance`, field-for-field, so the importer
/// can deserialize a pre-registry row and the typed load/save round-trips it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Appearance {
    pub bar_position: String,
    pub bar_height: i32,
    pub corner_radius: i32,
    pub snap_gap: i32,
    pub palette: Palette,
    pub wallpaper: Option<String>,
}

/// Conversions to/from the D-Bus wire type (`icedtea_contract::Appearance`),
/// which the compositor emits in `Event::ConfigReloaded` and passes to
/// `render::wallpaper_color`. Field-identical; this is the seam that keeps the
/// registry's own type and the wire type from leaking into each other.
impl From<icedtea_contract::Appearance> for Appearance {
    fn from(a: icedtea_contract::Appearance) -> Self {
        Appearance {
            bar_position: a.bar_position,
            bar_height: a.bar_height,
            corner_radius: a.corner_radius,
            snap_gap: a.snap_gap,
            palette: Palette {
                background: a.palette.background,
                foreground: a.palette.foreground,
                accent: a.palette.accent,
            },
            wallpaper: a.wallpaper,
        }
    }
}

impl From<Appearance> for icedtea_contract::Appearance {
    fn from(a: Appearance) -> Self {
        icedtea_contract::Appearance {
            bar_position: a.bar_position,
            bar_height: a.bar_height,
            corner_radius: a.corner_radius,
            snap_gap: a.snap_gap,
            palette: icedtea_contract::Palette {
                background: a.palette.background,
                foreground: a.palette.foreground,
                accent: a.palette.accent,
            },
            wallpaper: a.wallpaper,
        }
    }
}

impl Appearance {
    pub fn load(reg: &Registry) -> Result<Appearance, RegistryError> {
        let (bar_position, _) = reg.get(BAR_POSITION)?;
        let (bar_height, _) = reg.get(BAR_HEIGHT)?;
        let (corner_radius, _) = reg.get(CORNER_RADIUS)?;
        let (snap_gap, _) = reg.get(SNAP_GAP)?;
        let (background, _) = reg.get(PALETTE_BACKGROUND)?;
        let (foreground, _) = reg.get(PALETTE_FOREGROUND)?;
        let (accent, _) = reg.get(PALETTE_ACCENT)?;
        let (wallpaper, _) = reg.get(WALLPAPER)?;
        Ok(Appearance {
            bar_position: as_str(bar_position, BAR_POSITION)?,
            bar_height: as_u64(bar_height, BAR_HEIGHT)? as i32,
            corner_radius: as_u64(corner_radius, CORNER_RADIUS)? as i32,
            snap_gap: as_u64(snap_gap, SNAP_GAP)? as i32,
            palette: Palette {
                background: as_str(background, PALETTE_BACKGROUND)?,
                foreground: as_str(foreground, PALETTE_FOREGROUND)?,
                accent: as_str(accent, PALETTE_ACCENT)?,
            },
            wallpaper: as_opt_str(wallpaper, WALLPAPER)?,
        })
    }

    pub fn entries(&self) -> Vec<(String, Value)> {
        vec![
            (
                BAR_POSITION.to_string(),
                Value::Str(self.bar_position.clone()),
            ),
            (
                BAR_HEIGHT.to_string(),
                Value::Uint(self.bar_height.max(0) as u64),
            ),
            (
                CORNER_RADIUS.to_string(),
                Value::Uint(self.corner_radius.max(0) as u64),
            ),
            (
                SNAP_GAP.to_string(),
                Value::Uint(self.snap_gap.max(0) as u64),
            ),
            (
                PALETTE_BACKGROUND.to_string(),
                Value::Str(self.palette.background.clone()),
            ),
            (
                PALETTE_FOREGROUND.to_string(),
                Value::Str(self.palette.foreground.clone()),
            ),
            (
                PALETTE_ACCENT.to_string(),
                Value::Str(self.palette.accent.clone()),
            ),
            (
                WALLPAPER.to_string(),
                match &self.wallpaper {
                    Some(path) => Value::Str(path.clone()),
                    None => Value::Null,
                },
            ),
        ]
    }

    pub fn save(&self, reg: &Registry) -> Result<u64, RegistryError> {
        reg.set_many(&self.entries())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_cover_every_declared_key() {
        let appearance = Appearance {
            bar_position: "top".into(),
            bar_height: 30,
            corner_radius: 4,
            snap_gap: 6,
            palette: Palette {
                background: "#000".into(),
                foreground: "#fff".into(),
                accent: "#f00".into(),
            },
            wallpaper: None,
        };
        let paths: Vec<_> = appearance.entries().into_iter().map(|(p, _)| p).collect();
        let mut declared: Vec<_> = specs().into_iter().map(|s| s.path.to_string()).collect();
        declared.sort();
        let mut paths = paths;
        paths.sort();
        assert_eq!(paths, declared);
    }
}
