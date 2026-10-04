//! Session commands: `menu`, `gaming`, `session-restore`.
//!
//! `menu` opens the overlay (full menu with HDMI, state-only without).
//! `gaming` runs the full external-gaming session pipeline. `session-restore`
//! restores a gaming session from its handle. Display mutations always flow
//! through the F3 backend; workspace/focus moves through dispatch runners.

use display_core::{ModePolicy, OutputId};
use rosadeck_backend_hyprland::{
    connected_connector_names, load_last_snapshot, run_gaming_session, save_snapshot, state_dir,
    take_session_snapshot, take_snapshot, unify, exit_code_for, HyprctlDispatch, HyprctlRunner, UnifiedDisplay,
    DEFAULT_SYSFS,
};
use rosadeck_session_core::{GamingSessionHandle, ShellIntegrationPolicy};
use rosadeck_planner::{plan, DisplayRequest, PlanKind, parse_requested_mode};
use rosadeck_selector::{build_context, selection_to_request, ContextArgs, Selection, SelectionContext};
use rosadeck_session_core::{build_session_plan, FocusPlan, ShellPlan, WallpaperPlan};
use std::path::Path;

/// Selection wait for the overlay result file.
const SELECTION_TIMEOUT_S: u64 = 180;

/// Open the overlay for `ctx` via a floating terminal; return the confirmed
/// `Selection` (`None` = cancel/timeout). Presentation edge (duplicates the
/// daemon helper; F7 dedup noted).
fn open_overlay(ctx: &SelectionContext, wofi: bool) -> Option<Selection> {
    let dir = state_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let ctx_path = dir.join(format!("menu-{stamp}.json"));
    let res_path = dir.join(format!("menu-result-{stamp}.json"));
    std::fs::write(&ctx_path, serde_json::to_string_pretty(ctx).ok()?).ok()?;
    let overlay = sibling_binary("rosadeck-overlay");
    if wofi {
        // wofi draws its own surface: no terminal needed.
        return match std::process::Command::new(&overlay)
            .args(["--wofi", "--context", &ctx_path.to_string_lossy(), "--result", &res_path.to_string_lossy()])
            .status()
        {
            Ok(s) if s.success() => wait_result(&res_path),
            _ => None, // exit 3 / error = cancel
        };
    }
    match std::process::Command::new("kitty")
        .args(["--class", "rosadeck-overlay", "--", &overlay, "--context", &ctx_path.to_string_lossy(), "--result", &res_path.to_string_lossy()])
        .status()
    {
        Ok(s) if s.success() => {}
        Ok(s) => {
            eprintln!("rosadeck: overlay terminal exited {s} (need kitty?)");
            return None;
        }
        Err(e) => {
            eprintln!("rosadeck: cannot spawn kitty: {e}");
            return None;
        }
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(SELECTION_TIMEOUT_S);
    while std::time::Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(&res_path) {
            return serde_json::from_str(&text).ok();
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    eprintln!("rosadeck: selector timed out");
    None
}

/// Poll an overlay result file (shared by kitty/wofi paths).
fn wait_result(res_path: &std::path::Path) -> Option<Selection> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(SELECTION_TIMEOUT_S);
    while std::time::Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(res_path) {
            if let Ok(sel) = serde_json::from_str(&text) {
                return Some(sel);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    None
}

/// Sibling binary next to this executable, else PATH name.
fn sibling_binary(name: &str) -> String {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join(name);
            if p.exists() {
                return p.to_string_lossy().into_owned();
            }
        }
    }
    name.to_owned()
}

/// Hyprland-accepted modes of one unified display.
fn hyprland_modes(d: &UnifiedDisplay) -> Vec<display_core::Mode> {
    d.modes.iter().filter(|m| m.sources.contains(&display_core::ModeSource::Hyprland)).map(|m| m.mode).collect()
}

/// `rosadeck menu [--gaming]`: overlay menu (works with and without HDMI).
pub fn cmd_menu(gaming_only: bool, json: bool, wofi: bool) -> i32 {
    let live = match unify(Path::new(DEFAULT_SYSFS)) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("rosadeck: backend: {e}");
            return 2;
        }
    };
    let external = live.iter().find(|d| d.drm_connected && !rosadeck_planner::is_internal(&d.connector.0));
    let Some(ext) = external else {
        // §31 case B: no external — show state, menu stays open on Cancel-only.
        if json {
            println!("{}", serde_json::json!({"external": null, "outputs": live.len()}));
            return 0;
        }
        println!("No external display detected.");
        for d in &live {
            if let Some(o) = &d.output {
                if let Some(m) = o.mode {
                    println!("  {} {}x{} @ {:.3} Hz", d.connector.0, m.width, m.height, m.hz);
                }
            }
        }
        let ctx = SelectionContext {
            connector: String::new(),
            title: "No external display".into(),
            subtitle: "connect HDMI to unlock gaming options".into(),
            actions: vec![rosadeck_selector::SelectorAction::Cancel],
            profile: None,
            modes_per_action: vec![vec![]],
        };
        open_overlay(&ctx, wofi);
        return 0;
    };
    let internal_modes =
        live.iter().find(|d| rosadeck_planner::is_internal(&d.connector.0)).map(hyprland_modes).unwrap_or_default();
    let ctx = build_context(&ContextArgs {
        connector: OutputId(ext.connector.0.clone()),
        edid_name: ext.edid_name.clone(),
        identity: ext.identity.clone(),
        current: ext.output.as_ref().and_then(|o| o.mode),
        internal_modes,
        external_modes: hyprland_modes(ext),
        profile: None,
    });
    let ctx = if gaming_only {
        // Direct gaming context: ExternalGaming (when offered) + Cancel.
        let keep: Vec<usize> = ctx
            .actions
            .iter()
            .enumerate()
            .filter(|(_, a)| **a == rosadeck_selector::SelectorAction::ExternalGaming || **a == rosadeck_selector::SelectorAction::Cancel)
            .map(|(i, _)| i)
            .collect();
        SelectionContext {
            actions: keep.iter().map(|&i| ctx.actions[i]).collect(),
            modes_per_action: keep.iter().map(|&i| ctx.modes_per_action[i].clone()).collect(),
            ..ctx
        }
    } else {
        ctx
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&ctx).unwrap_or_default());
        return 0;
    }
    let Some(sel) = open_overlay(&ctx, wofi) else {
        println!("Cancelled.");
        return 0;
    };
    dispatch_selection(&ctx, &sel)
}

