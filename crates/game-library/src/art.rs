//! Artwork lookup by convention: no scraping, no config, no renaming of ROMs.
//!
//! ## Where covers live
//!
//! `<rom-root>/covers/` — for `~/roms` that is `~/roms/covers/` — is the
//! primary place, searched first so an explicit drop-in always wins. Then, in
//! order:
//!
//! 1. `<rom-root>/covers/<platform>/` — same folder, sorted by platform
//! 2. next to the ROM itself
//! 3. `<rom-root>/art/<platform>/` — the older layout, still honoured
//!
//! `ROSADECK_COVERS` overrides the primary folder when set (colon-separated),
//! which is how a library outside `~/roms` gets its covers.
//!
//! ## How covers are named
//!
//! Two conventions, in this order:
//!
//! 1. **Game name** — `Luigi's Mansion.png` (No-Intro tags stripped), which is
//!    what a person types and what the UI suggests.
//! 2. **ROM stem** — `Luigi's Mansion (Europe) (En,Fr,De,Es,It).png`.
//!
//! Matching is case-insensitive and every format in [`ART_EXTENSIONS`] is
//! tried. Missing art is never an error: the browser draws a generated cover.

use std::path::{Path, PathBuf};

/// Extensions tried when looking for artwork, in priority order.
///
/// Mirrors the decoders enabled in the TUI (`image` with its default format set
/// plus AVIF via dav1d): PNG, JPEG, WebP, AVIF/HEIF, GIF, TIFF, BMP, TGA, ICO,
/// QOI, PNM, DDS, EXR, HDR, farbfeld.
pub const ART_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "avif", "heic", "heif", "gif", "tif", "tiff", "bmp", "tga", "ico", "qoi", "pnm", "ppm", "pgm", "pbm", "pam", "dds", "exr", "hdr", "ff",
];

/// Primary covers folder name under a ROM root.
pub const COVERS_DIR: &str = "covers";
/// Legacy covers folder name under a ROM root.
pub const ART_DIR: &str = "art";

/// Resolved artwork (cover only in this version).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artwork {
    /// Cover image path.
    pub cover: PathBuf,
}

/// Every name a cover for `rom_path` may legitimately use, in lookup order.
pub fn candidate_names(rom_path: &Path) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    if let Some(stem) = rom_path.file_stem().and_then(|s| s.to_str()) {
        // Game name first: `Luigi's Mansion (Europe) (En,Fr,De,Es,It)` ->
        // `Luigi's Mansion`.
        let clean = crate::title::parse_title(stem).title;
        if !clean.is_empty() && clean != stem {
            names.push(clean.clone());
            names.push(stem.to_owned());
        } else if !clean.is_empty() {
            names.push(clean.clone());
        }
        // The article moved back where the ROM had it. Titles are stored with
        // the article in front (`The Legend of Zelda - A Link to the Past`) but
        // plenty of ROMs — and the covers people save next to them — use
        // `Legend of Zelda, The - A Link to the Past`. Both must resolve, or the
        // artwork silently degrades to a generated cover.
        if let Some(trailed) = crate::title::trailed_article(&clean) {
            names.push(trailed);
        }
    }
    names.dedup();
    names
}

/// Every file path this ROM would accept, in lookup order.
///
/// Exactly what the user has to create; used by `rosadeck art`.
pub fn suggested_paths(rom_path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in candidate_dirs(rom_path) {
        for name in candidate_names(rom_path) {
            for ext in ART_EXTENSIONS {
                out.push(dir.join(format!("{name}.{ext}")));
            }
        }
    }
    out
}

/// The one file name worth creating per ROM: the game name in the primary
/// covers folder, with the most common extension.
pub fn suggested_cover(rom_path: &Path) -> Option<PathBuf> {
    let dir = candidate_dirs(rom_path).into_iter().next()?;
    let name = candidate_names(rom_path).into_iter().next()?;
    Some(dir.join(format!("{name}.png")))
}

/// Directories searched for `rom_path`, in priority order.
///
/// `extra` comes from `ROSADECK_COVERS` (colon-separated) and, when set,
/// replaces the inferred `<root>/covers`.
pub fn candidate_dirs_with(rom_path: &Path, extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = extra.iter().filter(|p| !p.as_os_str().is_empty()).cloned().collect();
    if let Some(parent) = rom_path.parent() {
        let platform = parent.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if let Some(root) = parent.parent() {
            if dirs.is_empty() {
                dirs.push(root.join(COVERS_DIR));
            }
            dirs.push(root.join(COVERS_DIR).join(platform));
            dirs.push(parent.to_path_buf());
            dirs.push(root.join(ART_DIR).join(platform));
        } else {
            dirs.push(parent.to_path_buf());
        }
    }
    dirs.dedup();
    dirs
}

