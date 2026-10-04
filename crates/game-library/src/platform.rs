//! Platforms: directory + extension identification, default emulator mapping.
//!
//! Extension lists are curated per adapter capability. `.zip` rides along
//! for RetroArch cores; the Dolphin adapter rejects it (documented
//! assumption: Dolphin does not load zipped ROMs — recheck if it errors).

use serde::{Deserialize, Serialize};

/// Supported platform (directory name under the ROM root).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Platform {
    /// Super Nintendo (`.sfc`, `.smc`, `.zip`).
    Snes,
    /// Nintendo 64 (`.z64`, `.n64`, `.v64`, `.zip`).
    N64,
    /// GameCube (`.iso`, `.rvz`, `.ciso`, `.gcm`).
    GameCube,
    /// Wii (`.iso`, `.rvz`, `.ciso`, `.wbfs`, `.wad`).
    Wii,
    /// Nintendo 3DS (`.3ds`, `.cci`, `.cxi`).
    Nintendo3DS,
}

impl Platform {
    /// Canonical ROM directory name.
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::Snes => "snes",
            Self::N64 => "n64",
            Self::GameCube => "gamecube",
            Self::Wii => "wii",
            Self::Nintendo3DS => "3ds",
        }
    }

    /// Human label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Snes => "SNES",
            Self::N64 => "Nintendo 64",
            Self::GameCube => "GameCube",
            Self::Wii => "Wii",
            Self::Nintendo3DS => "Nintendo 3DS",
        }
    }

    /// Loadable extensions (lowercase, without dot).
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Snes => &["sfc", "smc", "zip"],
            Self::N64 => &["z64", "n64", "v64", "zip"],
            Self::GameCube => &["iso", "rvz", "ciso", "gcm"],
            Self::Wii => &["iso", "rvz", "ciso", "wbfs", "wad"],
            Self::Nintendo3DS => &["3ds", "cci", "cxi"],
        }
    }

    /// Default emulator profile id for the platform.
    pub fn default_emulator(self) -> &'static str {
        match self {
            // Native emulators first: RetroArch needs a core per system and is
            // not installed here, so it is opt-in, never the default.
            Self::Snes => "snes9x",
            Self::N64 => "mupen64plus",
            Self::GameCube | Self::Wii => "dolphin",
            Self::Nintendo3DS => "azahar",
        }
    }

    /// All platforms in sidebar order.
    pub fn all() -> &'static [Platform] {
        &[Platform::Snes, Platform::N64, Platform::GameCube, Platform::Wii, Platform::Nintendo3DS]
    }
}

/// Identify a platform from its ROM directory name.
pub fn platform_for(dir_name: &str) -> Option<Platform> {
    Platform::all().iter().find(|p| p.dir_name() == dir_name).copied()
}

/// Non-game sidecar extensions to skip (saves, states, notes, images).
pub fn is_skipped_extension(ext: &str) -> bool {
    matches!(ext, "srm" | "sav" | "state" | "txt" | "nfo" | "cue" | "000" | "png" | "jpg" | "jpeg")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_and_emulator_mapping() {
        assert_eq!(platform_for("wii"), Some(Platform::Wii));
        assert_eq!(platform_for("ps2"), None);
        assert_eq!(Platform::Wii.default_emulator(), "dolphin");
        assert_eq!(Platform::GameCube.default_emulator(), "dolphin");
        assert_eq!(Platform::Nintendo3DS.default_emulator(), "azahar");
        assert_eq!(Platform::Snes.default_emulator(), "snes9x");
        assert_eq!(Platform::N64.default_emulator(), "mupen64plus");
        assert!(is_skipped_extension("srm") && !is_skipped_extension("rvz"));
    }
}
