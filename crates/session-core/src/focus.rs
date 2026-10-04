//! Focus resolution with graceful fallbacks.
//!
//! Restore order: recorded window (if still present) → recorded workspace
//! (if it still exists) → first valid workspace. A closed window never fails
//! the whole restore.

use crate::plan::FocusPlan;
use crate::snapshot::{SessionSnapshot, WsRef};

/// Resolved focus target (all entries verified against live state).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusTarget {
    /// Workspace to focus.
    pub workspace: WsRef,
    /// Window address to focus, when still present.
    pub window: Option<String>,
}

/// Resolve the gaming focus: focus the target monitor's workspace.
/// Prefers the snapshot's focused workspace when it is migrating too,
// otherwise the first workspace heading to the target.
pub fn resolve_gaming_focus(snapshot: &SessionSnapshot, _target_monitor: &str, migrating: &[i32]) -> FocusTarget {
    let fallback = migrating
        .iter()
        .filter_map(|id| snapshot.workspaces.iter().find(|w| w.workspace.id == *id))
        .next()
        .map(|w| w.workspace.clone())
        .unwrap_or(WsRef { id: 1, name: "1".into() });
    let workspace = match &snapshot.focused_workspace {
        Some(fw) if migrating.contains(&fw.id) => fw.clone(),
        _ => fallback,
    };
    FocusTarget { workspace, window: None }
}

/// Resolve restore focus against live state:
/// `live_workspaces`/`live_windows` are `(id/name)` and address lists.
pub fn resolve_restore_focus(
    plan: &FocusPlan,
    snapshot: &SessionSnapshot,
    live_workspaces: &[(i32, String)],
    live_windows: &[String],
) -> FocusTarget {
    // 1. Recorded window still present.
    if let Some(addr) = plan.target_window.as_ref().or(snapshot.focused_window.as_ref()) {
        if live_windows.iter().any(|a| a == addr) {
            let ws = snapshot
                .clients
                .iter()
                .find(|c| &c.address == addr)
                .and_then(|c| snapshot.workspaces.iter().find(|w| w.workspace.id == c.workspace_id))
                .map(|w| w.workspace.clone());
            if let Some(ws) = ws {
                return FocusTarget { workspace: ws, window: Some(addr.clone()) };
            }
        }
    }
    // 2. Recorded workspace still exists.
    let want = plan.target_workspace.as_ref().or(snapshot.focused_workspace.as_ref());
    if let Some(w) = want {
        if live_workspaces.iter().any(|(id, _)| id == &w.id) {
            return FocusTarget { workspace: w.clone(), window: None };
        }
    }
    // 3. First valid workspace.
    let ws = live_workspaces
        .first()
        .map(|(id, name)| WsRef { id: *id, name: name.clone() })
        .unwrap_or(WsRef { id: 1, name: "1".into() });
    FocusTarget { workspace: ws, window: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::FocusPlan;
    use crate::fixtures::sample_snapshot;

    #[test]
    fn gaming_focus_prefers_migrating_focused_workspace() {
        let t = resolve_gaming_focus(&sample_snapshot(), "HDMI-A-1", &[1, 2, 5]);
        assert_eq!(t.workspace.id, 2); // snapshot focus was ws 2
        let t2 = resolve_gaming_focus(&sample_snapshot(), "HDMI-A-1", &[1, 5]);
        assert_eq!(t2.workspace.id, 1); // ws 2 not migrating → first migrating
    }

    #[test]
    fn restore_focus_fallback_chain() {
        let snap = sample_snapshot();
        let plan = FocusPlan { target_monitor: "eDP-1".into(), target_workspace: None, target_window: None };
        // Window gone, workspace present → workspace, no window.
        let t = resolve_restore_focus(&plan, &snap, &[(2, "2".into())], &[]);
        assert_eq!((t.workspace.id, t.window), (2, None));
        // Both gone → first valid.
        let t2 = resolve_restore_focus(&plan, &snap, &[(9, "9".into())], &[]);
        assert_eq!(t2.workspace.id, 9);
        // Window present → window + its workspace.
        let t3 = resolve_restore_focus(&plan, &snap, &[(2, "2".into())], &["0xabc".into()]);
        assert_eq!(t3.window.as_deref(), Some("0xabc"));
    }
}
