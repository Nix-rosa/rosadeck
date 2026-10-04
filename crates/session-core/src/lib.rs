//! `rosadeck-session-core`: pure desktop-session migration model.
//!
//! Display configuration (planner/backend) stays separate from desktop
//! session configuration (this crate). Everything here is pure data +
//! deterministic planning: `SessionSnapshot` capture shape, `WorkspacePlan`,
//! `FocusPlan`, `SessionPlan`, daemon session states, shell policies and the
//! gaming-session handle. Execution lives in `backend-hyprland`.

pub mod focus;
pub mod handle;
pub mod plan;
pub mod snapshot;
pub mod state;

pub use focus::{resolve_gaming_focus, resolve_restore_focus, FocusTarget};
pub use handle::GamingSessionHandle;
pub use plan::{
    build_session_plan, build_workspace_plan, invert_workspace_plan, FocusPlan, SessionPlan, ShellPlan, WallpaperPlan,
    WorkspaceMove, WorkspacePlan,
};
pub use snapshot::{ClientRecord, SessionSnapshot, WorkspacePlacement, WsRef};
pub use state::{Capability, DaemonSessionState, ShellIntegrationPolicy, WallpaperProviderKind};

/// Shared fixtures for unit tests (never compiled into the library).
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use display_core::{Mirror, OutputId, OutputState, Position, Scale};

    pub(crate) fn sample_snapshot() -> SessionSnapshot {
        SessionSnapshot {
            taken_at: 1,
            outputs: vec![snapshot::SessionOutput {
                state: OutputState {
                    id: OutputId("eDP-1".into()),
                    connector: None,
                    mode: None,
                    position: Position { x: 0, y: 0 },
                    scale: Scale(1.0),
                    disabled: false,
                    mirror: Mirror::Disabled,
                },
                transform: 0,
            }],
            workspaces: vec![
                WorkspacePlacement { workspace: WsRef { id: 1, name: "1".into() }, monitor: "eDP-1".into() },
                WorkspacePlacement { workspace: WsRef { id: 2, name: "2".into() }, monitor: "eDP-1".into() },
                WorkspacePlacement { workspace: WsRef { id: 5, name: "5".into() }, monitor: "eDP-1".into() },
            ],
            special_workspaces: vec![],
            clients: vec![ClientRecord {
                address: "0xabc".into(),
                workspace_id: 2,
                monitor: "eDP-1".into(),
                floating: true,
                fullscreen: 0,
                pinned: false,
            }],
            focused_output: Some("eDP-1".into()),
            focused_workspace: Some(WsRef { id: 2, name: "2".into() }),
            focused_window: Some("0xabc".into()),
        }
    }
}
