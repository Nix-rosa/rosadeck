//! Emulator + launch profiles: TOML load/parse/validate (persistence only).
//!
//! `DisplayProfile` (F1) stays untouched. `EmulatorProfile` describes one
//! emulator; `LaunchProfile` binds display + emulator (+ optional game).
//! No execution logic lives here — adapters consume validated profiles.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// `[emulator]` identity section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmulatorSection {
    /// Profile id (`dolphin`, `retroarch`, …).
    pub id: String,
    /// Human name.
    pub name: String,
    /// Adapter kind (`dolphin` | `retroarch`).
    pub adapter: String,
    /// Binary name for PATH lookup.
    pub binary: String,
    /// Explicit absolute binary (wins).
    pub binary_path: Option<String>,
}

/// `[launch]` section: extra args/env/workdir + referenced configs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmulatorLaunchSection {
    /// Extra CLI args (appended after adapter args).
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// Extra environment.
    #[serde(default)]
    pub environment: std::collections::HashMap<String, String>,
    /// Working directory.
    pub workdir: Option<String>,
    /// Base config path (referenced, never written by F5).
    pub config_path: Option<String>,
    /// Overlay config path (retroarch `--appendconfig`).
    pub append_config: Option<String>,
    /// libretro core (retroarch `-L`).
    pub core_path: Option<String>,
    /// User dir (dolphin `-u`).
    pub user_dir: Option<String>,
    /// Batch/exit-on-stop (dolphin `-b`).
    #[serde(default)]
    pub batch: bool,
    /// Mute audio at startup (snes9x `--mute`).
    pub mute: Option<bool>,
    /// Disable the speed limiter (mupen64plus `--nospeedlimit`).
    pub nospeedlimit: Option<bool>,
    /// Render size, e.g. `1024x768` (mupen64plus `--resolution`).
    pub resolution: Option<String>,
    /// Graphics plugin (mupen64plus `--gfx`).
    pub gfx_plugin: Option<String>,
}

/// `[graphics]` section: wishes recorded by adapters, applied per-adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmulatorGraphicsSection {
    /// Desired VSync (mechanism differs per adapter; see adapters).
    pub vsync: Option<bool>,
    /// Launch fullscreen (default true when unset; adapters map to verified flags).
    pub fullscreen: Option<bool>,
}

/// `[shader]` section: preset reference (resolved by shader-manager).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmulatorShaderSection {
    /// Preset id (`crt-light`, …) or `off`.
    pub preset: Option<String>,
}

/// One emulator profile (one TOML file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmulatorProfile {
    /// Identity.
    pub emulator: EmulatorSection,
    /// Launch extras.
    #[serde(default)]
    pub launch: Option<EmulatorLaunchSection>,
    /// Graphics wishes.
    #[serde(default)]
    pub graphics: Option<EmulatorGraphicsSection>,
    /// Shader reference.
    #[serde(default)]
    pub shader: Option<EmulatorShaderSection>,
}

/// Optional inline game in a launch profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchGame {
    /// Platform label.
    pub platform: String,
    /// Game path.
    pub path: String,
    /// Title.
    pub title: Option<String>,
}

/// `[launch]` binding section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchBinding {
    /// Binding id (`tv-gaming`).
    pub id: String,
    /// Display profile alias or stable id.
    pub display_profile: String,
    /// Emulator profile id.
    pub emulator_profile: String,
}

/// `[display]` disposition override section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LaunchDisplay {
    /// `duplicate` | `external-only` | `extend`.
    pub disposition: String,
    /// `max-resolution` | `max-refresh` | `balanced` | `match-internal` |
    /// `match-external` | `manual`.
    #[serde(default = "default_policy")]
    pub policy: String,
    /// Explicit `WxH@Hz` (implies manual).
    pub resolution: Option<String>,
    /// Refresh override (combined with `resolution` WxH when set).
    pub refresh: Option<f64>,
}

fn default_policy() -> String {
    "max-refresh".into()
}

/// One launch binding: display + emulator (+ optional game).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LaunchProfile {
    /// Binding.
    pub launch: LaunchBinding,
    /// Optional inline game.
    pub game: Option<LaunchGame>,
    /// Display disposition override.
    #[serde(default)]
    pub display: Option<LaunchDisplay>,
}

/// Profile file failure.
#[derive(Debug)]
pub enum EmulatorProfileError {
    /// Filesystem read failure.
    Io(std::io::Error),
    /// TOML failure.
    Toml(toml::de::Error),
    /// Semantic failure.
    Invalid(String),
}

impl std::fmt::Display for EmulatorProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Toml(e) => write!(f, "toml: {e}"),
            Self::Invalid(m) => write!(f, "invalid: {m}"),
        }
    }
}

impl std::error::Error for EmulatorProfileError {}

