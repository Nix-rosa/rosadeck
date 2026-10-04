//! Mutation executor: structured commands → Hyprland, then re-read + verify.
//!
//! Mutation boundary (audited): this module and `HyprctlRunner` are the ONLY
//! places allowed to change compositor state. `display-core`, `edid`,
//! `profiles` and `planner` never mutate. All commands go through
//! [`MonitorCommand::to_keyword_arg`](rosadeck_planner::MonitorCommand),
//! never string concatenation of user input.
//!
//! Why the `hyprctl` binary instead of direct-socket writes (Decision):
//! the `.socket.sock` write framing (`[flags]/command`) is version-sensitive
//! and undocumented for mutations, while `hyprctl` ships version-matched with
//! the compositor. The [`CommandRunner`] trait keeps a future direct-socket
//! swap local to this module. Batch is best-effort sequential, NOT atomic
//! (see atomicity notes): a mid-batch failure stops the batch and triggers
//! restore.

use crate::backend::BackendResult;
use crate::snapshot::Snapshot;
use crate::unify::UnifiedDisplay;
use display_core::Mirror;
use rosadeck_planner::{DisplayPlan, ExpectedOutput, MonitorCommand, PlanKind};
use std::time::{Duration, Instant};

/// Verify budget: ~3s total (F0/F2 baseline), polled, never infinite.
pub const VERIFY_TIMEOUT_MS: u64 = 3000;
/// Re-read attempts within the budget.
pub const VERIFY_ATTEMPTS: u32 = 6;
/// Pause between attempts.
pub const VERIFY_INTERVAL_MS: u64 = 400;

/// Transition failure with stable meaning (CLI maps these to exit codes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionError {
    /// A `keyword monitor` / batch invocation failed.
    ApplyFailed(String),
    /// Post-apply state did not match the plan in budget.
    VerifyFailed(Vec<String>),
    /// Restore itself failed to apply or verify (worst case: manual recovery).
    RestoreFailed(String),
    /// No snapshot available for restore.
    NoSnapshot,
}

impl std::fmt::Display for TransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApplyFailed(m) => write!(f, "APPLY_FAILED: {m}"),
            Self::VerifyFailed(ms) => write!(f, "VERIFY_FAILED: {}", ms.join("; ")),
            Self::RestoreFailed(m) => write!(f, "RESTORE_FAILED: {m}"),
            Self::NoSnapshot => write!(f, "NO_SNAPSHOT"),
        }
    }
}

impl std::error::Error for TransitionError {}

/// How the transition ended (with a full log for forensics).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionOutcome {
    /// Applied and verified.
    Success,
    /// Rolled back to the snapshot and re-verified.
    RolledBack,
}

/// Runs one `keyword monitor "<arg>"` or a `--batch` group. Mocked in tests
/// so `cargo test` never touches real screens.
pub trait CommandRunner {
    /// Execute a single `hyprctl keyword monitor "<arg>"`.
    fn keyword_monitor(&mut self, arg: &str) -> Result<String, String>;
    /// Execute one `hyprctl --batch "<kw A> ; <kw B> ; ..."` call.
    fn batch(&mut self, args: &[String]) -> Result<String, String>;
}

/// Production runner: the version-matched `hyprctl` binary, contained here.
pub struct HyprctlRunner;

impl CommandRunner for HyprctlRunner {
    fn keyword_monitor(&mut self, arg: &str) -> Result<String, String> {
        run_hyprctl(&["keyword".into(), "monitor".into(), arg.into()])
    }

    fn batch(&mut self, args: &[String]) -> Result<String, String> {
        let script = args.iter().map(|a| format!("keyword monitor {a}")).collect::<Vec<_>>().join(" ; ");
        run_hyprctl(&["--batch".into(), script])
    }
}

