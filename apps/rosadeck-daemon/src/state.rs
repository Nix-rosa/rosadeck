//! Explicit daemon state machine: impossible transitions are rejected.
//!
//! `Applying → Applying` (concurrent transitions) can never happen: the only
//! path into [`DaemonState::Applying`] requires `can_start_transition()`.

/// Daemon lifecycle state (F4 + F6 gaming awareness).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonState {
    /// Waiting for events.
    Idle,
    /// Debounced batch under reconciliation.
    Reconciling,
    /// Overlay open, awaiting the user's selection.
    AwaitingSelection,
    /// A transition (CLI invocation) is running.
    Applying,
    /// Reserved: verify currently runs inside the delegated CLI call, so the
    /// daemon never enters this state yet; kept to complete the model for F5.
    #[allow(dead_code)]
    Verifying,
    /// Rolling back to the snapshot.
    Restoring,
    /// External gaming session active (handle file present).
    GamingSession,
    /// Gaming session restore running.
    RestoringSession,
    /// Terminal/error note (message preserved, loop continues cautiously).
    Error(String),
}

impl DaemonState {
    /// True while snapshot/apply/verify/restore is in flight.
    pub fn transition_in_progress(&self) -> bool {
        matches!(self, Self::Applying | Self::Verifying | Self::Restoring | Self::RestoringSession)
    }

    /// Only these states may start a new transition.
    pub fn can_start_transition(&self) -> bool {
        matches!(self, Self::Idle | Self::AwaitingSelection | Self::Error(_))
    }

    /// Validated step; `Err` names the illegal transition (never panics).
    pub fn step(&self, next: DaemonState) -> Result<DaemonState, String> {
        if self.transition_in_progress() && next.transition_in_progress() && *self != DaemonState::Applying {
            // Verifying/Restoring advance internally; external requests wait.
        }
        let illegal = matches!(
            (&self, &next),
            (DaemonState::Applying, DaemonState::Applying)
                | (DaemonState::Applying, DaemonState::AwaitingSelection)
                | (DaemonState::Verifying, DaemonState::Applying)
                | (DaemonState::Restoring, DaemonState::Applying)
                | (DaemonState::GamingSession, DaemonState::GamingSession)
                | (DaemonState::RestoringSession, DaemonState::Applying)
        );
        if illegal {
            return Err(format!("illegal daemon transition: {self:?} -> {next:?}"));
        }
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_concurrent_transitions() {
        let s = DaemonState::Applying;
        assert!(s.step(DaemonState::Applying).is_err());
        assert!(s.step(DaemonState::AwaitingSelection).is_err());
        assert!(s.step(DaemonState::Verifying).is_ok());
        assert!(DaemonState::Idle.step(DaemonState::Applying).is_ok());
        assert!(DaemonState::Idle.step(DaemonState::Reconciling).is_ok());
        // No nested gaming sessions without an explicit exit.
        assert!(DaemonState::GamingSession.step(DaemonState::GamingSession).is_err());
        assert!(DaemonState::GamingSession.step(DaemonState::RestoringSession).is_ok());
    }

    #[test]
    fn lock_predicate() {
        assert!(DaemonState::Applying.transition_in_progress());
        assert!(DaemonState::Verifying.transition_in_progress());
        assert!(DaemonState::Restoring.transition_in_progress());
        assert!(DaemonState::RestoringSession.transition_in_progress());
        assert!(!DaemonState::GamingSession.transition_in_progress());
        assert!(!DaemonState::Idle.transition_in_progress());
        assert!(!DaemonState::AwaitingSelection.transition_in_progress());
    }
}
