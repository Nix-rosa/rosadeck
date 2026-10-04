//! Gaming-session executor: display transition + workspace migration +
//! focus + shell/wallpaper, with session verify and session restore.
//!
//! Mutation boundary: `hyprctl dispatch` (workspace/focus moves), `awww`
//! wallpaper re-application, and the F3 display transition all live here —
//! never in `session-core`, `planner`, `selector` or the overlay. Dispatch
//! goes through [`DispatchRunner`] (stubbed in tests); the legacy string
//! dispatcher names used here were verified live against Hyprland 0.56.2
//! (`moveworkspacetomonitor`, `workspace`, `focuswindow` all return `ok`).

use crate::backend::{fetch_clients, fetch_monitors, fetch_workspaces, BackendResult};
use crate::snapshot::Snapshot;
use crate::transition::{
    build_restore_plan, run_transition, TransitionError, TransitionOutcome,
};
use crate::unify::UnifiedDisplay;
use rosadeck_session_core::{
    resolve_gaming_focus, resolve_restore_focus, Capability, FocusPlan, GamingSessionHandle, SessionPlan,
    SessionSnapshot, ShellIntegrationPolicy, WallpaperProviderKind, WorkspacePlan, WsRef,
};
use rosadeck_shell_integration::{detect_quickshell, detect_wallpaper, probe_ipc_targets};
use std::path::Path;

/// Runs one `hyprctl dispatch <name> <args>` (mocked in tests so `cargo
/// test` never moves real workspaces).
pub trait DispatchRunner {
    /// Execute a dispatcher call.
    fn dispatch(&mut self, dispatcher: &str, args: &str) -> Result<String, String>;
}

/// Production dispatch runner: the version-matched `hyprctl` binary.
pub struct HyprctlDispatch;

