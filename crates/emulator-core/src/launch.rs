//! `LaunchContext` (small explicit inputs) and `PreparedLaunch`
//! (inspectable description of exactly what would be executed).

use crate::game::GameTarget;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Everything an adapter needs to describe a launch. Small on purpose:
/// game + emulator identity + optional display request summary + extras.
/// Display planning itself stays in `planner`/backend (never here).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchContext {
    /// What to run.
    pub game: GameTarget,
    /// Adapter id (`retroarch`, `dolphin`, …).
    pub emulator_id: String,
    /// Requested display disposition label for notes (e.g. `external-only`);
    /// the actual mode/plan is resolved by the display subsystem.
    pub display_mode_label: Option<String>,
    /// Requested display mode label for notes (e.g. `1920x1080@120`).
    pub display_mode_requested: Option<String>,
    /// Resolved shader preset path, if any (resolved by shader-manager).
    pub shader_path: Option<String>,
    /// Extra environment entries from the emulator profile.
    pub extra_env: Vec<(String, String)>,
    /// Extra CLI args from the emulator profile (appended after adapter args).
    pub extra_args: Vec<String>,
    /// Working directory from the profile, if any.
    pub workdir: Option<PathBuf>,
}

/// Exact description of one execution: program, args, env, workdir.
/// Printable and testable without spawning anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedLaunch {
    /// Resolved executable (absolute when found; profile name otherwise).
    pub program: String,
    /// Full argv (excluding argv[0]).
    pub args: Vec<String>,
    /// Full environment additions (inherited env is untouched).
    pub env: Vec<(String, String)>,
    /// Working directory, if any.
    pub workdir: Option<PathBuf>,
    /// Human notes (requirements, fallbacks, unmapped options).
    pub notes: Vec<String>,
}

impl PreparedLaunch {
    /// One-line summary for `--dry-run` output.
    pub fn summary(&self) -> String {
        format!("{} {}", self.program, self.args.join(" "))
    }
}
