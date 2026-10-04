//! `shader-manager`: preset-ID → path resolution only.
//!
//! No rendering, no config writes, no downloads. A registry maps preset IDs
//! to paths plus the emulator IDs that accept them; `resolve` reports
//! missing files and incompatibilities explicitly. CRT labels (`crt-off`,
//! `crt-light`, …) are *references* to presets, not calibrations: F5 asserts
//! nothing about their look until real preset files exist.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One preset entry (one TOML file or table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShaderPreset {
    /// Stable preset id (`crt-light`, …).
    pub id: String,
    /// Filesystem path to the `.slangp`/`.glslp` preset.
    pub path: String,
    /// Emulator ids accepting it (`retroarch`, …).
    pub emulators: Vec<String>,
    /// Human note (not a calibration claim).
    pub note: Option<String>,
}

/// Resolution failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShaderError {
    /// Unknown preset id.
    UnknownPreset(String),
    /// Emulator does not accept this preset.
    Incompatible {
        /// Preset id.
        preset: String,
        /// Emulator id.
        emulator: String,
    },
    /// Preset file absent on disk.
    MissingFile(PathBuf),
}

impl std::fmt::Display for ShaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownPreset(p) => write!(f, "unknown shader preset: {p}"),
            Self::Incompatible { preset, emulator } => write!(f, "preset {preset} incompatible with {emulator}"),
            Self::MissingFile(p) => write!(f, "preset file missing: {}", p.display()),
        }
    }
}

impl std::error::Error for ShaderError {}

/// Successfully resolved preset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedShader {
    /// Preset id.
    pub id: String,
    /// Verified path.
    pub path: PathBuf,
}

/// In-memory preset registry (built from TOML files or fixtures).
#[derive(Debug, Default)]
pub struct ShaderRegistry {
    /// Presets by id.
    presets: HashMap<String, ShaderPreset>,
}

#[derive(Debug, Deserialize)]
struct RegistryFile {
    /// Preset entries.
    #[serde(default)]
    preset: Vec<ShaderPreset>,
}

impl ShaderRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert one preset (later inserts win on id clash).
    pub fn insert(&mut self, preset: ShaderPreset) {
        self.presets.insert(preset.id.clone(), preset);
    }

    /// Load every `*.toml` in `dir` (each: `[[preset]]` entries). Missing dir
    /// yields an empty registry (never an error: shaders are optional).
    pub fn from_dir(dir: &Path) -> Self {
        let mut reg = Self::new();
        let Ok(rd) = std::fs::read_dir(dir) else { return reg };
        let mut files: Vec<_> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        files.sort();
        for f in files {
            if f.extension().is_some_and(|x| x == "toml") {
                if let Ok(text) = std::fs::read_to_string(&f) {
                    if let Ok(parsed) = toml::from_str::<RegistryFile>(&text) {
                        for p in parsed.preset {
                            reg.insert(p);
                        }
                    }
                }
            }
        }
        reg
    }

    /// Resolve `preset_id` for `emulator`: known + compatible + file present.
    pub fn resolve(&self, preset_id: &str, emulator: &str) -> Result<ResolvedShader, ShaderError> {
        let p = self.presets.get(preset_id).ok_or_else(|| ShaderError::UnknownPreset(preset_id.into()))?;
        if !p.emulators.iter().any(|e| e == emulator) {
            return Err(ShaderError::Incompatible { preset: preset_id.into(), emulator: emulator.into() });
        }
        let path = PathBuf::from(&p.path);
        if !path.is_file() {
            return Err(ShaderError::MissingFile(path));
        }
        Ok(ResolvedShader { id: preset_id.into(), path })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> ShaderRegistry {
        let mut r = ShaderRegistry::new();
        r.insert(ShaderPreset {
            id: "crt-light".into(),
            path: "/tmp/rosadeck-shader-test/crt.slangp".into(),
            emulators: vec!["retroarch".into()],
            note: Some("conceptual reference only".into()),
        });
        r
    }

    #[test]
    fn resolves_known_compatible_present() {
        let dir = std::path::Path::new("/tmp/rosadeck-shader-test");
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("crt.slangp"), "# stub preset\n").unwrap();
        let r = registry().resolve("crt-light", "retroarch").unwrap();
        assert_eq!(r.id, "crt-light");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn reports_unknown_incompatible_missing() {
        let r = registry();
        assert_eq!(r.resolve("nope", "retroarch"), Err(ShaderError::UnknownPreset("nope".into())));
        assert!(matches!(
            r.resolve("crt-light", "dolphin"),
            Err(ShaderError::Incompatible { .. })
        ));
        let mut r2 = ShaderRegistry::new();
        r2.insert(ShaderPreset { id: "ghost".into(), path: "/no/such/file.slangp".into(), emulators: vec!["retroarch".into()], note: None });
        assert!(matches!(r2.resolve("ghost", "retroarch"), Err(ShaderError::MissingFile(_))));
    }

    #[test]
    fn missing_dir_is_empty_registry() {
        let r = ShaderRegistry::from_dir(Path::new("/no/such/dir-rosadeck"));
        assert!(r.resolve("crt-light", "retroarch").is_err());
    }
}