fn run_hyprctl(args: &[String]) -> Result<String, String> {
    let out = std::process::Command::new("hyprctl")
        .args(args)
        .output()
        .map_err(|e| format!("spawn hyprctl: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        Ok(text)
    } else {
        Err(format!("hyprctl exited {}: {}", out.status, String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// Serialize + execute a plan's steps: one call for a single step, one
/// `--batch` otherwise. Stops at the first failing step (atomicity boundary:
/// the compositor applies steps sequentially; a failure may leave earlier
/// steps applied — the caller must restore).
pub fn apply_plan(plan: &DisplayPlan, runner: &mut dyn CommandRunner, log: &mut Vec<String>) -> Result<(), TransitionError> {
    let args: Vec<String> = plan.steps.iter().map(|s| s.to_keyword_arg()).collect();
    for (i, a) in args.iter().enumerate() {
        log.push(format!("APPLY step {}/{}: keyword monitor \"{a}\"", i + 1, args.len()));
    }
    if args.len() == 1 {
        runner.keyword_monitor(&args[0]).map_err(TransitionError::ApplyFailed)?;
    } else if !args.is_empty() {
        runner.batch(&args).map_err(TransitionError::ApplyFailed)?;
    }
    log.push("APPLY done".into());
    Ok(())
}

/// Re-read actual state and compare against the plan's expectations.
/// Returns mismatch descriptions (empty = verified).
pub fn check_plan(plan: &DisplayPlan, displays: &[UnifiedDisplay]) -> Vec<String> {
    let mut mismatches = Vec::new();
    for e in &plan.expect {
        let d = displays.iter().find(|d| d.connector.0 == e.id.0);
        match (d, e.mode) {
            (None, _) if !e.disabled => mismatches.push(format!("{}: output missing", e.id.0)),
            (None, _) => {}
            (Some(d), _) => {
                let enabled = d.output.as_ref().is_some_and(|o| !o.disabled);
                if enabled == e.disabled {
                    mismatches.push(format!("{}: enabled={enabled}, expected disabled={}", e.id.0, e.disabled));
                }
                if let Some(want) = e.mode {
                    match d.output.as_ref().and_then(|o| o.mode) {
                        Some(got) if got.matches_within(want, display_core::HZ_TOLERANCE) => {}
                        Some(got) => mismatches.push(format!("{}: mode {got} != expected {want}", e.id.0)),
                        None => mismatches.push(format!("{}: no active mode, expected {want}", e.id.0)),
                    }
                }
                if let Some(o) = &d.output {
                    if o.mirror != e.mirror {
                        mismatches.push(format!("{}: mirror {:?} != expected {:?}", e.id.0, o.mirror, e.mirror));
                    }
                    if let Some(p) = e.position {
                        if o.position != p {
                            mismatches.push(format!("{}: position {:?} != expected {p:?}", e.id.0, o.position));
                        }
                    }
                }
            }
        }
    }
    let enabled = displays.iter().filter(|d| d.output.as_ref().is_some_and(|o| !o.disabled)).count();
    if enabled < plan.min_enabled {
        mismatches.push(format!("enabled outputs {enabled} < min {}", plan.min_enabled));
    }
    if let Some(exact) = plan.exact_enabled {
        if enabled != exact {
            mismatches.push(format!("enabled outputs {enabled} != required {exact}"));
        }
    }
    mismatches
}

/// Verify with retries inside the budget. `read` re-unifies live state.
pub fn verify_plan(
    plan: &DisplayPlan,
    read: &dyn Fn() -> BackendResult<Vec<UnifiedDisplay>>,
    log: &mut Vec<String>,
) -> Result<(), TransitionError> {
    let deadline = Instant::now() + Duration::from_millis(VERIFY_TIMEOUT_MS);
    let mut last = Vec::new();
    for attempt in 1..=VERIFY_ATTEMPTS {
        match read() {
            Ok(displays) => {
                last = check_plan(plan, &displays);
                if last.is_empty() {
                    log.push(format!("VERIFY OK (attempt {attempt})"));
                    return Ok(());
                }
                log.push(format!("VERIFY attempt {attempt}: {}", last.join("; ")));
            }
            Err(e) => {
                last = vec![format!("re-read failed: {e}")];
                log.push(format!("VERIFY attempt {attempt}: {e}"));
            }
        }
        if attempt < VERIFY_ATTEMPTS && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(VERIFY_INTERVAL_MS));
        }
    }
    Err(TransitionError::VerifyFailed(last))
}

/// Build a restore plan from a snapshot: every snapshotted output returns to
/// its recorded mode/position/scale/mirror/disabled/transform. Outputs that
/// no longer exist (e.g. unplugged HDMI) are skipped; if skipping would leave
/// zero enabled outputs, the internal output is forced back on when present
/// in the snapshot (hotplug-disconnect rule).
pub fn build_restore_plan(snapshot: &Snapshot, connected_now: &[String]) -> DisplayPlan {
    let mut steps = Vec::new();
    let mut expect = Vec::new();
    for o in &snapshot.outputs {
        if !connected_now.contains(&o.state.id.0) {
            continue; // vanished (unplugged): cannot restore, must not fail
        }
        let cmd = if o.state.disabled {
            MonitorCommand {
                output: o.state.id.clone(),
                resolution: None,
                position: None,
                scale: None,
                mirror: None,
                disabled: true,
                transform: None,
            }
        } else {
            MonitorCommand {
                output: o.state.id.clone(),
                resolution: o.state.mode.map(|m| format!("{}x{}@{}", m.width, m.height, m.hz)),
                position: Some(o.state.position),
                scale: Some(o.state.scale.0),
                mirror: match &o.state.mirror {
                    Mirror::Disabled => None,
                    Mirror::MirrorOf(id) => Some(id.clone()),
                },
                disabled: false,
                transform: (o.transform != 0).then_some(o.transform),
            }
        };
        expect.push(ExpectedOutput {
            id: o.state.id.clone(),
            mode: o.state.mode,
            disabled: o.state.disabled,
            mirror: o.state.mirror.clone(),
            position: Some(o.state.position),
        });
        steps.push(cmd);
    }
    // Hotplug rule: never restore into zero enabled outputs.
    if !steps.iter().any(|s| !s.disabled) {
        if let Some(internal) = snapshot.outputs.iter().find(|o| rosadeck_planner::is_internal(&o.state.id.0)) {
            steps.push(MonitorCommand {
                output: internal.state.id.clone(),
                resolution: internal.state.mode.map(|m| format!("{}x{}@{}", m.width, m.height, m.hz)),
                position: Some(internal.state.position),
                scale: Some(internal.state.scale.0),
                mirror: None,
                disabled: false,
                transform: (internal.transform != 0).then_some(internal.transform),
            });
            expect.push(ExpectedOutput {
                id: internal.state.id.clone(),
                mode: internal.state.mode,
                disabled: false,
                mirror: Mirror::Disabled,
                position: Some(internal.state.position),
            });
        }
    }
    DisplayPlan { kind: PlanKind::Extend, steps, expect, min_enabled: 1, exact_enabled: None }
}

/// Full transition: apply → verify → (on failure) restore → verify restore.
/// No `unsafe_apply` path exists: the snapshot is a required argument.
pub fn run_transition(
    snapshot: &Snapshot,
    plan: &DisplayPlan,
    runner: &mut dyn CommandRunner,
    read: &dyn Fn() -> BackendResult<Vec<UnifiedDisplay>>,
    connected_now: &dyn Fn() -> Vec<String>,
) -> Result<TransitionOutcome, TransitionError> {
    let mut log = Vec::new();
    log.push(format!("SNAPSHOT taken at {}", snapshot.taken_at));
    apply_plan(plan, runner, &mut log)?;
    if verify_plan(plan, read, &mut log).is_ok() {
        log.push("TRANSITION SUCCESS".into());
        eprint_log(&log);
        return Ok(TransitionOutcome::Success);
    }
    log.push("VERIFY FAILED — RESTORE START".into());
    let restore = build_restore_plan(snapshot, &connected_now());
    if let Err(e) = apply_plan(&restore, runner, &mut log) {
        log.push(format!("RESTORE APPLY FAILED: {e}"));
        eprint_log(&log);
        return Err(TransitionError::RestoreFailed(e.to_string()));
    }
    match verify_plan(&restore, read, &mut log) {
        Ok(()) => {
            log.push("RESTORE VERIFY OK — TRANSITION ROLLED BACK".into());
            eprint_log(&log);
            Ok(TransitionOutcome::RolledBack)
        }
        Err(TransitionError::VerifyFailed(ms)) => {
            log.push("RESTORE VERIFY FAILED".into());
            eprint_log(&log);
            Err(TransitionError::RestoreFailed(ms.join("; ")))
        }
        Err(e) => {
            eprint_log(&log);
            return Err(e);
        }
    }
}

fn eprint_log(log: &[String]) {
    for line in log {
        eprintln!("[rosadeck] {line}");
    }
}

/// Map a transition failure to a stable CLI exit code.
pub fn exit_code_for(e: &TransitionError) -> i32 {
    match e {
        TransitionError::ApplyFailed(_) => 12,
        TransitionError::VerifyFailed(_) => 13,
        TransitionError::RestoreFailed(_) => 14,
        TransitionError::NoSnapshot => 11,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::SnapshotOutput;
    use display_core::{Mode, OutputId, Position, Scale};
    use rosadeck_planner::{DisplayPlan, ExpectedOutput, MonitorCommand, PlanKind};

    /// Stub runner: scripted results, records every call (never touches screens).
    #[derive(Default)]
    struct StubRunner {
        calls: Vec<String>,
        fail_on: Vec<String>, // substrings that make a call fail
    }

    impl CommandRunner for StubRunner {
        fn keyword_monitor(&mut self, arg: &str) -> Result<String, String> {
            self.calls.push(format!("keyword {arg}"));
            if self.fail_on.iter().any(|f| arg.contains(f)) {
                return Err(format!("stub refuse: {arg}"));
            }
            Ok("ok".into())
        }

        fn batch(&mut self, args: &[String]) -> Result<String, String> {
            self.calls.push(format!("batch {}", args.join(" ; ")));
            if args.iter().any(|a| self.fail_on.iter().any(|f| a.contains(f))) {
                return Err("stub refuse batch".into());
            }
            Ok("ok".into())
        }
    }

    fn output_state(id: &str, mode: Option<Mode>, disabled: bool) -> display_core::OutputState {
        display_core::OutputState {
            id: OutputId(id.into()),
            connector: None,
            mode,
            position: Position { x: 0, y: 0 },
            scale: Scale(1.0),
            disabled,
            mirror: Mirror::Disabled,
        }
    }

    fn unified(id: &str, mode: Option<Mode>, disabled: bool) -> UnifiedDisplay {
        UnifiedDisplay {
            output: Some(output_state(id, mode, disabled)),
            info: None,
            connector: display_core::ConnectorId(id.into()),
            drm_connected: !disabled,
            identity: None,
            edid_name: None,
            modes: vec![],
            warnings: vec![],
            notes: vec![],
        }
    }

    fn snapshot_two() -> Snapshot {
        Snapshot {
            taken_at: 1,
            outputs: vec![
                SnapshotOutput {
                    state: output_state("eDP-1", Some(Mode { width: 1920, height: 1080, hz: 60.0 }), false),
                    transform: 0,
                    vrr: false,
                    focused: false,
                    active_workspace: None,
                },
                SnapshotOutput {
                    state: output_state("HDMI-A-1", None, true),
                    transform: 0,
                    vrr: false,
                    focused: false,
                    active_workspace: None,
                },
            ],
            workspaces: vec![],
            focused_output: Some("eDP-1".into()),
            raw_monitors: serde_json::json!([]),
            raw_workspaces: serde_json::json!([]),
        }
    }

    #[test]
    fn apply_uses_batch_for_multi_step() {
        let plan = DisplayPlan {
            kind: PlanKind::ExternalOnly,
            steps: vec![
                MonitorCommand { output: OutputId("HDMI-A-1".into()), resolution: Some("1920x1080@120".into()), position: Some(Position { x: 0, y: 0 }), scale: None, mirror: None, disabled: false, transform: None },
                MonitorCommand { output: OutputId("eDP-1".into()), resolution: None, position: None, scale: None, mirror: None, disabled: true, transform: None },
            ],
            expect: vec![],
            min_enabled: 1,
            exact_enabled: Some(1),
        };
        let mut r = StubRunner::default();
        let mut log = vec![];
        apply_plan(&plan, &mut r, &mut log).unwrap();
        assert_eq!(r.calls.len(), 1);
        assert!(r.calls[0].starts_with("batch "));
        assert!(r.calls[0].contains("HDMI-A-1,1920x1080@120,0x0,auto"));
        assert!(!r.calls[0].contains("mirror,"));
        assert!(r.calls[0].contains("eDP-1,disable"));
    }

    #[test]
    fn apply_failure_propagates_without_verify() {
        let plan = DisplayPlan {
            kind: PlanKind::ExternalOnly,
            steps: vec![MonitorCommand { output: OutputId("NOPE".into()), resolution: None, position: None, scale: None, mirror: None, disabled: true, transform: None }],
            expect: vec![],
            min_enabled: 1,
            exact_enabled: None,
        };
        let mut r = StubRunner { calls: vec![], fail_on: vec!["NOPE".into()] };
        let mut log = vec![];
        assert!(matches!(apply_plan(&plan, &mut r, &mut log), Err(TransitionError::ApplyFailed(_))));
    }

    #[test]
    fn verify_failure_triggers_restore_and_reports_rollback() {
        let snap = snapshot_two();
        // Plan disables eDP-1 but the stub "screen" never changes: verify fails.
        let plan = DisplayPlan {
            kind: PlanKind::ExternalOnly,
            steps: vec![MonitorCommand { output: OutputId("eDP-1".into()), resolution: None, position: None, scale: None, mirror: None, disabled: true, transform: None }],
            expect: vec![ExpectedOutput { id: OutputId("eDP-1".into()), mode: None, disabled: true, mirror: Mirror::Disabled, position: None }],
            min_enabled: 1,
            exact_enabled: None,
        };
        let mut r = StubRunner::default();
        // Live state stays eDP-1 enabled: plan expect fails, restore expect passes.
        let read = || Ok(vec![unified("eDP-1", Some(Mode { width: 1920, height: 1080, hz: 60.0 }), false)]);
        let connected = || vec!["eDP-1".into(), "HDMI-A-1".into()];
        let outcome = run_transition(&snap, &plan, &mut r, &read, &connected).unwrap();
        assert_eq!(outcome, TransitionOutcome::RolledBack);
        assert!(r.calls.iter().any(|c| c.contains("eDP-1,1920x1080@60,0x0,1")));
    }

    #[test]
    fn restore_failure_is_terminal() {
        let snap = snapshot_two();
        let plan = DisplayPlan {
            kind: PlanKind::ExternalOnly,
            steps: vec![MonitorCommand { output: OutputId("eDP-1".into()), resolution: None, position: None, scale: None, mirror: None, disabled: true, transform: None }],
            expect: vec![ExpectedOutput { id: OutputId("eDP-1".into()), mode: None, disabled: true, mirror: Mirror::Disabled, position: None }],
            min_enabled: 1,
            exact_enabled: None,
        };
        // Everything the runner touches fails.
        let mut r = StubRunner { calls: vec![], fail_on: vec!["eDP-1".into()] };
        let read = || Ok(vec![unified("eDP-1", Some(Mode { width: 1920, height: 1080, hz: 60.0 }), false)]);
        let connected = || vec!["eDP-1".into()];
        let err = run_transition(&snap, &plan, &mut r, &read, &connected).unwrap_err();
        assert!(matches!(err, TransitionError::ApplyFailed(_))); // first step already fails
    }

    #[test]
    fn restore_skips_vanished_external_and_keeps_internal() {
        let snap = snapshot_two();
        // HDMI unplugged mid-transition: only eDP-1 connected now.
        let restore = build_restore_plan(&snap, &["eDP-1".into()]);
        assert!(restore.steps.iter().all(|s| s.output.0 == "eDP-1"));
        assert!(restore.steps.iter().any(|s| !s.disabled));
    }

    #[test]
    fn exit_codes_are_stable() {
        assert_eq!(exit_code_for(&TransitionError::ApplyFailed("x".into())), 12);
        assert_eq!(exit_code_for(&TransitionError::VerifyFailed(vec![])), 13);
        assert_eq!(exit_code_for(&TransitionError::RestoreFailed("x".into())), 14);
    }
}