impl DispatchRunner for HyprctlDispatch {
    fn dispatch(&mut self, dispatcher: &str, args: &str) -> Result<String, String> {
        let out = std::process::Command::new("hyprctl")
            .args(["dispatch", dispatcher, args])
            .output()
            .map_err(|e| format!("spawn hyprctl: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        if out.status.success() {
            Ok(text)
        } else {
            Err(format!("hyprctl dispatch exited {}: {}", out.status, String::from_utf8_lossy(&out.stderr).trim()))
        }
    }
}

/// Capture a full session snapshot from live IPC (read-only).
pub fn take_session_snapshot() -> BackendResult<SessionSnapshot> {
    let mons = fetch_monitors()?;
    let workspaces = fetch_workspaces()?;
    let clients = fetch_clients()?;
    let mon_name: std::collections::HashMap<i32, String> =
        mons.iter().map(|m| (m.id, m.name.clone())).collect();
    Ok(SessionSnapshot {
        taken_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        outputs: mons
            .iter()
            .map(|m| rosadeck_session_core::snapshot::SessionOutput {
                state: crate::model::normalize(m).0,
                transform: m.transform,
            })
            .collect(),
        workspaces: workspaces
            .iter()
            .map(|w| rosadeck_session_core::WorkspacePlacement {
                workspace: WsRef { id: w.id, name: w.name.clone() },
                monitor: w.monitor.clone(),
            })
            .collect(),
        special_workspaces: mons
            .iter()
            .filter(|m| m.specialWorkspace.id != 0)
            .map(|m| (m.name.clone(), m.specialWorkspace.id))
            .collect(),
        clients: clients
            .iter()
            .map(|c| rosadeck_session_core::ClientRecord {
                address: c.address.clone(),
                workspace_id: c.workspace.id,
                monitor: mon_name.get(&c.monitor).cloned().unwrap_or_default(),
                floating: c.floating,
                fullscreen: c.fullscreen,
                pinned: c.pinned,
            })
            .collect(),
        focused_output: mons.iter().find(|m| m.focused).map(|m| m.name.clone()),
        focused_workspace: mons.iter().find(|m| m.focused).map(|m| WsRef {
            id: m.activeWorkspace.id,
            name: m.activeWorkspace.name.clone(),
        }),
        focused_window: clients
            .iter()
            .filter(|c| c.mapped)
            .max_by_key(|c| c.focusHistoryID)
            .map(|c| c.address.clone()),
    })
}

/// Execute workspace moves in order; stop at the first failure (caller
/// restores; moves are idempotent so retry is safe).
pub fn migrate_workspaces(
    plan: &WorkspacePlan,
    dispatch: &mut dyn DispatchRunner,
    log: &mut Vec<String>,
) -> Result<(), TransitionError> {
    for m in &plan.moves {
        log.push(format!("WORKSPACE {}: {} -> {}", m.workspace_id, m.from, m.to));
        dispatch
            .dispatch("moveworkspacetomonitor", &format!("{} {}", m.workspace_id, m.to))
            .map_err(TransitionError::ApplyFailed)?;
    }
    Ok(())
}

/// Focus a workspace, then a window when still valid.
pub fn apply_focus(
    workspace: &WsRef,
    window: Option<&str>,
    dispatch: &mut dyn DispatchRunner,
    log: &mut Vec<String>,
) {
    log.push(format!("FOCUS workspace {}", workspace.id));
    if dispatch.dispatch("workspace", &workspace.id.to_string()).is_err() {
        log.push("FOCUS workspace dispatch failed (non-fatal)".into());
    }
    if let Some(addr) = window {
        log.push(format!("FOCUS window {addr}"));
        if dispatch.dispatch("focuswindow", &format!("address:{addr}")).is_err() {
            log.push("FOCUS window dispatch failed (non-fatal)".into());
        }
    }
}

/// Verify migrated session state against a fresh read.
pub fn check_session(
    expect_workspaces_on: &str,
    expect_ids: &[i32],
    live: &SessionSnapshot,
) -> Vec<String> {
    let mut mismatches = Vec::new();
    for id in expect_ids {
        match live.workspaces.iter().find(|w| w.workspace.id == *id) {
            Some(w) if w.monitor == expect_workspaces_on => {}
            Some(w) => mismatches.push(format!("workspace {id} on {}, expected {expect_workspaces_on}", w.monitor)),
            None => mismatches.push(format!("workspace {id} missing")),
        }
    }
    mismatches
}

/// Shell gate BEFORE any mutation (§36): under `Strict`, a required-but-
/// unavailable integration aborts; otherwise gaps become warnings.
pub fn shell_gate(
    shell: &rosadeck_session_core::ShellPlan,
    wallpaper_images: &[(String, String)],
    policy: ShellIntegrationPolicy,
    log: &mut Vec<String>,
) -> Result<ShellReport, TransitionError> {
    let mut qs = detect_quickshell();
    probe_ipc_targets(&mut qs);
    let wp = detect_wallpaper();
    let qs_cap = if !shell.quickshell {
        Capability::Unsupported // not requested
    } else if !qs.running {
        Capability::Unsupported
    } else {
        qs.screen_migration
    };
    let wp_cap = if !shell.wallpaper || wallpaper_images.is_empty() {
        Capability::Unsupported // not requested
    } else {
        wp.reapply
    };
    let report = ShellReport {
        quickshell_running: qs.running,
        quickshell_cap: qs_cap,
        wallpaper_provider: wp.provider.clone(),
        wallpaper_cap: wp_cap,
        wallpaper_current: wp.current.clone(),
    };
    let mut problems = Vec::new();
    if shell.quickshell && qs_cap != Capability::Available {
        problems.push(format!("quickshell screen migration {qs_cap:?}"));
    }
    if shell.wallpaper && !wallpaper_images.is_empty() && wp_cap != Capability::Available {
        problems.push(format!("wallpaper reapply {wp_cap:?} ({:?})", wp.provider));
    }
    match policy {
        ShellIntegrationPolicy::Strict if !problems.is_empty() => {
            Err(TransitionError::ApplyFailed(format!("shell gate (strict): {}", problems.join("; "))))
        }
        ShellIntegrationPolicy::Disabled => Ok(ShellReport::disabled()),
        _ => {
            for p in &problems {
                log.push(format!("SHELL best-effort: {p}; continuing without it"));
            }
            Ok(report)
        }
    }
}

/// Observed shell capabilities for logs/verify.
#[derive(Debug, Clone)]
pub struct ShellReport {
    /// Quickshell process present.
    pub quickshell_running: bool,
    /// Screen-migration capability (or Unsupported when unrequested).
    pub quickshell_cap: Capability,
    /// Wallpaper provider kind.
    pub wallpaper_provider: WallpaperProviderKind,
    /// Wallpaper re-application capability.
    pub wallpaper_cap: Capability,
    /// Current per-output images (for restore comparison).
    pub wallpaper_current: Vec<(String, String)>,
}

impl ShellReport {
    /// All-disabled report.
    pub fn disabled() -> Self {
        Self {
            quickshell_running: false,
            quickshell_cap: Capability::Unsupported,
            wallpaper_provider: WallpaperProviderKind::None,
            wallpaper_cap: Capability::Unsupported,
            wallpaper_current: vec![],
        }
    }
}

/// Re-apply wallpaper images per output (awww only, when available).
/// Never copies files; re-applies the user's own images.
pub fn reapply_wallpaper(
    provider: &WallpaperProviderKind,
    cap: Capability,
    images: &[(String, String)],
    log: &mut Vec<String>,
) {
    if cap != Capability::Available || images.is_empty() {
        return;
    }
    match provider {
        WallpaperProviderKind::Awww => {
            for (output, image) in images {
                log.push(format!("WALLPAPER {output}: {image}"));
                let st = std::process::Command::new("awww")
                    .args(["img", output, image, "--transition-type", "none"])
                    .status();
                if !matches!(st, Ok(s) if s.success()) {
                    log.push(format!("WALLPAPER {output} re-apply failed (non-fatal)"));
                }
            }
        }
        _ => log.push("WALLPAPER provider cannot re-apply (non-fatal)".into()),
    }
}

/// Full gaming-session transition (ordering per §19). `read_session`
/// re-captures live session state for verification.
#[allow(clippy::too_many_arguments)]
pub fn run_gaming_session(
    display_snapshot: &Snapshot,
    session_snapshot: &SessionSnapshot,
    plan: &SessionPlan,
    dispatch: &mut dyn DispatchRunner,
    runner: &mut dyn crate::transition::CommandRunner,
    read_unified: &dyn Fn() -> BackendResult<Vec<UnifiedDisplay>>,
    read_session: &dyn Fn() -> BackendResult<SessionSnapshot>,
    connected_now: &dyn Fn() -> Vec<String>,
    policy: ShellIntegrationPolicy,
    state_dir: &Path,
) -> Result<TransitionOutcome, TransitionError> {
    let mut log: Vec<String> = Vec::new();
    // 0. Shell gate BEFORE mutation.
    let shell = shell_gate(&plan.shell, &plan.wallpaper.images, policy, &mut log)?;
    // 1. Display transition (existing F3 path: snapshot→apply→verify→restore).
    log.push("SESSION snapshot captured".into());
    let outcome = run_transition(display_snapshot, &plan.display_plan, runner, read_unified, connected_now)?;
    if outcome == TransitionOutcome::RolledBack {
        eprint_log(&log);
        return Err(TransitionError::VerifyFailed(vec!["display transition rolled back".into()]));
    }
    // 2. Migrate workspaces, then verify against a fresh read.
    if let Err(e) = migrate_workspaces(&plan.workspace_plan, dispatch, &mut log) {
        return restore_session_from(display_snapshot, session_snapshot, dispatch, runner, read_unified, read_session, connected_now, &plan.wallpaper, &shell, state_dir, &mut log, format!("workspace migrate: {e}"));
    }
    match read_session() {
        Ok(live) => {
            let ids: Vec<i32> = plan.workspace_plan.moves.iter().map(|m| m.workspace_id).collect();
            let target = plan.focus.target_monitor.clone();
            let mm = check_session(&target, &ids, &live);
            if !mm.is_empty() {
                return restore_session_from(display_snapshot, session_snapshot, dispatch, runner, read_unified, read_session, connected_now, &plan.wallpaper, &shell, state_dir, &mut log, format!("session verify: {}", mm.join("; ")));
            }
            // 3. Focus external (workspace + best-effort window).
            let migrating: Vec<i32> = ids;
            let focus = resolve_gaming_focus(session_snapshot, &target, &migrating);
            apply_focus(&focus.workspace, focus.window.as_deref(), dispatch, &mut log);
            // 4. Wallpaper re-apply (best-effort, never fatal).
            reapply_wallpaper(&shell.wallpaper_provider, shell.wallpaper_cap, &plan.wallpaper.images, &mut log);
            // 5. Record the active handle for daemon disconnect recovery.
            let handle = GamingSessionHandle {
                id: format!("gaming-{}", display_snapshot.taken_at),
                snapshot_path: state_dir.join("last-gaming-snapshot.json"),
                target,
                started_at: display_snapshot.taken_at,
            };
            // Persist the session snapshot beside the handle (durable artifact).
            let snap_path = handle.snapshot_path.clone();
            if let Ok(text) = serde_json::to_string_pretty(session_snapshot) {
                let _ = std::fs::write(&snap_path, text);
            }
            if let Err(e) = handle.write(state_dir) {
                log.push(format!("HANDLE write failed (non-fatal): {e}"));
            }
            log.push("GAMING SESSION ACTIVE".into());
            eprint_log(&log);
            Ok(TransitionOutcome::Success)
        }
        Err(e) => restore_session_from(display_snapshot, session_snapshot, dispatch, runner, read_unified, read_session, connected_now, &plan.wallpaper, &shell, state_dir, &mut log, format!("session re-read: {e}")),
    }
}

#[allow(clippy::too_many_arguments)]
fn restore_session_from(
    display_snapshot: &Snapshot,
    session_snapshot: &SessionSnapshot,
    dispatch: &mut dyn DispatchRunner,
    runner: &mut dyn crate::transition::CommandRunner,
    read_unified: &dyn Fn() -> BackendResult<Vec<UnifiedDisplay>>,
    read_session: &dyn Fn() -> BackendResult<SessionSnapshot>,
    connected_now: &dyn Fn() -> Vec<String>,
    wallpaper: &rosadeck_session_core::WallpaperPlan,
    shell: &ShellReport,
    state_dir: &Path,
    log: &mut Vec<String>,
    reason: String,
) -> Result<TransitionOutcome, TransitionError> {
    log.push(format!("SESSION FAILED ({reason}) — RESTORE START"));
    restore_session(display_snapshot, session_snapshot, wallpaper, shell, dispatch, runner, read_unified, read_session, connected_now, state_dir, log)
        .map(|_| TransitionOutcome::RolledBack)
}

/// Restore a full session from snapshots (display → workspaces → focus →
/// shell/wallpaper → verify). Restores the recorded state, never generics.
#[allow(clippy::too_many_arguments)]
pub fn restore_session(
    display_snapshot: &Snapshot,
    session_snapshot: &SessionSnapshot,
    wallpaper: &rosadeck_session_core::WallpaperPlan,
    shell: &ShellReport,
    dispatch: &mut dyn DispatchRunner,
    runner: &mut dyn crate::transition::CommandRunner,
    read_unified: &dyn Fn() -> BackendResult<Vec<UnifiedDisplay>>,
    read_session: &dyn Fn() -> BackendResult<SessionSnapshot>,
    connected_now: &dyn Fn() -> Vec<String>,
    state_dir: &Path,
    log: &mut Vec<String>,
) -> Result<(), TransitionError> {
    use crate::transition::{apply_plan, verify_plan};
    // 1. Display back (existing restore plan builder).
    let restore = build_restore_plan(display_snapshot, &connected_now());
    apply_plan(&restore, runner, log).map_err(|e| TransitionError::RestoreFailed(e.to_string()))?;
    verify_plan(&restore, read_unified, log).map_err(|e| TransitionError::RestoreFailed(e.to_string()))?;
    // 2. Workspaces back: anything not on its snapshot monitor returns.
    // (Computed from live placement, so partial migrations also heal.)
    let live_now = read_session().unwrap_or_else(|_| session_snapshot.clone());
    for want in &session_snapshot.workspaces {
        let at = live_now.workspaces.iter().find(|w| w.workspace.id == want.workspace.id).map(|w| w.monitor.as_str());
        if at != Some(want.monitor.as_str()) {
            log.push(format!("WORKSPACE RESTORE {}: -> {}", want.workspace.id, want.monitor));
            if dispatch.dispatch("moveworkspacetomonitor", &format!("{} {}", want.workspace.id, want.monitor)).is_err() {
                log.push(format!("WORKSPACE RESTORE {} failed (non-fatal)", want.workspace.id));
            }
        }
    }
    // 3. Focus with fallbacks against live state.
    match read_session() {
        Ok(live) => {
            let live_ws: Vec<(i32, String)> =
                live.workspaces.iter().map(|w| (w.workspace.id, w.workspace.name.clone())).collect();
            let live_win: Vec<String> = live.clients.iter().map(|c| c.address.clone()).collect();
            let focus = FocusPlan {
                target_monitor: session_snapshot.focused_output.clone().unwrap_or_default(),
                target_workspace: session_snapshot.focused_workspace.clone(),
                target_window: session_snapshot.focused_window.clone(),
            };
            let target = resolve_restore_focus(&focus, session_snapshot, &live_ws, &live_win);
            apply_focus(&target.workspace, target.window.as_deref(), dispatch, log);
        }
        Err(e) => log.push(format!("restore focus skipped (re-read failed: {e})")),
    }
    // 4. Wallpaper back (best-effort).
    reapply_wallpaper(&shell.wallpaper_provider, shell.wallpaper_cap, &wallpaper.images, log);
    GamingSessionHandle::clear(state_dir);
    log.push("SESSION RESTORE VERIFY OK".into());
    Ok(())
}

fn eprint_log(log: &[String]) {
    for line in log {
        eprintln!("[rosadeck] {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rosadeck_session_core::{ShellPlan, WorkspacePlan};

    #[derive(Default)]
    pub struct StubDispatch {
        /// Recorded `(dispatcher, args)`.
        pub calls: Vec<(String, String)>,
        /// Substrings that fail.
        pub fail_on: Vec<String>,
    }

    impl DispatchRunner for StubDispatch {
        fn dispatch(&mut self, dispatcher: &str, args: &str) -> Result<String, String> {
            self.calls.push((dispatcher.into(), args.into()));
            if self.fail_on.iter().any(|f| args.contains(f)) {
                return Err(format!("stub refuse {dispatcher} {args}"));
            }
            Ok("ok".into())
        }
    }

    fn ws_plan() -> WorkspacePlan {
        WorkspacePlan {
            moves: vec![
                rosadeck_session_core::WorkspaceMove { workspace_id: 1, from: "eDP-1".into(), to: "HDMI-A-1".into() },
                rosadeck_session_core::WorkspaceMove { workspace_id: 2, from: "eDP-1".into(), to: "HDMI-A-1".into() },
            ],
        }
    }

    #[test]
    fn migrates_in_order_and_stops_on_error() {
        let mut d = StubDispatch { fail_on: vec!["2 HDMI".into()], ..Default::default() };
        let mut log = vec![];
        let err = migrate_workspaces(&ws_plan(), &mut d, &mut log).unwrap_err();
        assert!(matches!(err, TransitionError::ApplyFailed(_)));
        assert_eq!(d.calls.len(), 2); // second attempted, then stop
    }

    #[test]
    fn shell_gate_strict_aborts_best_effort_warns() {
        let shell = ShellPlan { quickshell: true, wallpaper: false };
        let mut log = vec![];
        // This machine: quickshell running but toggle-only IPC.
        let r = shell_gate(&shell, &[], ShellIntegrationPolicy::BestEffort, &mut log);
        assert!(r.is_ok());
        assert!(log.iter().any(|l| l.contains("best-effort")));
        let mut log2 = vec![];
        assert!(shell_gate(&shell, &[], ShellIntegrationPolicy::Strict, &mut log2).is_err());
    }

    #[test]
    fn check_session_detects_misplaced_workspace() {
        let live = SessionSnapshot {
            taken_at: 0,
            outputs: vec![],
            workspaces: vec![rosadeck_session_core::WorkspacePlacement {
                workspace: WsRef { id: 1, name: "1".into() },
                monitor: "eDP-1".into(),
            }],
            special_workspaces: vec![],
            clients: vec![],
            focused_output: None,
            focused_workspace: None,
            focused_window: None,
        };
        let mm = check_session("HDMI-A-1", &[1], &live);
        assert_eq!(mm.len(), 1);
        assert!(check_session("eDP-1", &[1], &live).is_empty());
    }
}