/// Route an overlay selection to the matching flow.
///
/// The overlay selection IS the confirmation (`yes = true`): the user already
/// chose explicitly, and there is no TTY to prompt on when launched from a
/// keybind. Direct CLI invocations without `--yes` still prompt.
fn dispatch_selection(ctx: &SelectionContext, sel: &Selection) -> i32 {
    use rosadeck_planner::PlanKind;
    use rosadeck_selector::SelectorAction;
    if sel.action == SelectorAction::Cancel {
        return 0;
    }
    let Some(idx) = ctx.actions.iter().position(|a| *a == sel.action) else { return 2 };
    let req = match selection_to_request(ctx, idx, sel, None) {
        Some(r) => r,
        None => {
            eprintln!("rosadeck: INVALID selection");
            return 11;
        }
    };
    if sel.action == SelectorAction::ExternalGaming {
        return cmd_gaming_with_policy(&req, ShellIntegrationPolicy::BestEffort, false, false, true);
    }
    let kind = match req.kind {
        PlanKind::Duplicate => PlanKind::Duplicate,
        PlanKind::ExternalOnly => PlanKind::ExternalOnly,
        PlanKind::Extend => PlanKind::Extend,
    };
    let (policy, mode) = match &req.policy {
        display_core::ModePolicy::Manual(m) => ("manual".to_owned(), Some(format!("{}x{}@{}", m.width, m.height, m.hz))),
        display_core::ModePolicy::MaxResolution => ("max-resolution".to_owned(), None),
        display_core::ModePolicy::Balanced => ("balanced".to_owned(), None),
        display_core::ModePolicy::MatchInternal => ("match-internal".to_owned(), None),
        display_core::ModePolicy::MatchExternal => ("match-external".to_owned(), None),
        display_core::ModePolicy::MaxRefresh => ("max-refresh".to_owned(), None),
    };
    super::run_mutating(kind, None, policy, mode, Some(ctx.connector.clone()), false, false, true)
}

