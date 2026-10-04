//! Library commands: `library` (scan + list) and `play` (fuzzy pick → launch).
//!
//! Read-only except stats recording after successful real launches.
//! `play` reuses the F5 launch pipeline with the platform's default emulator.

use rosadeck_emulator_core::resolve_in_path;
use rosadeck_game_library::{
    candidate_dirs, default_roots, resolve_artwork, scan_roots, suggested_cover, suggested_paths, GameEntry, StatsDb, ART_EXTENSIONS,
};

/// `<emulator>` + whether its binary exists, read-only: a game whose emulator
/// is missing must be visible as such in the plain listing, not discovered by
/// pressing Enter and waiting for a failure.
fn emulator_note(profiles: &[rosadeck_profiles::EmulatorProfile], platform: rosadeck_game_library::Platform) -> String {
    let id = platform.default_emulator();
    match profiles.iter().find(|p| p.emulator.id == id) {
        None => format!("{id} (no profile)"),
        Some(p) => {
            let installed = resolve_in_path(&p.emulator.binary).is_some()
                || p.emulator.binary_path.as_deref().is_some_and(|b| std::path::Path::new(b).exists());
            if installed {
                format!("{id} ok")
            } else {
                format!("{id} MISSING")
            }
        }
    }
}

/// Stats file: `$XDG_STATE_HOME/rosadeck/library.json` (shared with the TUI).
pub fn stats_path() -> std::path::PathBuf {
    let base = std::env::var("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())).join(".local/state")
        });
    rosadeck_game_library::dir_with_legacy(base.join("rosadeck"), base.join("hyprgame")).join("library.json")
}

/// `rosadeck library [--platform P] [--query Q] [--json]`.
pub fn cmd_library(platform: Option<String>, query: Option<String>, json: bool) -> i32 {
    let games = scan_roots(&default_roots());
    let profiles = super::launch::load_emulator_profiles();
    let (db, corrupt) = StatsDb::load(&stats_path());
    if let Some(e) = corrupt {
        eprintln!("rosadeck: stats corrupt, starting fresh: {e}");
    }
    let q = query.unwrap_or_default().to_lowercase();
    let games: Vec<&GameEntry> = games
        .iter()
        .filter(|g| platform.as_ref().is_none_or(|p| g.platform.dir_name() == p || g.platform.label().eq_ignore_ascii_case(p)))
        .filter(|g| q.is_empty() || g.title.to_lowercase().contains(&q))
        .collect();
    if json {
        let items: Vec<_> = games
            .iter()
            .map(|g| {
                serde_json::json!({
                    "id": g.id, "title": g.title, "platform": g.platform.dir_name(),
                    "region": g.region, "path": g.path,
                    "emulator": g.platform.default_emulator(),
                    "emulator_ready": emulator_note(&profiles, g.platform).ends_with(" ok"),
                    "favorite": db.stats.favorites.contains(&g.id),
                    "plays": db.stats.plays.get(&g.id).map(|p| p.plays).unwrap_or(0),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items).unwrap_or_default());
        return 0;
    }
    if games.is_empty() {
        println!("no games (roms under ~/roms/<platform>/ or $ROSADECK_ROMS)");
        return 0;
    }
    for g in &games {
        let fav = if db.stats.favorites.contains(&g.id) { "*" } else { " " };
        let plays = db.stats.plays.get(&g.id).map(|p| p.plays).unwrap_or(0);
        println!("{fav} {:<44} {:<12} emulator={} plays={plays}", g.title, g.platform.label(), emulator_note(&profiles, g.platform));
    }
    0
}

/// Resolve `<query>` to one entry: exact id, then unique title substring.
/// Ambiguous/missing prints candidates and returns `None` (exit 11).
fn pick_entry<'a>(games: &'a [GameEntry], query: &str, id: bool) -> Option<&'a GameEntry> {
    if id {
        if let Some(g) = games.iter().find(|g| g.id == query) {
            return Some(g);
        }
        // `--id` came from the browser, so a miss here means the ROM moved or
        // was removed. Say so; a silent exit 11 looks like a broken tool.
        eprintln!("rosadeck: no game with id {query:?} among {} scanned roms", games.len());
        eprintln!("rosadeck: list them with: rosadeck library  (ids are stable, paths are not)");
        return None;
    }
    if let Some(g) = games.iter().find(|g| g.id == query) {
        return Some(g);
    }
    let q = query.to_lowercase();
    let hits: Vec<&GameEntry> = games.iter().filter(|g| g.title.to_lowercase().contains(&q)).collect();
    match hits.len() {
        1 => Some(hits[0]),
        0 => {
            eprintln!("rosadeck: no game matches {query:?}");
            None
        }
        _ => {
            eprintln!("rosadeck: ambiguous {query:?}:");
            for g in hits.iter().take(10) {
                eprintln!("  {} [{}] {}", g.id, g.platform.label(), g.title);
            }
            None
        }
    }
}

