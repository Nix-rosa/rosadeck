//! Scan: walk platform directories into stable [`GameEntry`] records.
//!
//! Convention over configuration: `<root>/<platform-dir>/<rom>`. Only known
//! platform dirs + loadable extensions; sidecars skipped. Entry ids are
//! stable across rescans (SHA-256 of `platform + canonical path`, 16 hex).

use crate::platform::{is_skipped_extension, platform_for};
use crate::title::parse_title;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// One library game (identity + display data; stats live in [`StatsDb`](crate::StatsDb)).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameEntry {
    /// Stable id (16 hex of SHA-256 over platform + path).
    pub id: String,
    /// Clean display title.
    pub title: String,
    /// Platform.
    pub platform: crate::Platform,
    /// Absolute ROM path.
    pub path: PathBuf,
    /// Region tag, when parsed.
    pub region: Option<String>,
    /// File size in bytes (for the detail pane).
    pub size: u64,
}

fn entry_id(platform: &str, path: &Path) -> String {
    let digest = Sha256::digest(format!("{platform}:{}\n", path.display()));
    format!("{digest:x}")[..16].to_owned()
}

/// Where the browser writes the ROM roots the user adds with `d`.
///
/// One path per line, `#` for comments, `~` allowed. This is the *only* file
/// Rosadeck writes outside its state directory, and only when the user asks for
/// it by adding a root.
pub fn roots_config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())).join(".config")
        });
    base.join("rosadeck/roms")
}

/// Resolve a data directory that changed name, without moving anything.
///
/// Rosadeck used to be called hyprgame, and the user's `library.json`, its
/// covers and the emulator profiles are still where they were. Rather than
/// touching their files behind their back, both names work: the new one wins as
/// soon as it exists, the old one keeps answering until they move or delete it.
pub fn dir_with_legacy(new: PathBuf, legacy: PathBuf) -> PathBuf {
    if new.exists() || !legacy.exists() { new } else { legacy }
}

/// Expand a leading `~` to `$HOME`; leave anything else alone.
pub fn expand_home(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if path == "~" {
        return match std::env::var("HOME") {
            Ok(h) => PathBuf::from(h),
            Err(_) => p,
        };
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    p
}

/// Roots the user configured with `d` (may include paths that are not mounted
/// right now, which is why the filter happens in [`default_roots`]).
pub fn configured_roots() -> Vec<PathBuf> {
    read_roots_from(&roots_config_path())
}

/// Read roots from one file: one per line, `#` comments, `~` expanded.
fn read_roots_from(path: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        out.push(expand_home(line));
    }
    out
}

/// Persist `roots`, one per line, with a header so the file explains itself.
pub fn save_configured_roots(roots: &[PathBuf]) -> std::io::Result<()> {
    write_roots_to(&roots_config_path(), roots)
}

fn write_roots_to(path: &Path, roots: &[PathBuf]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut body = String::from(
        "# Rosadeck: directorios de ROM adicionales, uno por línea.\n\
         # Se añaden con la tecla `d` en rosadeck-library. `~` vale por $HOME.\n\
         # Este archivo lo escribe la app; se puede editar a mano.\n",
    );
    for r in roots {
        body.push_str(&r.display().to_string());
        body.push('\n');
    }
    std::fs::write(path, body)
}

/// Default ROM roots: `~/roms`, the roots configured with `d`, and any
/// `$ROSADECK_ROMS` extras. Only directories that exist are returned, so a
/// unplugged drive silently contributes nothing instead of breaking the scan.
///
/// `~/roms` comes first when it exists, then the configured roots in the order
/// they were added, then the environment: the order is the scan order, and it is
/// what the status line shows.
pub fn default_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let push = |p: PathBuf, roots: &mut Vec<PathBuf>| {
        if p.is_dir() && !roots.contains(&p) {
            roots.push(p);
        }
    };
    if let Ok(home) = std::env::var("HOME") {
        push(PathBuf::from(home).join("roms"), &mut roots);
    }
    for r in configured_roots() {
        push(r, &mut roots);
    }
    if let Ok(extra) = std::env::var("ROSADECK_ROMS") {
        for p in std::env::split_paths(&extra) {
            push(p, &mut roots);
        }
    }
    roots
}

