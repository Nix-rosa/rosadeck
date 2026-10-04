//! Launch session state machine + hotplug contract.
//!
//! The session tracks one game run from preparation through display restore.
//! Display work itself is done by the caller (CLI pipeline) via the F3
//! backend; the session only records phases so illegal transitions
//! (e.g. `Running → Preparing`) are rejected, and so a `DisplayLost` event
//! has a well-defined meaning in every phase.

use serde::{Deserialize, Serialize};

/// Session lifecycle (terminal: `Finished`, `Failed`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    /// Created, nothing done.
    Idle,
    /// `prepare()` description built.
    Preparing,
    /// Display transition applied, not yet verified.
    DisplayApplying,
    /// Display verified, emulator not yet spawned.
    DisplayVerified,
    /// Child spawned, not yet observed running.
    Launching,
    /// Child running.
    Running,
    /// Stop requested (exit, crash observed, signal, display lost).
    Stopping,
    /// Releasing adapter resources (idempotent).
    Cleaning,
    /// Re-applying the pre-launch snapshot.
    Restoring,
    /// Terminal success.
    Finished,
    /// Terminal failure (reason preserved).
    Failed(String),
}

impl SessionState {
    /// Terminal states never leave.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Finished | Self::Failed(_))
    }
}

/// What `on_display_lost` decided (contract for the daemon, §26).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayLostOutcome {
    /// Session already terminal: nothing to do.
    AlreadyFinished,
    /// Child running (or launching): caller should stop it, then restore.
    StopAndRestore,
    /// No child yet: just restore the display.
    RestoreOnly,
}

/// One game run.
#[derive(Debug, Clone)]
pub struct EmulatorSession {
    /// Current phase.
    pub state: SessionState,
    /// Adapter id for logs.
    pub emulator_id: String,
    /// Game title for logs.
    pub game_title: String,
    /// Cleanup calls so far (idempotence witness).
    pub cleanups: u32,
}

impl EmulatorSession {
    /// New idle session.
    pub fn new(emulator_id: &str, game_title: &str) -> Self {
        Self { state: SessionState::Idle, emulator_id: emulator_id.into(), game_title: game_title.into(), cleanups: 0 }
    }

    /// Validated step; illegal transitions (incl. leaving terminal states)
    /// are rejected, never panicked.
    pub fn step(&mut self, next: SessionState) -> Result<(), String> {
        if self.state.is_terminal() {
            return Err(format!("session terminal ({:?}), cannot step to {next:?}", self.state));
        }
        let legal = matches!(
            (&self.state, &next),
            (SessionState::Idle, SessionState::Preparing)
                | (SessionState::Preparing, SessionState::DisplayApplying)
                // No display work needed: the game launches on the current
                // setup (the library's default, and the only path when no
                // external output is attached).
                | (SessionState::Preparing, SessionState::Launching)
                | (SessionState::DisplayApplying, SessionState::DisplayVerified)
                | (SessionState::DisplayApplying, SessionState::Failed(_))
                | (SessionState::DisplayVerified, SessionState::Launching)
                | (SessionState::DisplayVerified, SessionState::Failed(_))
                | (SessionState::Launching, SessionState::Running)
                | (SessionState::Launching, SessionState::Stopping)
                | (SessionState::Launching, SessionState::Failed(_))
                | (SessionState::Running, SessionState::Stopping)
                | (SessionState::Running, SessionState::Failed(_))
                | (SessionState::Stopping, SessionState::Cleaning)
                | (SessionState::Stopping, SessionState::Failed(_))
                | (SessionState::Cleaning, SessionState::Restoring)
                | (SessionState::Cleaning, SessionState::Finished)
                | (SessionState::Cleaning, SessionState::Failed(_))
                | (SessionState::Restoring, SessionState::Finished)
                | (SessionState::Restoring, SessionState::Failed(_))
                | (_, SessionState::Failed(_))
        );
        if !legal {
            return Err(format!("illegal session transition: {:?} -> {next:?}", self.state));
        }
        self.state = next;
        Ok(())
    }

    /// Hotplug contract (§26): map the external-display loss to an outcome.
    /// No dynamic output migration in F5: running sessions stop, then the
    /// caller restores the display through the F3 path.
    pub fn on_display_lost(&self) -> DisplayLostOutcome {
        match self.state {
            SessionState::Finished | SessionState::Failed(_) => DisplayLostOutcome::AlreadyFinished,
            SessionState::Launching | SessionState::Running | SessionState::Stopping => {
                DisplayLostOutcome::StopAndRestore
            }
            _ => DisplayLostOutcome::RestoreOnly,
        }
    }

    /// Record a cleanup pass (the adapter's `cleanup` stays idempotent;
    /// this counter only witnesses calls in tests).
    pub fn note_cleanup(&mut self) {
        self.cleanups += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running_session() -> EmulatorSession {
        let mut s = EmulatorSession::new("dolphin", "MK");
        s.step(SessionState::Preparing).unwrap();
        s.step(SessionState::DisplayApplying).unwrap();
        s.step(SessionState::DisplayVerified).unwrap();
        s.step(SessionState::Launching).unwrap();
        s.step(SessionState::Running).unwrap();
        s
    }

    #[test]
    fn happy_path_states() {
        let mut s = running_session();
        s.step(SessionState::Stopping).unwrap();
        s.step(SessionState::Cleaning).unwrap();
        s.step(SessionState::Restoring).unwrap();
        s.step(SessionState::Finished).unwrap();
        assert!(s.state.is_terminal());
    }

    #[test]
    fn rejects_illegal_and_terminal_steps() {
        let mut s = EmulatorSession::new("dolphin", "MK");
        assert!(s.step(SessionState::Running).is_err());
        s.step(SessionState::Preparing).unwrap();
        assert!(s.step(SessionState::Finished).is_err()); // must clean first
        s.step(SessionState::Failed("x".into())).unwrap(); // failure from anywhere
        assert!(s.step(SessionState::Idle).is_err());
    }

    #[test]
    fn display_lost_contract() {
        assert_eq!(running_session().on_display_lost(), DisplayLostOutcome::StopAndRestore);
        assert_eq!(EmulatorSession::new("d", "g").on_display_lost(), DisplayLostOutcome::RestoreOnly);
        let mut s = running_session();
        s.step(SessionState::Failed("boom".into())).unwrap();
        assert_eq!(s.on_display_lost(), DisplayLostOutcome::AlreadyFinished);
    }

    #[test]
    fn launching_without_a_display_transition_is_legal() {
        // The library's path: no display work at all (e.g. no HDMI attached).
        let mut s = EmulatorSession::new("dolphin", "Game");
        assert!(s.step(SessionState::Preparing).is_ok());
        assert!(s.step(SessionState::Launching).is_ok(), "must not require a display step");
        assert!(s.step(SessionState::Running).is_ok());
        assert!(s.step(SessionState::Stopping).is_ok());
        assert!(s.step(SessionState::Cleaning).is_ok());
        assert!(s.step(SessionState::Finished).is_ok());
    }

    #[test]
    fn display_path_still_works_and_skipping_verification_does_not() {
        let mut s = EmulatorSession::new("dolphin", "Game");
        s.step(SessionState::Preparing).unwrap();
        s.step(SessionState::DisplayApplying).unwrap();
        assert!(s.step(SessionState::Launching).is_err(), "cannot skip verification");
        assert!(s.step(SessionState::DisplayVerified).is_ok());
        assert!(s.step(SessionState::Launching).is_ok());
    }
}