/// `rosadeck play <query> [--id] [--dry-run] [--json] [--yes] [display opts…]`.
#[allow(clippy::too_many_arguments)]
pub fn cmd_play(
    query: String,
    by_id: bool,
    dry_run: bool,
    json: bool,
    yes: bool,
    display_profile: Option<String>,
    mode: Option<String>,
    policy: Option<String>,
    quiet: bool,
) -> i32 {
    let games = scan_roots(&default_roots());
    let Some(entry) = pick_entry(&games, &query, by_id) else { return 11 };
    let opts = super::launch::LaunchOptions {
        emulator: Some(entry.platform.default_emulator().into()),
        game: Some(entry.path.to_string_lossy().into_owned()),
        title: Some(entry.title.clone()),
        platform: Some(entry.platform.dir_name().into()),
        display_profile,
        launch_profile: None,
        mode,
        policy,
        pos: None,
        dry_run,
        json,
        yes,
        quiet,
    };
    let (code, played_secs) = super::launch::cmd_launch_timed(&opts);
    // Se cuenta cualquier sesión en la que el emulador llegó a estar en
    // pantalla: cerrar el juego normalmente es el código 19 («el juego corrió y
    // terminó con un error»), que es el final normal de una partida. Antes sólo
    // se contaba el 0, así que casi ninguna sesión llegaba a la estantería.
    let ran = code == 0 || code == super::launch::EXIT_EMULATOR_NONZERO;
    if ran && !dry_run {
        let (mut db, _) = StatsDb::load(&stats_path());
        db.record_play(
            &entry.id,
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            played_secs,
        );
        if let Err(e) = db.save() {
            eprintln!("rosadeck: stats save failed (non-fatal): {e}");
        }
    }
    code
}

/// `rosadeck art [--platform P] [--json]`.
///
/// Reports the cover each game resolves to and, when there is none, the exact
/// file to create. Primary location is `<rom-root>/covers/` (`~/roms/covers/`),
/// named after the game; also accepted: `<rom-root>/covers/<platform>/`, next to
/// the ROM, and the legacy `<rom-root>/art/<platform>/`. Any format in
/// [`ART_EXTENSIONS`] works (png, jpg, webp, avif, gif, tiff, bmp, …).
pub fn cmd_art(platform: Option<String>, json: bool) -> i32 {
    let games = scan_roots(&default_roots());
    let games: Vec<&GameEntry> = games
        .iter()
        .filter(|g| platform.as_ref().is_none_or(|p| g.platform.dir_name() == p))
        .collect();
    let mut found = 0usize;
    if json {
        let rows: Vec<serde_json::Value> = games
            .iter()
            .map(|g| {
                let cover = resolve_artwork(&g.path).map(|a| a.cover.display().to_string());
                serde_json::json!({
                    "title": g.title,
                    "platform": g.platform.dir_name(),
                    "rom": g.path,
                    "cover": cover,
                    "suggested": suggested_cover(&g.path),
                    "search_dirs": candidate_dirs(&g.path),
                    "accepts": suggested_paths(&g.path).len(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows).unwrap_or_default());
        return 0;
    }
    for g in &games {
        match resolve_artwork(&g.path) {
            Some(art) => {
                found += 1;
                println!("  ok  {:<44} {}", g.title, art.cover.display());
            }
            None => {
                println!("  --  {:<44} {}", g.title, g.platform.label());
                if let Some(one) = suggested_cover(&g.path) {
                    println!("        crea: {}", one.display());
                }
            }
        }
    }
    println!("\n{} con portada · {} sin portada · formatos: {}", found, games.len() - found, ART_EXTENSIONS.join(" "));
    0
}