/// Scan `roots` (each expected to contain platform dirs). Sorted, deterministic.
pub fn scan_roots(roots: &[PathBuf]) -> Vec<GameEntry> {
    let mut out = Vec::new();
    for root in roots {
        let Ok(rd) = std::fs::read_dir(root) else { continue };
        let mut dirs: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()).collect();
        dirs.sort();
        for dir in dirs {
            let Some(platform) = dir.file_name().and_then(|n| n.to_str()).and_then(platform_for) else { continue };
            let Ok(files) = std::fs::read_dir(&dir) else { continue };
            let mut files: Vec<PathBuf> = files.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_file()).collect();
            files.sort();
            for file in files {
                let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                if ext.is_empty() || is_skipped_extension(&ext) || !platform.extensions().contains(&ext.as_str()) {
                    continue;
                }
                let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                let parsed = parse_title(stem);
                out.push(GameEntry {
                    id: entry_id(platform.dir_name(), &file),
                    title: parsed.title,
                    platform,
                    region: parsed.region,
                    size: file.metadata().map(|m| m.len()).unwrap_or(0),
                    path: file,
                });
            }
        }
    }
    out.sort_by(|a, b| a.title.cmp(&b.title).then(a.path.cmp(&b.path)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The roots the browser writes with `d` survive a round trip, and a line
    /// that is a comment or blank is not a root.
    ///
    /// The file is the only thing Rosadeck writes outside its state directory,
    /// and it exists because the user asked for it with a key press.
    #[test]
    fn configured_roots_round_trip_through_the_file() {
        let dir = std::env::temp_dir().join("rosadeck-roots-config");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("roms");
        // Write the file by hand: comments, blanks, `~` and absolute paths.
        std::fs::write(
            &file,
            "# comentario\n\n  /mnt/roms  \n~/Games\n/mnt/my roms\n",
        )
        .unwrap();
        let roots = read_roots_from(&file);
        assert_eq!(
            roots,
            vec![PathBuf::from("/mnt/roms"), expand_home("~/Games"), PathBuf::from("/mnt/my roms")],
            "los espacios del medio son parte de la ruta; sólo se recortan los bordes"
        );

        // And what the app writes is what it reads back.
        let want = vec![PathBuf::from("/mnt/roms"), PathBuf::from("/srv/roms2")];
        write_roots_to(&file, &want).unwrap();
        assert_eq!(read_roots_from(&file), want);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.starts_with("#"), "el fichero se explica solo: {text}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `~` is the only shell-ism: a relative path stays relative.
    #[test]
    fn only_a_leading_tilde_is_expanded() {
        std::env::set_var("HOME", "/home/tester");
        assert_eq!(expand_home("~"), PathBuf::from("/home/tester"));
        assert_eq!(expand_home("~/roms"), PathBuf::from("/home/tester/roms"));
        assert_eq!(expand_home("roms"), PathBuf::from("roms"));
        assert_eq!(expand_home("/srv/~"), PathBuf::from("/srv/~"), "no se expande en medio");
        assert_eq!(expand_home("~user/roms"), PathBuf::from("~user/roms"), "~user no es $HOME");
    }

    fn fixture_roms() -> PathBuf {
        let dir = std::env::temp_dir().join("rosadeck-lib-test");
        std::fs::remove_dir_all(&dir).ok();
        for (plat, files) in [
            ("wii", vec!["Luigi's Mansion (Europe) (En,Fr,De,Es,It).rvz", "notes.txt"]),
            ("snes", vec!["Legend of Zelda, The - A Link to the Past (Europe).sfc", "game.srm"]),
            ("ps2", vec!["something.iso"]),
        ] {
            let d = dir.join(plat);
            std::fs::create_dir_all(&d).unwrap();
            for f in files {
                std::fs::write(d.join(f), b"x").unwrap();
            }
        }
        dir
    }

    #[test]
    fn scans_known_platforms_and_skips_sidecars() {
        let dir = fixture_roms();
        let games = scan_roots(&[dir.clone()]);
        assert_eq!(games.len(), 2); // wii rvz + snes sfc; txt/srm/ps2 skipped
        assert_eq!(games[0].title, "Luigi's Mansion");
        assert_eq!(games[1].title, "The Legend of Zelda - A Link to the Past");
        // Stable ids across rescans.
        assert_eq!(scan_roots(&[dir.clone()])[0].id, games[0].id);
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod legacy_tests {
    use super::*;

    /// The rename kept the user's data reachable: the new name wins as soon as
    /// it exists, the old one answers until then, and a machine with neither gets
    /// the new path so the app can create it.
    #[test]
    fn the_new_name_wins_and_the_old_one_keeps_answering() {
        let tmp = std::env::temp_dir().join("rosadeck-legacy-test");
        std::fs::remove_dir_all(&tmp).ok();
        let nuevo = tmp.join("rosadeck");
        let viejo = tmp.join("hyprgame");
        // Ni uno ni otro: la ruta nueva, para poder crearla.
        assert_eq!(dir_with_legacy(nuevo.clone(), viejo.clone()), nuevo);
        // Sólo el viejo: sigue mandando (es donde están los datos del usuario).
        std::fs::create_dir_all(&viejo).unwrap();
        assert_eq!(dir_with_legacy(nuevo.clone(), viejo.clone()), viejo);
        // Los dos: gana el nuevo.
        std::fs::create_dir_all(&nuevo).unwrap();
        assert_eq!(dir_with_legacy(nuevo.clone(), viejo), nuevo);
        std::fs::remove_dir_all(&tmp).ok();
    }
}
