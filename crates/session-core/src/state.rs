//! Daemon session states, shell policies and capability states.
//!
//! `GamingSession → GamingSession` without an explicit exit is rejected by
//! the daemon (second entry while a handle exists = already satisfied).

use serde::{Deserialize, Serialize};

/// Extended daemon session awareness (F4 states plus gaming).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DaemonSessionState {
    /// Plain desktop, no session.
    NormalDesktop,
    /// Selector open.
    DisplayMenu,
    /// Display-only transition running.
    DisplayTransition,
    /// External gaming session active.
    GamingSession,
    /// Session restore running.
    RestoringSession,
    /// Noted error (message preserved).
    Error(String),
}

impl DaemonSessionState {
    /// Reject entering `GamingSession` while already inside one.
    pub fn enter_gaming(&self) -> Result<DaemonSessionState, String> {
        match self {
            Self::GamingSession => Err("gaming session already active".into()),
            _ => Ok(DaemonSessionState::GamingSession),
        }
    }
}

/// Shell integration policy (default decided after probing real shells).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShellIntegrationPolicy {
    /// Shell failure aborts before any mutation.
    Strict,
    /// Display/session succeed; shell gaps become warnings.
    BestEffort,
    /// Never touch shell integration.
    Disabled,
}

/// Capability of one shell integration point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Capability {
    /// Probed and usable.
    Available,
    /// Present but not configured for this (e.g. no IPC handler).
    NotConfigured,
    /// Not present / cannot work here.
    Unsupported,
}

/// Detected wallpaper provider kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WallpaperProviderKind {
    /// `awww` daemon with per-output control.
    Awww,
    /// `hyprpaper` via `hyprctl hyprpaper`.
    Hyprpaper,
    /// Managed inside the shell (provider-specific path).
    ShellManaged,
    /// Nothing detected.
    None,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_nested_gaming_sessions() {
        let s = DaemonSessionState::GamingSession;
        assert!(s.enter_gaming().is_err());
        assert_eq!(DaemonSessionState::NormalDesktop.enter_gaming().unwrap(), DaemonSessionState::GamingSession);
    }
}
