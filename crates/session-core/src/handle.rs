//! Gaming-session handle: which session is active (for the daemon).
//!
//! Written when a gaming session starts, removed on clean exit. The daemon
//! uses it to distinguish "active gaming session lost its output" (restore
//! the *session*) from a plain display change (restore the *display*).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Active gaming session handle (one JSON file in the state dir).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GamingSessionHandle {
    /// Unique id (`gaming-<unix>`).
    pub id: String,
    /// Snapshot the session was built from.
    pub snapshot_path: PathBuf,
    /// External target output.
    pub target: String,
    /// Unix seconds at start.
    pub started_at: u64,
}

impl GamingSessionHandle {
    /// Handle file path in `state_dir`.
    pub fn path(state_dir: &Path) -> PathBuf {
        state_dir.join("gaming-session.json")
    }

    /// Write atomically (tmp + rename; no fsync needed for a hint file —
    /// the snapshot itself is the durable artifact).
    pub fn write(&self, state_dir: &Path) -> Result<(), String> {
        let dest = Self::path(state_dir);
        let tmp = state_dir.join(".gaming-session.json.tmp");
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Read when present (`None` = no active gaming session).
    pub fn read(state_dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(Self::path(state_dir)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Remove (clean session exit).
    pub fn clear(state_dir: &Path) {
        let _ = std::fs::remove_file(Self::path(state_dir));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_read_clear_roundtrip() {
        let dir = std::env::temp_dir().join("rosadeck-handle-test");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(GamingSessionHandle::read(&dir).is_none());
        let h = GamingSessionHandle {
            id: "gaming-1".into(),
            snapshot_path: PathBuf::from("/tmp/snap.json"),
            target: "HDMI-A-1".into(),
            started_at: 1,
        };
        h.write(&dir).unwrap();
        assert_eq!(GamingSessionHandle::read(&dir).unwrap().target, "HDMI-A-1");
        GamingSessionHandle::clear(&dir);
        assert!(GamingSessionHandle::read(&dir).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
