//! `rosadeck-backend-hyprland`: the only crate that talks to Hyprland.
//!
//! Read-only in F2: socket query (`monitors`), socket2 event watch, and
//! reconciliation with DRM/EDID/CTA into [`UnifiedDisplay`]. No `keyword`,
//! no `--batch`, no writes of any kind.

pub mod backend;
pub mod events;
pub mod model;
pub mod session;
pub mod snapshot;
pub mod transition;
pub mod unify;

pub use backend::{fetch_clients, fetch_monitors, fetch_workspaces, BackendError, BackendResult, ClientRaw, DisplayBackend, HyprlandBackend, WorkspaceId, WorkspaceRaw};
pub use events::{parse_event_line, watch_events, DisplayEvent};
pub use model::{normalize, ActiveWs, HyprMonitor, HyprMonitorInfo};
pub use session::{
    apply_focus, check_session, migrate_workspaces, reapply_wallpaper, restore_session, run_gaming_session,
    shell_gate, take_session_snapshot, DispatchRunner, HyprctlDispatch, ShellReport,
};
pub use snapshot::{load_last_snapshot, save_snapshot, take_snapshot, Snapshot, SnapshotOutput, WorkspaceRef, state_dir};
pub use transition::{
    apply_plan, build_restore_plan, exit_code_for, run_transition, verify_plan, CommandRunner, HyprctlRunner, TransitionError,
    TransitionOutcome, VERIFY_ATTEMPTS, VERIFY_INTERVAL_MS, VERIFY_TIMEOUT_MS,
};
pub use unify::{connected_connector_names, unify, unify_with, UnifiedDisplay, DEFAULT_SYSFS};