/// `rosadeck gaming [--on O] [--mode M] [--policy P] [--shell-policy S] [--dry-run] [--json] [--yes]`.
#[allow(clippy::too_many_arguments)]
pub fn cmd_gaming(
    on: Option<String>,
    mode: Option<String>,
    policy: Option<String>,
    shell_policy: Option<String>,
    dry_run: bool,
    json: bool,
    yes: bool,
) -> i32 {
    let requested = match mode {
        Some(s) => match parse_requested_mode(&s) {
            Some(m) => Some(m),
            None => {
                eprintln!("rosadeck: INVALID_PLAN: bad --mode {s}");
                return 11;
            }
        },
        None => None,
    };
    let mut pol = match policy.as_deref().unwrap_or("max-refresh") {
        "max-resolution" => ModePolicy::MaxResolution,
        "max-refresh" => ModePolicy::MaxRefresh,
        "balanced" => ModePolicy::Balanced,
        "match-internal" => ModePolicy::MatchInternal,
        "match-external" => ModePolicy::MatchExternal,
        "manual" => match requested {
            Some(m) => ModePolicy::Manual(m),
            None => {
                eprintln!("rosadeck: manual policy needs --mode");
                return 11;
            }
        },
        other => {
            eprintln!("rosadeck: INVALID_PLAN: unknown policy {other}");
            return 11;
        }
    };
    if requested.is_some() && !matches!(pol, ModePolicy::Manual(_)) {
        pol = ModePolicy::Manual(requested.unwrap());
    }
    let shell = match shell_policy.as_deref().unwrap_or("best-effort") {
        "strict" => ShellIntegrationPolicy::Strict,
        "best-effort" => ShellIntegrationPolicy::BestEffort,
        "disabled" => ShellIntegrationPolicy::Disabled,
        other => {
            eprintln!("rosadeck: INVALID shell policy {other}");
            return 11;
        }
    };
    let req = DisplayRequest {
        kind: PlanKind::ExternalOnly,
        target: on.map(OutputId),
        policy: pol,
        requested_mode: requested,
        position: None,
    };
    cmd_gaming_with_policy(&req, shell, dry_run, json, yes)
}

fn cmd_gaming_with_policy(
    req: &DisplayRequest,
    shell: ShellIntegrationPolicy,
    dry_run: bool,
    json: bool,
    yes: bool,
) -> i32 {
    // Snapshots first (display + session), read-only.
    let display_snapshot = match take_snapshot() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("rosadeck: backend: {e}");
            return 2;
        }
    };
    let session_snapshot = match take_session_snapshot() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("rosadeck: backend: {e}");
            return 2;
        }
    };
    let live = match unify(Path::new(DEFAULT_SYSFS)) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("rosadeck: backend: {e}");
            return 2;
        }
    };
    // Resolve target: explicit --on or first connected external.
    let target = match &req.target {
        Some(t) => {
            if !live.iter().any(|d| d.connector.0 == t.0 && d.drm_connected) {
                eprintln!("rosadeck: INVALID_TARGET: {}", t.0);
                return 11;
            }
            t.clone()
        }
        None => match live.iter().find(|d| d.drm_connected && !rosadeck_planner::is_internal(&d.connector.0)) {
            Some(d) => OutputId(d.connector.0.clone()),
            None => {
                eprintln!("rosadeck: NO_EXTERNAL_OUTPUT");
                return 15;
            }
        },
    };
    let req = DisplayRequest { target: Some(target.clone()), ..req.clone() };
    let input = super::planning_input_from(&display_snapshot, &live);
    let display_plan = match plan(&req, &input) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("rosadeck: {e}");
            return match e {
                rosadeck_planner::PlanError::NoCommonMode => 10,
                rosadeck_planner::PlanError::NoExternalOutput | rosadeck_planner::PlanError::NoInternalOutput => 15,
                rosadeck_planner::PlanError::WouldDisableLastOutput => 16,
                _ => 11,
            };
        }
    };
    // Workspace + focus + shell + wallpaper (all pure).
    let internal = session_snapshot.outputs.iter().find(|o| rosadeck_planner::is_internal(&o.state.id.0)).map(|o| o.state.id.0.clone()).unwrap_or_default();
    let ws_plan = rosadeck_session_core::build_workspace_plan(&session_snapshot, &internal, &target.0);
    let migrating: Vec<i32> = ws_plan.moves.iter().map(|m| m.workspace_id).collect();
    let focus = resolve_gaming_focus_target(&session_snapshot, &target.0, &migrating);
    let wallpaper_images = wallpaper_images_for(&target.0, &internal);
    let session_plan = build_session_plan(
        display_plan,
        ws_plan,
        focus,
        ShellPlan { quickshell: true, wallpaper: !wallpaper_images.is_empty() },
        WallpaperPlan { images: wallpaper_images },
    );
    if json {
        println!("{}", serde_json::to_string_pretty(&session_plan).unwrap_or_default());
        return 0;
    }
    print_session_plan(&session_plan);
    if dry_run {
        println!("Result: VALID PLAN (dry-run, nothing applied)");
        return 0;
    }
    if !yes && !super::confirm("Start external gaming session?") {
        eprintln!("rosadeck: aborted (needs --yes)");
        return 2;
    }
    if save_snapshot(&state_dir(), &display_snapshot).is_err() {
        eprintln!("rosadeck: cannot persist snapshot, aborting");
        return 2;
    }
    let mut dispatch = HyprctlDispatch;
    let mut runner = HyprctlRunner;
    let read_unified = || unify(Path::new(DEFAULT_SYSFS));
    let read_session = take_session_snapshot;
    let connected = || connected_connector_names(Path::new(DEFAULT_SYSFS));
    match run_gaming_session(
        &display_snapshot,
        &session_snapshot,
        &session_plan,
        &mut dispatch,
        &mut runner,
        &read_unified,
        &read_session,
        &connected,
        shell,
        &state_dir(),
    ) {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("rosadeck: {e}");
            exit_code_for(&e)
        }
    }
}

