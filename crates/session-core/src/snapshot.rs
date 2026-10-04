//! Full session snapshot shape: outputs + workspaces + clients + focus.
//!
//! Identity uses stable keys (`address`, workspace id, monitor name) —
//! never window titles. Only the fields needed for migration/restore are
//! kept; pixels, PIDs and titles are deliberately excluded.

use display_core::OutputState;
use serde::{Deserialize, Serialize};

/// Workspace reference (id is the stable key; name kept for logs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WsRef {
    /// Workspace id.
    pub id: i32,
    /// Workspace name.
    pub name: String,
}

/// Where a workspace lived at capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacePlacement {
    /// Workspace reference.
    pub workspace: WsRef,
    /// Hosting monitor name.
    pub monitor: String,
}

/// One client/window record (migration-relevant flags only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientRecord {
    /// Stable window address (`0x…`).
    pub address: String,
    /// Hosting workspace id.
    pub workspace_id: i32,
    /// Hosting monitor name.
    pub monitor: String,
    /// Floating window.
    pub floating: bool,
    /// Fullscreen mode (0 = none, as reported).
    pub fullscreen: i32,
    /// Pinned window.
    pub pinned: bool,
}

/// Output entry: restorable display state (mirror of backend snapshot entry,
// re-declared here so session-core stays backend-independent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionOutput {
    /// Core display state.
    pub state: OutputState,
    /// Transform at capture.
    pub transform: i32,
}

/// Complete restorable desktop session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Unix seconds at capture.
    pub taken_at: u64,
    /// Outputs (enabled + disabled).
    pub outputs: Vec<SessionOutput>,
    /// Workspace placement.
    pub workspaces: Vec<WorkspacePlacement>,
    /// Special workspace per monitor (`monitor → special id`, when active).
    pub special_workspaces: Vec<(String, i32)>,
    /// Client records.
    pub clients: Vec<ClientRecord>,
    /// Focused output, when any.
    pub focused_output: Option<String>,
    /// Focused workspace, when any.
    pub focused_workspace: Option<WsRef>,
    /// Focused window address, when any.
    pub focused_window: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::sample_snapshot;

    #[test]
    fn snapshot_roundtrips_without_titles() {
        let s = sample_snapshot();
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("title"));
        let back: SessionSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back.workspaces.len(), 3);
    }
}
