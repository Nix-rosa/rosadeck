//! Minimal game target: what to run, without a game library.
//!
//! No filesystem scanning, no artwork, no scraping in F5. `path` shape is
//! validated in `prepare`; existence is only observed at spawn time (so
//! `PreparedLaunch` stays fully inspectable without files).

use serde::{Deserialize, Serialize};

/// A game to launch (platform + path + optional title).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameTarget {
    /// Platform label (`wii`, `snes`, …; free-form in F5).
    pub platform: String,
    /// Game file path (as given; existence checked at spawn, not prepare).
    pub path: String,
    /// Human title, when known.
    pub title: Option<String>,
}

impl GameTarget {
    /// Shape validation (non-empty platform/path).
    pub fn validate(&self) -> Result<(), String> {
        if self.platform.trim().is_empty() {
            return Err("game platform is empty".into());
        }
        if self.path.trim().is_empty() {
            return Err("game path is empty".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_shape_only() {
        let g = GameTarget { platform: "wii".into(), path: "/games/mk.wbfs".into(), title: Some("Mario Kart Wii".into()) };
        assert!(g.validate().is_ok());
        assert!(GameTarget { platform: "".into(), path: "x".into(), title: None }.validate().is_err());
        assert!(GameTarget { platform: "wii".into(), path: "".into(), title: None }.validate().is_err());
    }
}