fn bad(m: &str) -> EmulatorProfileError {
    EmulatorProfileError::Invalid(m.into())
}

/// Validate + parse emulator TOML text.
pub fn parse_emulator(text: &str) -> Result<EmulatorProfile, EmulatorProfileError> {
    let p: EmulatorProfile = toml::from_str(text).map_err(EmulatorProfileError::Toml)?;
    if p.emulator.id.trim().is_empty() {
        return Err(bad("emulator.id empty"));
    }
    if !["dolphin", "retroarch", "azahar", "snes9x", "mupen64plus"].contains(&p.emulator.adapter.as_str()) {
        return Err(bad("emulator.adapter must be dolphin, retroarch, azahar, snes9x or mupen64plus"));
    }
    if p.emulator.binary.trim().is_empty() {
        return Err(bad("emulator.binary empty"));
    }
    Ok(p)
}

/// Validate + parse launch TOML text.
pub fn parse_launch(text: &str) -> Result<LaunchProfile, EmulatorProfileError> {
    let p: LaunchProfile = toml::from_str(text).map_err(EmulatorProfileError::Toml)?;
    if p.launch.id.trim().is_empty() || p.launch.display_profile.trim().is_empty() || p.launch.emulator_profile.trim().is_empty() {
        return Err(bad("launch binding needs id + display_profile + emulator_profile"));
    }
    if let Some(d) = &p.display {
        if !["duplicate", "external-only", "extend"].contains(&d.disposition.as_str()) {
            return Err(bad("display.disposition must be duplicate|external-only|extend"));
        }
        if !["max-resolution", "max-refresh", "balanced", "match-internal", "match-external", "manual"]
            .contains(&d.policy.as_str())
        {
            return Err(bad("display.policy unknown"));
        }
        if let Some(r) = &d.resolution {
            if !r.contains('x') {
                return Err(bad("display.resolution must be WxH[@Hz]"));
            }
        }
        if let Some(hz) = d.refresh {
            if !(hz.is_finite() && hz > 0.0) {
                return Err(bad("display.refresh must be positive"));
            }
        }
    }
    Ok(p)
}

/// Load + validate one TOML file (emulator or launch; caller picks parser).
pub fn load_emulator(path: &Path) -> Result<EmulatorProfile, EmulatorProfileError> {
    let text = std::fs::read_to_string(path).map_err(EmulatorProfileError::Io)?;
    parse_emulator(&text)
}

/// Load + validate one launch TOML file.
pub fn load_launch(path: &Path) -> Result<LaunchProfile, EmulatorProfileError> {
    let text = std::fs::read_to_string(path).map_err(EmulatorProfileError::Io)?;
    parse_launch(&text)
}

/// Find an emulator profile by id.
pub fn find_emulator<'a>(profiles: &'a [EmulatorProfile], id: &str) -> Option<&'a EmulatorProfile> {
    profiles.iter().find(|p| p.emulator.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOLPHIN: &str = include_str!("../tests/fixtures/dolphin.toml");
    const RETROARCH: &str = include_str!("../tests/fixtures/retroarch.toml");
    const AZAHAR: &str = include_str!("../tests/fixtures/azahar.toml");
    const TV_GAMING: &str = include_str!("../tests/fixtures/tv-gaming.toml");

    #[test]
    fn parses_emulator_profiles() {
        let d = parse_emulator(DOLPHIN).unwrap();
        assert_eq!(d.emulator.adapter, "dolphin");
        assert_eq!(d.graphics.unwrap().vsync, Some(true));
        let r = parse_emulator(RETROARCH).unwrap();
        assert_eq!(r.launch.unwrap().core_path.as_deref(), Some("/cores/snes9x_libretro.so"));
    }

    #[test]
    fn parses_launch_binding() {
        let l = parse_launch(TV_GAMING).unwrap();
        assert_eq!(l.launch.emulator_profile, "dolphin");
        assert_eq!(l.display.unwrap().disposition, "external-only");
    }

    #[test]
    fn rejects_bad_profiles() {
        assert!(parse_emulator("emulator = 1").is_err());
        assert!(parse_emulator(DOLPHIN.replace("dolphin", "ps3").as_str()).is_err());
        assert!(parse_launch(TV_GAMING.replace("external-only", "sideways").as_str()).is_err());
    }

    #[test]
    fn finds_by_id() {
        let ps = vec![parse_emulator(DOLPHIN).unwrap(), parse_emulator(RETROARCH).unwrap(), parse_emulator(AZAHAR).unwrap()];
        assert_eq!(find_emulator(&ps, "retroarch").unwrap().emulator.binary, "retroarch");
        assert_eq!(find_emulator(&ps, "azahar").unwrap().graphics.as_ref().unwrap().fullscreen, Some(true));
        assert!(find_emulator(&ps, "pcsx2").is_none());
    }
}
