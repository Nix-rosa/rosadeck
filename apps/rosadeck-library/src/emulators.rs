//! Emulator availability, read-only, for the library detail pane.
//!
//! The browser must tell the truth *before* Enter: a game whose emulator is
//! not installed cannot be played, and the honest way to say so is the name of
//! the binary plus the package that provides it. Nothing here installs,
//! downloads or writes anything: profiles are read, PATH is looked up, and the
//! install hint is a printable string (Rosadeck never runs distro tooling).
//!
//! Note the boundary: this reads config files and PATH only — no `hyprctl`, no
//! sockets, no sysfs.

use rosadeck_emulator_core::resolve_in_path;
use std::path::{Path, PathBuf};

/// What the UI needs to know about one emulator profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmulatorStatus {
    /// Profile id (`snes9x`, `dolphin`, …); matches `Platform::default_emulator`.
    pub id: String,
    /// Human name from the profile.
    pub name: String,
    /// Executable name (PATH lookup) or path.
    pub binary: String,
    /// Absolute path when found, `None` when the binary is missing.
    pub resolved: Option<PathBuf>,
}

impl EmulatorStatus {
    /// True when the executable exists (PATH or absolute path).
    pub fn installed(&self) -> bool {
        self.resolved.is_some()
    }

}

/// Load every `*.toml` in `dir` as an emulator profile (invalid files skipped).
pub fn load_from(dir: &Path) -> Vec<EmulatorStatus> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut files: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();
    let mut out = Vec::new();
    for f in files {
        // A broken profile must not break the browser; `rosadeck emulators`
        // is the place that reports the error in full.
        if let Ok(p) = rosadeck_profiles::load_emulator(&f) {
            out.push(from_profile(&p));
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Build a status from a parsed profile (resolution happens here, once).
pub fn from_profile(p: &rosadeck_profiles::EmulatorProfile) -> EmulatorStatus {
    let resolved = if let Some(abs) = &p.emulator.binary_path {
        let path = PathBuf::from(abs);
        path.is_file().then_some(path)
    } else {
        resolve_in_path(&p.emulator.binary)
    };
    EmulatorStatus { id: p.emulator.id.clone(), name: p.emulator.name.clone(), binary: p.emulator.binary.clone(), resolved }
}

/// Find the status for an emulator id (`Platform::default_emulator`).
pub fn find<'a>(list: &'a [EmulatorStatus], id: &str) -> Option<&'a EmulatorStatus> {
    list.iter().find(|e| e.id == id)
}

/// What the detail pane shows for a platform's emulator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    /// Profile found and the binary exists.
    Installed(String),
    /// Profile found, binary missing: `id` + package hint.
    Missing { id: String, hint: Option<String> },
    /// No profile for this id at all.
    Unconfigured(String),
}

impl Availability {
    /// Render the label for the detail pane (`✓` / red missing text).
    pub fn label(&self) -> String {
        match self {
            Self::Installed(id) => format!("{id} ✓"),
            Self::Missing { id, hint } => match hint {
                Some(h) => format!("{id} NOT INSTALLED · {h}"),
                None => format!("{id} NOT INSTALLED"),
            },
            Self::Unconfigured(id) => format!("{id} NOT CONFIGURED"),
        }
    }

    /// True when pressing Enter can actually start the emulator.
    pub fn playable(&self) -> bool {
        matches!(self, Self::Installed(_))
    }
}

/// Availability of `id` given the loaded profiles.
pub fn availability(list: &[EmulatorStatus], id: &str) -> Availability {
    match find(list, id) {
        None => Availability::Unconfigured(id.to_owned()),
        Some(e) if e.installed() => Availability::Installed(e.id.clone()),
        Some(e) => Availability::Missing { id: e.id.clone(), hint: install_hint(&e.id).map(str::to_owned) },
    }
}

/// Arch package that provides an emulator, as a printable hint.
///
/// Rosadeck never installs anything: this string exists so a human can copy
/// it. `None` when the mapping is unknown (custom profiles).
pub fn install_hint(id: &str) -> Option<&'static str> {
    Some(match id {
        "dolphin" => "pacman -S dolphin-emu",
        "azahar" => "pacman -S azahar",
        "snes9x" => "pacman -S snes9x-gtk",
        "mupen64plus" => "pacman -S mupen64plus",
        "retroarch" => "pacman -S retroarch (+ a libretro core)",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(id: &str) -> EmulatorStatus {
        EmulatorStatus { id: id.into(), name: id.into(), binary: "true".into(), resolved: Some(PathBuf::from("/usr/bin/true")) }
    }

    fn missing(id: &str) -> EmulatorStatus {
        EmulatorStatus { id: id.into(), name: id.into(), binary: format!("{id}-nope"), resolved: None }
    }

    #[test]
    fn installed_emulator_is_playable() {
        let list = vec![installed("snes9x")];
        assert_eq!(availability(&list, "snes9x"), Availability::Installed("snes9x".into()));
        assert!(availability(&list, "snes9x").playable());
        assert_eq!(availability(&list, "snes9x").label(), "snes9x ✓");
    }

    #[test]
    fn missing_emulator_names_the_package_and_is_not_playable() {
        let list = vec![missing("retroarch")];
        let a = availability(&list, "retroarch");
        assert!(!a.playable());
        assert!(a.label().contains("NOT INSTALLED"), "{a:?}");
        assert!(a.label().contains("pacman -S retroarch"), "{a:?}");
    }

    #[test]
    fn unknown_id_is_unconfigured() {
        let a = availability(&[installed("snes9x")], "mednafen");
        assert_eq!(a, Availability::Unconfigured("mednafen".into()));
        assert!(!a.playable());
        assert_eq!(a.label(), "mednafen NOT CONFIGURED");
    }

    #[test]
    fn absolute_binary_path_wins_over_path_lookup() {
        // Present now, absent later: the profile keeps resolving either way.
        let p = rosadeck_profiles::EmulatorProfile {
            emulator: rosadeck_profiles::EmulatorSection {
                id: "snes9x".into(),
                name: "Snes9x".into(),
                adapter: "snes9x".into(),
                binary: "snes9x-gtk".into(),
                binary_path: None,
            },
            launch: None,
            graphics: None,
            shader: None,
        };
        let st = from_profile(&p);
        assert_eq!(st.resolved, resolve_in_path("snes9x-gtk"), "same resolution the CLI uses");
        assert!(st.installed(), "snes9x-gtk is installed on the dev machine");
    }

    #[test]
    fn missing_directory_is_empty_not_a_panic() {
        assert!(load_from(Path::new("/nonexistent-rosadeck/emulators")).is_empty());
    }

    #[test]
    fn hints_exist_for_every_adapter_the_library_defaults_to() {
        // The defaults in `game-library` must always have an install hint, or a
        // user without that emulator gets no way forward.
        for id in ["dolphin", "azahar", "snes9x", "mupen64plus", "retroarch"] {
            assert!(install_hint(id).is_some(), "no hint for {id}");
        }
        assert!(install_hint("custom-emu").is_none());
    }
}