fn resolve_gaming_focus_target(
    snap: &rosadeck_session_core::SessionSnapshot,
    target: &str,
    migrating: &[i32],
) -> FocusPlan {
    let t = rosadeck_session_core::resolve_gaming_focus(snap, target, migrating);
    FocusPlan { target_monitor: target.into(), target_workspace: Some(t.workspace), target_window: t.window }
}

/// Wallpaper images to re-apply: current internal image onto the target.
fn wallpaper_images_for(target: &str, internal: &str) -> Vec<(String, String)> {
    let wp = rosadeck_shell_integration::detect_wallpaper();
    wp.current
        .iter()
        .find(|(o, _)| o == internal)
        .map(|(_, img)| vec![(target.into(), img.clone())])
        .unwrap_or_default()
}

fn print_session_plan(plan: &rosadeck_session_core::SessionPlan) {
    println!("SessionPlan (external gaming):");
    println!("  display steps:");
    for s in &plan.display_plan.steps {
        println!("    keyword monitor \"{}\"", s.to_keyword_arg());
    }
    println!("  workspaces: {}", plan.workspace_plan.moves.iter().map(|m| m.workspace_id.to_string()).collect::<Vec<_>>().join(", "));
    println!("  focus: {} ws {}", plan.focus.target_monitor, plan.focus.target_workspace.as_ref().map(|w| w.id.to_string()).unwrap_or("-".into()));
    println!("  wallpaper: {}", plan.wallpaper.images.iter().map(|(o, i)| format!("{o}={i}")).collect::<Vec<_>>().join(", "));
}

/// `rosadeck session-restore [--dry-run] [--json] [--yes]`: restore the
/// active gaming session (handle) or fall back to plain display restore.
pub fn cmd_session_restore(dry_run: bool, json: bool, yes: bool) -> i32 {
    use rosadeck_backend_hyprland::{restore_session, ShellReport};
    let dir = state_dir();
    let Some(handle) = GamingSessionHandle::read(&dir) else {
        // No gaming session: plain F3 restore path.
        return super::run_restore(json, dry_run, yes, None);
    };
    let session_snapshot: rosadeck_session_core::SessionSnapshot = match std::fs::read_to_string(&handle.snapshot_path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("rosadeck: INVALID session snapshot: {e}");
                return 11;
            }
        },
        Err(e) => {
            eprintln!("rosadeck: cannot read {}: {e}", handle.snapshot_path.display());
            return 11;
        }
    };
    let display_snapshot = match load_last_snapshot(&dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("rosadeck: {e}");
            return 11;
        }
    };
    // Wallpaper intent: current internal image back onto internal.
    let wallpaper_images = wallpaper_images_for(
        &session_snapshot.focused_output.clone().unwrap_or_default(),
        &session_snapshot.focused_output.clone().unwrap_or_default(),
    );
    let wallpaper = rosadeck_session_core::WallpaperPlan { images: wallpaper_images };
    let shell = ShellReport {
        quickshell_running: false,
        quickshell_cap: rosadeck_session_core::Capability::Unsupported,
        wallpaper_provider: rosadeck_shell_integration::detect_wallpaper().provider,
        wallpaper_cap: rosadeck_session_core::Capability::Available,
        wallpaper_current: vec![],
    };
    if json {
        println!("{}", serde_json::json!({"restore": handle.target, "workspaces": session_snapshot.workspaces.len()}));
        return 0;
    }
    println!("Restore gaming session {} (target was {})", handle.id, handle.target);
    if dry_run {
        println!("Result: VALID PLAN (dry-run, nothing applied)");
        return 0;
    }
    if !yes && !super::confirm("Restore gaming session?") {
        eprintln!("rosadeck: aborted (needs --yes)");
        return 2;
    }
    let mut dispatch = HyprctlDispatch;
    let mut runner = HyprctlRunner;
    let read_unified = || unify(Path::new(DEFAULT_SYSFS));
    let read_session = take_session_snapshot;
    let connected = || connected_connector_names(Path::new(DEFAULT_SYSFS));
    let mut log = vec![];
    match restore_session(
        &display_snapshot,
        &session_snapshot,
        &wallpaper,
        &shell,
        &mut dispatch,
        &mut runner,
        &read_unified,
        &read_session,
        &connected,
        &dir,
        &mut log,
    ) {
        Ok(()) => 0,
        Err(e) => {
            let code = exit_code_for(&e);
            eprintln!("rosadeck: {e}");
            code
        }
    }
}
