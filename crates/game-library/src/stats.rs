//! Local stats: favorites + play time (JSON in the state dir, no accounts).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Per-game counters.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlayStat {
    /// Total launches. Kept next to `played_secs`: it costs nothing and says
    /// how many sessions produced that time.
    pub plays: u32,
    /// Seconds of emulator time, accumulated over every session.
    #[serde(default)]
    pub played_secs: u64,
    /// Last launch (unix seconds).
    pub last_played: u64,
}

/// Whole stats file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LibraryStats {
    /// Favorite game ids.
    #[serde(default)]
    pub favorites: Vec<String>,
    /// Play counters by game id.
    #[serde(default)]
    pub plays: HashMap<String, PlayStat>,
}

/// Load/save wrapper (missing file = empty stats, never an error to read).
#[derive(Debug, Default)]
pub struct StatsDb {
    /// Current stats.
    pub stats: LibraryStats,
    /// Backing file.
    pub path: Option<std::path::PathBuf>,
}

impl StatsDb {
    /// Load from `path` (absent/corrupt → empty, with corrupt reported).
    /// The path is always remembered so a later `save` works.
    pub fn load(path: &Path) -> (Self, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(stats) => (Self { stats, path: Some(path.into()) }, None),
                Err(e) => (Self { stats: LibraryStats::default(), path: Some(path.into()) }, Some(e.to_string())),
            },
            Err(_) => (Self { stats: LibraryStats::default(), path: Some(path.into()) }, None),
        }
    }

    /// Persist atomically (tmp + rename).
    pub fn save(&self) -> Result<(), String> {
        let path = self.path.clone().ok_or("no stats path")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&self.stats).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Toggle favorite; returns new state.
    pub fn toggle_favorite(&mut self, id: &str) -> bool {
        if let Some(i) = self.stats.favorites.iter().position(|f| f == id) {
            self.stats.favorites.remove(i);
            false
        } else {
            self.stats.favorites.push(id.into());
            true
        }
    }

    /// Record one finished session: `now` is unix seconds (injected for tests)
    /// and `secs` how long the emulator actually ran.
    ///
    /// The duration is what the shelf shows, so it is measured by whoever waited
    /// for the child — the CLI, which is the only place that spawns it.
    pub fn record_play(&mut self, id: &str, now: u64, secs: u64) {
        let e = self.stats.plays.entry(id.into()).or_default();
        e.plays += 1;
        e.played_secs += secs;
        e.last_played = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn favorites_and_plays_roundtrip() {
        let dir = std::env::temp_dir().join("rosadeck-stats-test");
        std::fs::remove_dir_all(&dir).ok();
        let path = dir.join("library.json");
        let (mut db, _) = StatsDb::load(&path);
        assert!(db.toggle_favorite("abc"));
        assert!(!db.toggle_favorite("abc"));
        db.toggle_favorite("abc");
        db.record_play("abc", 100, 45);
        db.record_play("abc", 200, 3_600);
        db.save().unwrap();
        let (db2, _) = StatsDb::load(&path);
        assert_eq!(db2.stats.favorites, vec!["abc"]);
        assert_eq!(db2.stats.plays["abc"].plays, 2);
        assert_eq!(db2.stats.plays["abc"].played_secs, 3_645, "el tiempo se acumula");
        assert_eq!(db2.stats.plays["abc"].last_played, 200);
        std::fs::remove_dir_all(&dir).ok();
    }
}