/// [`candidate_dirs_with`] with no overrides.
pub fn candidate_dirs(rom_path: &Path) -> Vec<PathBuf> {
    candidate_dirs_with(rom_path, &env_covers_dirs())
}

/// Covers folders from `ROSADECK_COVERS` (colon-separated, existing only).
pub fn env_covers_dirs() -> Vec<PathBuf> {
    match std::env::var("ROSADECK_COVERS") {
        Ok(list) => std::env::split_paths(&list).filter(|p| p.is_dir()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Resolve the cover for `rom_path`, or `None` when there is no art.
///
/// Each directory is listed at most once per call, which makes the
/// case-insensitive fallback cheap on libraries with hundreds of games.
pub fn resolve_artwork(rom_path: &Path) -> Option<Artwork> {
    resolve_artwork_with(rom_path, &env_covers_dirs())
}

/// [`resolve_artwork`] with explicit override directories.
pub fn resolve_artwork_with(rom_path: &Path, extra: &[PathBuf]) -> Option<Artwork> {
    let names = candidate_names(rom_path);
    if names.is_empty() {
        return None;
    }
    let dirs = candidate_dirs_with(rom_path, extra);
    let mut listings: Vec<Vec<(String, PathBuf)>> = Vec::with_capacity(dirs.len());
    for dir in &dirs {
        // Fast path: exact name, exact extension.
        for name in &names {
            for ext in ART_EXTENSIONS {
                let exact = dir.join(format!("{name}.{ext}"));
                if exact.is_file() {
                    return Some(Artwork { cover: exact });
                }
            }
        }
        listings.push(list_dir(dir));
    }
    // Slow path: same names, different case (`luigi's mansion.PNG`).
    for listing in &listings {
        if listing.is_empty() {
            continue;
        }
        for name in &names {
            let want = name.to_lowercase();
            for ext in ART_EXTENSIONS {
                let target = format!("{want}.{ext}");
                if let Some((_, path)) = listing.iter().find(|(f, _)| *f == target) {
                    return Some(Artwork { cover: path.clone() });
                }
            }
        }
    }
    None
}

/// Lowercased file names + paths for one directory (empty when unreadable).
fn list_dir(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    rd.filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_lowercase();
            Some((name, e.path()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Platform;

    /// One isolated tree per test (tests run in parallel and share the tmpdir).
    fn fixture(name: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join("rosadeck-art-lookup").join(name);
        std::fs::remove_dir_all(&root).ok();
        let wii = root.join("wii");
        let covers = root.join(COVERS_DIR);
        let art = root.join("art/wii");
        std::fs::create_dir_all(&wii).unwrap();
        std::fs::create_dir_all(&covers).unwrap();
        std::fs::create_dir_all(&art).unwrap();
        std::fs::write(wii.join("Luigi's Mansion (Europe) (En,Fr,De,Es,It).rvz"), b"rom").unwrap();
        std::fs::write(wii.join("Mario Kart 7 (USA) (Rev 1).cci"), b"rom").unwrap();
        (root, wii, covers, art)
    }

    const LM: &str = "Luigi's Mansion (Europe) (En,Fr,De,Es,It)";

    #[test]
    fn covers_folder_is_searched_before_everything_else() {
        let (root, wii, covers, art) = fixture("covers_first");
        // Same cover in three places: the covers folder must win.
        std::fs::write(covers.join("Luigi's Mansion.png"), b"a").unwrap();
        std::fs::write(wii.join(format!("{LM}.png")), b"b").unwrap();
        std::fs::write(art.join("Luigi's Mansion.png"), b"c").unwrap();
        let found = resolve_artwork(&root.join(format!("wii/{LM}.rvz"))).unwrap();
        assert_eq!(found.cover, covers.join("Luigi's Mansion.png"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn game_name_wins_over_rom_stem_within_a_folder() {
        let (root, _wii, covers, _art) = fixture("game_name_first");
        std::fs::write(covers.join(format!("{LM}.png")), b"rom stem").unwrap();
        std::fs::write(covers.join("Luigi's Mansion.png"), b"game name").unwrap();
        let found = resolve_artwork(&root.join(format!("wii/{LM}.rvz"))).unwrap();
        assert_eq!(found.cover, covers.join("Luigi's Mansion.png"), "the game name is the primary key");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn per_platform_subfolder_is_used() {
        let (root, _wii, covers, _art) = fixture("covers_platform");
        std::fs::create_dir_all(covers.join("wii")).unwrap();
        std::fs::write(covers.join("wii/Mario Kart 7.png"), b"x").unwrap();
        let found = resolve_artwork(&root.join("wii/Mario Kart 7 (USA) (Rev 1).cci")).unwrap();
        assert_eq!(found.cover, covers.join("wii/Mario Kart 7.png"));
        // A cover filed under the wrong platform must not be picked up.
        std::fs::create_dir_all(covers.join("snes")).unwrap();
        std::fs::write(covers.join("snes/Mario Kart 7.png"), b"x").unwrap();
        std::fs::remove_file(covers.join("wii/Mario Kart 7.png")).ok();
        assert!(resolve_artwork(&root.join("wii/Mario Kart 7 (USA) (Rev 1).cci")).is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rom_stem_still_works_as_a_fallback() {
        let (root, _wii, covers, _art) = fixture("rom_stem");
        std::fs::write(covers.join(format!("{LM}.webp")), b"x").unwrap();
        let found = resolve_artwork(&root.join(format!("wii/{LM}.rvz"))).unwrap();
        assert!(found.cover.to_string_lossy().ends_with(".webp"), "{found:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn next_to_the_rom_and_legacy_art_folder_still_work() {
        let (root, wii, _covers, art) = fixture("legacy");
        std::fs::write(wii.join(format!("{LM}.png")), b"x").unwrap();
        assert!(resolve_artwork(&root.join(format!("wii/{LM}.rvz"))).is_some(), "next to the ROM");
        std::fs::remove_file(wii.join(format!("{LM}.png"))).ok();
        std::fs::write(art.join("Luigi's Mansion.png"), b"x").unwrap();
        assert!(resolve_artwork(&root.join(format!("wii/{LM}.rvz"))).is_some(), "legacy art folder");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn every_supported_extension_resolves() {
        for ext in ["png", "jpg", "jpeg", "webp", "avif", "heic", "gif", "tif", "tiff", "bmp", "tga", "ico", "qoi", "pnm", "ff"] {
            let root = std::env::temp_dir().join("rosadeck-art-lookup/ext").join(ext);
            std::fs::remove_dir_all(&root).ok();
            let wii = root.join("wii");
            let covers = root.join(COVERS_DIR);
            std::fs::create_dir_all(&wii).unwrap();
            std::fs::create_dir_all(&covers).unwrap();
            std::fs::write(wii.join(format!("{LM}.rvz")), b"rom").unwrap();
            std::fs::write(covers.join(format!("Luigi's Mansion.{ext}")), b"x").unwrap();
            assert_eq!(
                resolve_artwork(&root.join(format!("wii/{LM}.rvz"))).map(|a| a.cover),
                Some(covers.join(format!("Luigi's Mansion.{ext}"))),
                "{ext}"
            );
            std::fs::remove_dir_all(&root).ok();
        }
    }

    #[test]
    fn covers_named_with_the_rom_article_spelling_resolve() {
        // Regression: `Legend of Zelda, The - A Link to the Past.png` next to
        // the ROM did not match the parsed title `The Legend of Zelda - ...`,
        // so the artwork silently degraded to a generated cover.
        let root = std::env::temp_dir().join("rosadeck-art-article");
        std::fs::remove_dir_all(&root).ok();
        let snes = root.join("snes");
        let covers = root.join(COVERS_DIR);
        std::fs::create_dir_all(&snes).unwrap();
        std::fs::create_dir_all(&covers).unwrap();
        let rom = snes.join("Legend of Zelda, The - A Link to the Past (Europe).sfc");
        std::fs::write(&rom, b"rom").unwrap();
        let cover = covers.join("Legend of Zelda, The - A Link to the Past.png");
        std::fs::write(&cover, b"x").unwrap();
        assert_eq!(resolve_artwork(&rom).map(|a| a.cover), Some(cover.clone()));

        // The 3DS one too, and with the other extension the user has.
        let n3ds = root.join("3ds");
        std::fs::create_dir_all(&n3ds).unwrap();
        let rom2 = n3ds.join("Legend of Zelda, The - A Link Between Worlds (Europe) (En,Fr,De,Es,It).3ds");
        std::fs::write(&rom2, b"rom").unwrap();
        let cover2 = covers.join("Legend of Zelda, The - A Link Between Worlds.jpg");
        std::fs::write(&cover2, b"x").unwrap();
        assert_eq!(resolve_artwork(&rom2).map(|a| a.cover), Some(cover2));

        // And the suggestion tells the truth about which spelling wins.
        assert_eq!(suggested_cover(&rom).map(|p| p.file_name().unwrap().to_string_lossy().into_owned()),
            Some("The Legend of Zelda - A Link to the Past.png".to_owned()));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn is_case_insensitive() {
        let (root, _wii, covers, _art) = fixture("case");
        std::fs::write(covers.join("luigi's mansion.PNG"), b"x").unwrap();
        let found = resolve_artwork(&root.join(format!("wii/{LM}.rvz"))).unwrap();
        assert!(found.cover.to_string_lossy().ends_with("luigi's mansion.PNG"), "{found:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn override_directory_replaces_the_inferred_one() {
        let (root, _wii, covers, _art) = fixture("override");
        let elsewhere = std::env::temp_dir().join("rosadeck-art-lookup/elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("Luigi's Mansion.png"), b"x").unwrap();
        let extra = vec![elsewhere.clone()];
        assert_eq!(
            resolve_artwork_with(&root.join(format!("wii/{LM}.rvz")), &extra).map(|a| a.cover),
            Some(elsewhere.join("Luigi's Mansion.png"))
        );
        // And the inferred covers folder is not consulted once overridden.
        std::fs::write(covers.join("Luigi's Mansion.png"), b"y").unwrap();
        assert_eq!(
            resolve_artwork_with(&root.join(format!("wii/{LM}.rvz")), &extra).map(|a| a.cover),
            Some(elsewhere.join("Luigi's Mansion.png"))
        );
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(elsewhere).ok();
    }

    #[test]
    fn no_art_is_none_not_a_panic() {
        let (root, _wii, _covers, _art) = fixture("missing");
        assert!(resolve_artwork(&root.join("wii/Mario Kart 7 (USA) (Rev 1).cci")).is_none());
        assert!(resolve_artwork(Path::new("/nope/rom.rvz")).is_none());
        assert!(resolve_artwork(Path::new("")).is_none());
        assert!(suggested_cover(Path::new("/")).is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn suggestions_point_at_the_covers_folder() {
        let (root, _wii, covers, _art) = fixture("suggest");
        let one = suggested_cover(&root.join(format!("wii/{LM}.rvz"))).unwrap();
        assert_eq!(one, covers.join("Luigi's Mansion.png"), "game name, primary folder, png");
        let all = suggested_paths(&root.join(format!("wii/{LM}.rvz")));
        assert!(all.len() > 100, "every folder x name x extension is offered");
        assert!(all.iter().all(|p| p.is_absolute() || p.starts_with(&root)));
        assert_eq!(Platform::Wii.dir_name(), "wii", "sanity: platform dir naming");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn candidate_names_put_the_game_name_first() {
        let names = candidate_names(Path::new("/roms/wii/Luigi's Mansion (Europe) (En,Fr,De,Es,It).rvz"));
        assert_eq!(names[0], "Luigi's Mansion");
        assert_eq!(names[1], "Luigi's Mansion (Europe) (En,Fr,De,Es,It)");
        // Without No-Intro tags there is only one name to try.
        assert_eq!(candidate_names(Path::new("/roms/wii/Metroid.bin")), vec!["Metroid".to_owned()]);
        assert!(candidate_names(Path::new("/")).is_empty());
    }

    #[test]
    fn candidate_dirs_order_is_covers_then_platform_then_rom_then_legacy() {
        let dirs = candidate_dirs_with(Path::new("/roms/wii/Game.rvz"), &[]);
        assert_eq!(dirs[0], PathBuf::from("/roms/covers"));
        assert_eq!(dirs[1], PathBuf::from("/roms/covers/wii"));
        assert_eq!(dirs[2], PathBuf::from("/roms/wii"));
        assert_eq!(dirs[3], PathBuf::from("/roms/art/wii"));
        assert_eq!(dirs.len(), 4);
        // Degenerate paths still yield something usable.
        assert!(!candidate_dirs_with(Path::new("Game.rvz"), &[]).is_empty());
        assert_eq!(candidate_dirs_with(Path::new("/roms/wii/Game.rvz"), &[PathBuf::from("/x")])[0], PathBuf::from("/x"));
    }
}