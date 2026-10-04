//! Pure session planning: workspace moves + full `SessionPlan`.
//!
//! Workspace moves carry `from`/`to` so restore inverts them exactly.
//! Non-consecutive IDs are fine; workspaces already on the target never move.

use crate::snapshot::{SessionSnapshot, WsRef};
use rosadeck_planner::DisplayPlan;
use serde::{Deserialize, Serialize};

/// One workspace migration step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceMove {
    /// Workspace id.
    pub workspace_id: i32,
    /// Source monitor.
    pub from: String,
    /// Target monitor.
    pub to: String,
}

/// Ordered workspace migration (empty = nothing to move).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacePlan {
    /// Moves in execution order (sorted by workspace id, deterministic).
    pub moves: Vec<WorkspaceMove>,
}

/// Shell integration intent (capabilities resolved at execution time).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellPlan {
    /// Attempt Quickshell screen migration when available.
    pub quickshell: bool,
    /// Attempt wallpaper re-application when available.
    pub wallpaper: bool,
}

/// Wallpaper intent: re-apply these images per output when the provider
/// allows it (no copies, no originals touched).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WallpaperPlan {
    /// `(output, image path)` pairs to re-apply.
    pub images: Vec<(String, String)>,
}

/// Complete gaming-session plan (pure, deterministic, testable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionPlan {
    /// Display transition (planner output).
    pub display_plan: DisplayPlan,
    /// Workspace migration.
    pub workspace_plan: WorkspacePlan,
    /// Focus intent (resolved against live state at execution).
    pub focus: FocusPlan,
    /// Shell intent.
    pub shell: ShellPlan,
    /// Wallpaper intent.
    pub wallpaper: WallpaperPlan,
}

/// Focus intent (workspace/window resolved with fallbacks at execution).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusPlan {
    /// Focus this monitor after migration.
    pub target_monitor: String,
    /// Prefer this workspace (fallback chain applies).
    pub target_workspace: Option<WsRef>,
    /// Prefer this window address (fallback chain applies).
    pub target_window: Option<String>,
}

/// Build the migration of every workspace hosted on `source` to `target`.
/// Workspaces already on `target` (or elsewhere) are left alone.
pub fn build_workspace_plan(snapshot: &SessionSnapshot, source: &str, target: &str) -> WorkspacePlan {
    let mut moves: Vec<WorkspaceMove> = snapshot
        .workspaces
        .iter()
        .filter(|w| w.monitor == source)
        .map(|w| WorkspaceMove { workspace_id: w.workspace.id, from: source.into(), to: target.into() })
        .collect();
    moves.sort_by_key(|m| m.workspace_id);
    moves.dedup_by_key(|m| m.workspace_id);
    WorkspacePlan { moves }
}

/// Invert a workspace plan for restore (exact reversal).
pub fn invert_workspace_plan(plan: &WorkspacePlan) -> WorkspacePlan {
    let mut moves: Vec<WorkspaceMove> = plan
        .moves
        .iter()
        .map(|m| WorkspaceMove { workspace_id: m.workspace_id, from: m.to.clone(), to: m.from.clone() })
        .collect();
    moves.sort_by_key(|m| m.workspace_id);
    WorkspacePlan { moves }
}

/// Assemble a full session plan from its parts (no IO).
pub fn build_session_plan(
    display_plan: DisplayPlan,
    workspace_plan: WorkspacePlan,
    focus: FocusPlan,
    shell: ShellPlan,
    wallpaper: WallpaperPlan,
) -> SessionPlan {
    SessionPlan { display_plan, workspace_plan, focus, shell, wallpaper }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::sample_snapshot;

    #[test]
    fn moves_all_source_workspaces_sorted() {
        let plan = build_workspace_plan(&sample_snapshot(), "eDP-1", "HDMI-A-1");
        assert_eq!(plan.moves.iter().map(|m| m.workspace_id).collect::<Vec<_>>(), vec![1, 2, 5]);
        assert!(plan.moves.iter().all(|m| m.from == "eDP-1" && m.to == "HDMI-A-1"));
    }

    #[test]
    fn skips_workspaces_not_on_source() {
        let plan = build_workspace_plan(&sample_snapshot(), "HDMI-A-1", "eDP-1");
        assert!(plan.moves.is_empty());
    }

    #[test]
    fn inversion_restores_origins() {
        let fwd = build_workspace_plan(&sample_snapshot(), "eDP-1", "HDMI-A-1");
        let back = invert_workspace_plan(&fwd);
        assert!(back.moves.iter().all(|m| m.from == "HDMI-A-1" && m.to == "eDP-1"));
        assert_eq!(back.moves.len(), 3);
    }
}
