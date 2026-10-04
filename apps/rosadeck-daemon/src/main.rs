//! `rosadeck-daemon`: resident session loop (orchestration only).
//!
//! Event flow: `watch_events` (500 ms slices) → [`Debouncer`] (750 ms quiet
//! window) → `unify` → [`decide`] → act. Mutations are delegated to the
//! `rosadeck` CLI binary (single tested code path, same exit codes) instead
//! of duplicating the snapshot/plan/transition discipline here.
//!
//! Signals (Decision): SIGTERM/SIGHUP set a flag via `signal-hook`. The flag
//! is honored between safe sections: no new transition starts once set, and
//! an in-flight CLI invocation (F3's own safe sequence) runs to completion
//! before exit. SIGKILL cannot be handled (documented, not promised).

mod debounce;
mod orchestrate;
mod state;

use clap::Parser;
use debounce::Debouncer;
use display_core::OutputId;
use rosadeck_backend_hyprland::{state_dir, unify, watch_events, UnifiedDisplay, DEFAULT_SYSFS};
use rosadeck_planner::is_internal;
use orchestrate::{decide, DaemonAction, DecideCtx, LoopGuard, OutputView};
use state::DaemonState;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Own-transition echo suppression window (compositor echoes our own
/// `keyword` applies as `configreloaded`; Assumption, tuned in §F4 doc).
const SELF_ECHO_QUIET_MS: u64 = 2000;
/// Socket2 poll slice.
const POLL_MS: u64 = 500;
/// Overlay result wait.
const SELECTION_TIMEOUT_S: u64 = 180;

#[derive(Debug, Parser)]
#[command(name = "rosadeck-daemon", version, about = "Rosadeck reactive session daemon")]
struct Args {
    /// Do one reconcile and exit (validation without daemonizing).
    #[arg(long)]
    reconcile_once: bool,
    /// Debounce quiet window in ms.
    #[arg(long, default_value = "750")]
    debounce_ms: u64,
    /// Log selector contexts instead of opening the overlay.
    #[arg(long)]
    no_overlay: bool,
    /// Use wofi instead of the floating terminal for the selector.
    #[arg(long)]
    wofi: bool,
}

fn log(level: &str, msg: &str) {
    eprintln!("[rosadeck-daemon][{level}] {msg}");
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn views(displays: &[UnifiedDisplay]) -> Vec<OutputView> {
    displays
        .iter()
        .map(|d| OutputView {
            connector: d.connector.0.clone(),
            connected: d.drm_connected,
            enabled: d.output.as_ref().is_some_and(|o| !o.disabled),
            is_external: !is_internal(&d.connector.0),
        })
        .collect()
}

/// Sibling binary next to this executable, else PATH lookup by name.
fn sibling(name: &str) -> String {
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

fn profiles_dir() -> PathBuf {
    let base = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let base = PathBuf::from(base);
    // El nombre viejo sigue valiendo hasta que el usuario mueva sus datos.
    let n = base.join(".config/rosadeck/displays");
    if n.exists() || !base.join(".config/hyprgame/displays").exists() {
        n
    } else {
        base.join(".config/hyprgame/displays")
    }
}

/// Best saved-profile summary for an identity hash (read-only scan).
fn lookup_profile(edid_hash: Option<&str>, alias: Option<&str>) -> Option<rosadeck_selector::SavedProfile> {
    use rosadeck_profiles::{find_best, load, MatchQuery};
    let dir = profiles_dir();
    let rd = std::fs::read_dir(&dir).ok()?;
    let mut profiles = Vec::new();
    for e in rd.filter_map(|e| e.ok()) {
        if e.path().extension().is_some_and(|x| x == "toml") {
            if let Ok(p) = load(&e.path()) {
                profiles.push(p);
            }
        }
    }
    if profiles.is_empty() {
        return None;
    }
    let q = MatchQuery {
        edid_hash: edid_hash.map(|s| s.into()),
        alias: alias.map(|s| s.into()),
        ..Default::default()
    };
    let (p, _) = find_best(&profiles, &q)?;
    let kind = match p.preferred.mode {
        rosadeck_profiles::ScreenMode::Duplicate => rosadeck_planner::PlanKind::Duplicate,
        rosadeck_profiles::ScreenMode::ExternalOnly => rosadeck_planner::PlanKind::ExternalOnly,
        rosadeck_profiles::ScreenMode::Extend => rosadeck_planner::PlanKind::Extend,
    };
    let policy = match p.preferred.policy.as_str() {
        "max-resolution" => display_core::ModePolicy::MaxResolution,
        "balanced" => display_core::ModePolicy::Balanced,
        "match-internal" => display_core::ModePolicy::MatchInternal,
        "match-external" => display_core::ModePolicy::MatchExternal,
        "manual" => display_core::ModePolicy::MaxRefresh,
        _ => display_core::ModePolicy::MaxRefresh,
    };
    Some(rosadeck_selector::SavedProfile {
        alias: p.display.alias.clone().unwrap_or_else(|| p.display.stable_id.clone()),
        kind,
        mode: rosadeck_profiles::preferred_mode(&p),
        policy,
    })
}

/// Open the floating selector and wait for the user's choice.
/// Returns the raw `Selection` JSON, or `None` on cancel/timeout/shutdown.
fn run_selector(
    ctx: &rosadeck_selector::SelectionContext,
    shutdown: &Arc<AtomicBool>,
    no_overlay: bool,
    wofi: bool,
) -> Option<rosadeck_selector::Selection> {
    let dir = state_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let stamp = now_ms();
    let ctx_path = dir.join(format!("selector-{stamp}.json"));
    let res_path = dir.join(format!("selection-{stamp}.json"));
    std::fs::write(&ctx_path, serde_json::to_string_pretty(ctx).ok()?).ok()?;
    if no_overlay {
        log("INFO", &format!("overlay suppressed; context at {}", ctx_path.display()));
        return None;
    }
    let overlay = sibling("rosadeck-overlay");
    if wofi {
        // wofi draws its own surface: run the overlay directly, no terminal.
        return match std::process::Command::new(&overlay)
            .args(["--wofi", "--context", &ctx_path.to_string_lossy(), "--result", &res_path.to_string_lossy()])
            .status()
        {
            Ok(s) if s.success() => wait_selection(&res_path, shutdown),
            _ => None, // exit 3 / error = cancel
        };
    }
    let status = std::process::Command::new("kitty")
        .args([
            "--class",
            "rosadeck-overlay",
            "--",
            &overlay,
            "--context",
            &ctx_path.to_string_lossy(),
            "--result",
            &res_path.to_string_lossy(),
        ])
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            log("WARN", &format!("overlay terminal exited {s} (need kitty?); context kept at {}", ctx_path.display()));
            return None;
        }
        Err(e) => {
            log("WARN", &format!("cannot spawn kitty: {e}"));
            return None;
        }
    }
    // The overlay writes the result file on confirm; Esc/close leaves nothing.
    match wait_selection(&res_path, shutdown) {
        Some(sel) => Some(sel),
        None => {
            log("WARN", "selector timed out");
            None
        }
    }
}

/// Poll an overlay result file (shared by kitty/wofi paths).
fn wait_selection(res_path: &Path, shutdown: &Arc<AtomicBool>) -> Option<rosadeck_selector::Selection> {
    let deadline = Instant::now() + Duration::from_secs(SELECTION_TIMEOUT_S);
    while Instant::now() < deadline {
        if shutdown.load(Ordering::Relaxed) {
            return None;
        }
        if let Ok(text) = std::fs::read_to_string(res_path) {
            if let Ok(sel) = serde_json::from_str(&text) {
                return Some(sel);
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    None
}

/// `DisplayRequest` (gaming flavor) → `rosadeck gaming` argv.
fn gaming_argv(req: &rosadeck_planner::DisplayRequest, connector: &str) -> Vec<String> {
    let mut args = vec!["gaming".to_owned(), "--on".into(), connector.to_owned(), "--yes".into()];
    match &req.policy {
        display_core::ModePolicy::Manual(m) => {
            args.extend(["--mode".into(), format!("{}x{}@{}", m.width, m.height, m.hz)]);
        }
        display_core::ModePolicy::MaxResolution => args.extend(["--policy".into(), "max-resolution".into()]),
        display_core::ModePolicy::Balanced => args.extend(["--policy".into(), "balanced".into()]),
        display_core::ModePolicy::MatchInternal => args.extend(["--policy".into(), "match-internal".into()]),
        display_core::ModePolicy::MatchExternal => args.extend(["--policy".into(), "match-external".into()]),
        display_core::ModePolicy::MaxRefresh => {}
    }
    if let Some(m) = &req.requested_mode {
        if !args.iter().any(|a| a == "--mode") {
            args.extend(["--mode".into(), format!("{}x{}@{}", m.width, m.height, m.hz)]);
        }
    }
    args
}

/// `DisplayRequest` → `rosadeck` CLI argv (single mutation path, F3 codes).
/// Saved profiles already arrive expanded via `selection_to_request`.
fn request_to_cli(req: &rosadeck_planner::DisplayRequest) -> Vec<String> {
    use rosadeck_planner::PlanKind;
    let sub = match req.kind {
        PlanKind::Duplicate => "duplicate",
        PlanKind::ExternalOnly => "external",
        PlanKind::Extend => "extend",
    };
    let mut args = vec![sub.to_owned(), "--yes".into()];
    if let Some(t) = &req.target {
        args.extend(["--on".into(), t.0.clone()]);
    }
    match &req.policy {
        display_core::ModePolicy::Manual(m) => {
            args.extend(["--mode".into(), format!("{}x{}@{}", m.width, m.height, m.hz)]);
        }
        display_core::ModePolicy::MaxResolution => args.extend(["--policy".into(), "max-resolution".into()]),
        display_core::ModePolicy::Balanced => args.extend(["--policy".into(), "balanced".into()]),
        display_core::ModePolicy::MatchInternal => args.extend(["--policy".into(), "match-internal".into()]),
        display_core::ModePolicy::MatchExternal => args.extend(["--policy".into(), "match-external".into()]),
        display_core::ModePolicy::MaxRefresh => {}
    }
    if let Some(p) = &req.position {
        args.extend(["--pos".into(), format!("{p:?}").to_lowercase()]);
    }
    args
}

fn main() {
    let args = Args::parse();
    let shutdown = Arc::new(AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGHUP] {
        let flag = shutdown.clone();
        if signal_hook::flag::register(sig, flag).is_err() {
            log("WARN", "cannot install signal handler");
        }
    }

    if args.reconcile_once {
        match unify(Path::new(DEFAULT_SYSFS)) {
            Ok(ds) => {
                let vs = views(&ds);
                log("INFO", &format!("reconciled outputs: {}", vs.len()));
                for v in &vs {
                    log("INFO", &format!("{} connected={} enabled={} external={}", v.connector, v.connected, v.enabled, v.is_external));
                }
            }
            Err(e) => {
                log("ERROR", &format!("reconcile failed: {e}"));
                std::process::exit(2);
            }
        }
        return;
    }

    log("INFO", "rosadeck-daemon starting (no autostart integration in F4)");
    let mut debouncer = Debouncer::new(args.debounce_ms);
    let mut guard = LoopGuard::new(60_000, 2);
    let mut state = DaemonState::Idle;
    let mut prev = views(&unify(Path::new(DEFAULT_SYSFS)).unwrap_or_default());
    let mut last_own_apply: Option<Instant> = None;

    while !shutdown.load(Ordering::Relaxed) {
        // 1. Collect one poll slice of socket2 events (read-only).
        let mut batch = Vec::new();
        if watch_events(Duration::from_millis(POLL_MS), |ev| batch.push(ev)).is_err() {
            log("ERROR", "socket2 watch failed; retrying");
            std::thread::sleep(Duration::from_millis(POLL_MS));
            continue;
        }
        let now = now_ms();
        for ev in batch {
            debouncer.push(ev, now);
        }
        if debouncer.pending_count() > 0 {
            log("INFO", &format!("{} socket2 events buffered", debouncer.pending_count()));
        }
        // 2. Reconcile once the line is quiet.
        let ready = debouncer.drain_ready(now_ms());
        if ready.is_empty() {
            continue;
        }
        log("INFO", &format!("debounce complete ({} events)", ready.len()));
        state = state.step(DaemonState::Reconciling).unwrap_or(DaemonState::Reconciling);
        let displays = match unify(Path::new(DEFAULT_SYSFS)) {
            Ok(d) => d,
            Err(e) => {
                log("ERROR", &format!("reconcile failed: {e}"));
                state = DaemonState::Error(e.to_string());
                continue;
            }
        };
        let curr = views(&displays);
        let echo_quiet = last_own_apply.is_some_and(|t| t.elapsed() < Duration::from_millis(SELF_ECHO_QUIET_MS));
        let snapshot_available = state_dir().join("last").exists();
        let ctx = DecideCtx {
            transition_in_progress: state.transition_in_progress(),
            suppress_self_echo: echo_quiet,
            restore_allowed: guard.would_allow(now_ms()),
            snapshot_available,
        };
        match decide(&prev, &curr, &ctx) {
            DaemonAction::Nothing => {
                state = DaemonState::Idle;
            }
            DaemonAction::ReportError(m) => {
                log("ERROR", &m);
                state = DaemonState::Error(m);
            }
            DaemonAction::ShowSelector { connector } => {
                if !state.can_start_transition() {
                    log("WARN", "selector requested while a transition runs; deferred");
                    continue;
                }
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                handle_selection(&displays, &connector, &shutdown, args.no_overlay, args.wofi, &mut last_own_apply, &mut state);
            }
            DaemonAction::AutoRestore { reason } => {
                if !state.can_start_transition() {
                    log("WARN", "auto-restore requested while a transition runs; deferred");
                    continue;
                }
                guard.allow(now_ms());
                // Gaming session active for the lost output → session restore
                // (workspaces/focus/shell), else plain display restore.
                let gaming = rosadeck_session_core::GamingSessionHandle::read(&rosadeck_backend_hyprland::state_dir())
                    .filter(|h| reason.contains(&h.target));
                let (sub, next_state) = match &gaming {
                    Some(h) => {
                        log("WARN", &format!("{}; gaming session {} → session restore", reason, h.id));
                        (vec!["session-restore".to_owned(), "--yes".to_owned()], DaemonState::RestoringSession)
                    }
                    None => {
                        log("WARN", &format!("{reason}; automatic restore started"));
                        (vec!["restore".to_owned(), "--yes".to_owned()], DaemonState::Restoring)
                    }
                };
                state = state.step(next_state).unwrap_or(DaemonState::Restoring);
                log("WARN", &format!("auto-restore running (state={state:?})"));
                let code = run_cli(&sub);
                last_own_apply = Some(Instant::now());
                if code == 0 {
                    log("INFO", "restore successful; internal output verified by CLI");
                    state = DaemonState::Idle;
                } else {
                    log("ERROR", &format!("restore verification failed (exit {code})"));
                    state = DaemonState::Error(format!("auto-restore exit {code}"));
                }
            }
        }
        prev = views(&unify(Path::new(DEFAULT_SYSFS)).unwrap_or_default());
        // Reflect an externally-started gaming session (CLI writes the handle;
        // the daemon observes it so disconnect recovery picks the right path).
        let gaming_active =
            rosadeck_session_core::GamingSessionHandle::read(&rosadeck_backend_hyprland::state_dir()).is_some();
        if gaming_active && state == DaemonState::Idle {
            state = DaemonState::GamingSession;
            log("INFO", "gaming session active (handle observed)");
        } else if !gaming_active && state == DaemonState::GamingSession {
            state = DaemonState::Idle;
            log("INFO", "gaming session ended (handle cleared)");
        }
    }
    log("INFO", "shutdown requested; no transition in flight, exiting cleanly");
}

/// Build the selector context, run the overlay, and delegate the confirmed
/// selection to the `rosadeck` CLI (F3 path: snapshot→plan→apply→verify).
fn handle_selection(
    displays: &[UnifiedDisplay],
    connector: &str,
    shutdown: &Arc<AtomicBool>,
    no_overlay: bool,
    wofi: bool,
    last_own_apply: &mut Option<Instant>,
    state: &mut DaemonState,
) {
    use rosadeck_selector::{build_context, selection_to_request, ContextArgs};
    let Some(d) = displays.iter().find(|d| d.connector.0 == connector) else { return };
    let internal_modes: Vec<display_core::Mode> = displays
        .iter()
        .find(|d| is_internal(&d.connector.0))
        .map(hyprland_modes)
        .unwrap_or_default();
    let external_modes = hyprland_modes(d);
    let hash = d.identity.as_ref().map(|i| i.stable_id().rsplit('-').next().unwrap_or("").to_owned());
    let profile = lookup_profile(hash.as_deref(), None).map(|s| {
        log("INFO", &format!("profile matched: {}", s.alias));
        s
    });
    if profile.is_none() {
        log("INFO", "unknown display (no profile found)");
    }
    let ctx_args = ContextArgs {
        connector: OutputId(connector.into()),
        edid_name: d.edid_name.clone(),
        identity: d.identity.clone(),
        current: d.output.as_ref().and_then(|o| o.mode),
        internal_modes,
        external_modes,
        profile,
    };
    let sel_ctx = build_context(&ctx_args);
    log("INFO", &format!("showing display selector for {connector}"));
    *state = DaemonState::AwaitingSelection;
    let Some(sel) = run_selector(&sel_ctx, shutdown, no_overlay, wofi) else {
        log("INFO", "selector cancelled");
        *state = DaemonState::Idle;
        return;
    };
    let Some(idx) = sel_ctx.actions.iter().position(|a| *a == sel.action) else { return };
    // Validate manual picks against the action's real options first.
    let Some(req) = selection_to_request(&sel_ctx, idx, &sel, None) else {
        log("WARN", "selection names a mode the action cannot offer; ignoring");
        *state = DaemonState::Idle;
        return;
    };
    // ExternalGaming runs the full session pipeline, not a display command.
    let argv = if sel.action == rosadeck_selector::SelectorAction::ExternalGaming {
        gaming_argv(&req, connector)
    } else {
        request_to_cli(&req)
    };
    *state = DaemonState::Applying;
    log("INFO", &format!("user selected {}; plan validated by CLI", argv.join(" ")));
    let code = run_cli(&argv);
    *last_own_apply = Some(Instant::now());
    *state = DaemonState::Idle;
    if code == 0 {
        log("INFO", "transition applied; verification successful (CLI)");
    } else {
        log("ERROR", &format!("transition failed (exit {code}); CLI restored snapshot"));
    }
}

/// Hyprland-accepted modes of one unified display (planner input only).
fn hyprland_modes(d: &UnifiedDisplay) -> Vec<display_core::Mode> {
    d.modes
        .iter()
        .filter(|m| m.sources.contains(&display_core::ModeSource::Hyprland))
        .map(|m| m.mode)
        .collect()
}

/// Delegate a mutation to the `rosadeck` CLI (single code path). Returns its
/// exit code unchanged so daemon logs match CLI semantics.
fn run_cli(argv: &[String]) -> i32 {
    let cli = sibling("rosadeck");
    match std::process::Command::new(&cli).args(argv).status() {
        Ok(s) => s.code().unwrap_or(2),
        Err(e) => {
            log("ERROR", &format!("cannot spawn {cli}: {e}"));
            2
        }
    }
}
