//! Pure orchestration decisions: views in, [`DaemonAction`] out.
//!
//! Rules (Decision):
//! - While a transition is in flight, or events are our own echo, do nothing.
//! - A lost *active* external beats a new external (black-screen avoidance).
//! - A new connected external offers the selector (never auto-applies).
//! - Repeated auto-restores inside the cooldown window escalate to an error
//!   instead of looping (`restore → configreloaded → restore → …`).

/// One output as the daemon sees it after `unify`.
#[derive(Debug, Clone)]
pub struct OutputView {
    /// Connector/output name.
    pub connector: String,
    /// Kernel reports attached.
    pub connected: bool,
    /// Compositor keeps it in the layout.
    pub enabled: bool,
    /// Non-laptop output.
    pub is_external: bool,
}

/// Inputs to one decision step.
#[derive(Debug, Clone)]
pub struct DecideCtx {
    /// A transition (CLI invocation) is running right now.
    pub transition_in_progress: bool,
    /// These events are the echo of our own apply (quiet window).
    pub suppress_self_echo: bool,
    /// Loop guard still allows another automatic restore.
    pub restore_allowed: bool,
    /// A snapshot covering the lost output exists.
    pub snapshot_available: bool,
}

/// What the daemon should do after reconciling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonAction {
    /// Nothing to do.
    Nothing,
    /// Open the selector for this connector.
    ShowSelector {
        /// Target external connector.
        connector: String,
    },
    /// Re-apply the last safe snapshot (active external vanished).
    AutoRestore {
        /// Human reason for the log.
        reason: String,
    },
    /// Explicit error (e.g. suspected restore loop); never silent.
    ReportError(String),
}

/// Decide from the previous and current reconciled views.
pub fn decide(prev: &[OutputView], curr: &[OutputView], ctx: &DecideCtx) -> DaemonAction {
    if ctx.transition_in_progress || ctx.suppress_self_echo {
        return DaemonAction::Nothing;
    }
    if prev.is_empty() {
        return DaemonAction::Nothing; // startup: quiet, user drives via CLI
    }
    let was_active = |c: &str| prev.iter().any(|o| o.connector == c && o.enabled && o.is_external);
    let is_gone = |c: &str| curr.iter().find(|o| o.connector == c).is_none_or(|o| !o.connected);
    // 1. Lost active external → restore (black-screen avoidance first).
    for o in prev {
        if o.is_external && o.enabled && is_gone(&o.connector) && was_active(&o.connector) {
            if !ctx.snapshot_available {
                return DaemonAction::ReportError(format!("active output {} lost, no snapshot", o.connector));
            }
            if !ctx.restore_allowed {
                return DaemonAction::ReportError(format!("restore loop suspected for {}", o.connector));
            }
            return DaemonAction::AutoRestore { reason: format!("active external {} removed", o.connector) };
        }
    }
    // 2. Newly connected external → offer the selector (never auto-apply).
    for o in curr {
        if o.is_external && o.connected {
            let known_before = prev.iter().any(|p| p.connector == o.connector && p.connected);
            if !known_before {
                return DaemonAction::ShowSelector { connector: o.connector.clone() };
            }
        }
    }
    DaemonAction::Nothing
}

/// Loop guard: at most `max` restores per `window_ms` (ms clock, testable).
#[derive(Debug)]
pub struct LoopGuard {
    /// Window in ms.
    window_ms: u64,
    /// Max restores inside the window.
    max: u32,
    /// Recent restore timestamps.
    hits: Vec<u64>,
}

impl LoopGuard {
    /// New guard (F4 default: 2 restores / 60 s; the 3rd escalates).
    pub fn new(window_ms: u64, max: u32) -> Self {
        Self { window_ms, max, hits: Vec::new() }
    }

    /// True if another automatic restore may run now (no side effects).
    pub fn would_allow(&mut self, now_ms: u64) -> bool {
        self.hits.retain(|t| now_ms.saturating_sub(*t) < self.window_ms);
        (self.hits.len() as u32) < self.max
    }

    /// True if another automatic restore may run now (records the hit).
    /// Call only when the restore actually runs.
    pub fn allow(&mut self, now_ms: u64) -> bool {
        if !self.would_allow(now_ms) {
            return false;
        }
        self.hits.push(now_ms);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ext(connector: &str, connected: bool, enabled: bool) -> OutputView {
        OutputView { connector: connector.into(), connected, enabled, is_external: true }
    }

    fn int(connector: &str, enabled: bool) -> OutputView {
        OutputView { connector: connector.into(), connected: true, enabled, is_external: false }
    }

    fn ctx() -> DecideCtx {
        DecideCtx { transition_in_progress: false, suppress_self_echo: false, restore_allowed: true, snapshot_available: true }
    }

    #[test]
    fn new_external_offers_selector() {
        let prev = vec![int("eDP-1", true)];
        let curr = vec![int("eDP-1", true), ext("HDMI-A-1", true, false)];
        assert_eq!(decide(&prev, &curr, &ctx()), DaemonAction::ShowSelector { connector: "HDMI-A-1".into() });
    }

    #[test]
    fn lost_active_external_triggers_restore() {
        let prev = vec![int("eDP-1", false), ext("HDMI-A-1", true, true)];
        let curr = vec![int("eDP-1", false), ext("HDMI-A-1", false, false)];
        assert_eq!(
            decide(&prev, &curr, &ctx()),
            DaemonAction::AutoRestore { reason: "active external HDMI-A-1 removed".into() }
        );
    }

    #[test]
    fn lock_and_echo_suppress_actions() {
        let prev = vec![int("eDP-1", true)];
        let curr = vec![int("eDP-1", true), ext("HDMI-A-1", true, false)];
        let mut c = ctx();
        c.transition_in_progress = true;
        assert_eq!(decide(&prev, &curr, &c), DaemonAction::Nothing);
        c.transition_in_progress = false;
        c.suppress_self_echo = true;
        assert_eq!(decide(&prev, &curr, &c), DaemonAction::Nothing);
    }

    #[test]
    fn restore_exactly_once_then_loop_error() {
        let prev = vec![int("eDP-1", false), ext("HDMI-A-1", true, true)];
        let curr = vec![int("eDP-1", false), ext("HDMI-A-1", false, false)];
        let mut c = ctx();
        c.restore_allowed = false;
        assert_eq!(
            decide(&prev, &curr, &c),
            DaemonAction::ReportError("restore loop suspected for HDMI-A-1".into())
        );
        c.restore_allowed = true;
        c.snapshot_available = false;
        assert!(matches!(decide(&prev, &curr, &c), DaemonAction::ReportError(_)));
    }

    #[test]
    fn startup_and_steady_state_stay_quiet() {
        let curr = vec![int("eDP-1", true), ext("HDMI-A-1", true, true)];
        assert_eq!(decide(&[], &curr, &ctx()), DaemonAction::Nothing);
        assert_eq!(decide(&curr, &curr, &ctx()), DaemonAction::Nothing);
    }

    #[test]
    fn loop_guard_caps_restores() {
        let mut g = LoopGuard::new(60_000, 2);
        assert!(g.allow(0));
        assert!(g.allow(1_000));
        assert!(!g.allow(2_000)); // 3rd inside window: escalate
        assert!(g.allow(61_001)); // window passed: allowed again
    }
}
