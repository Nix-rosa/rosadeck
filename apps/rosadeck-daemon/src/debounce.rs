//! Debounce: one cable event → many compositor events → one reconciliation.
//!
//! Pure over an injected millisecond clock (fully testable). Default window
//! 750 ms (Decision, in the 500–1000 ms research range): long enough to cover
//! a `monitoraddedv2 + configreloaded` burst, short enough to feel instant.

use rosadeck_backend_hyprland::DisplayEvent;

/// Coalescing event buffer with a quiet-period window.
#[derive(Debug, Default)]
pub struct Debouncer {
    /// Quiet period in ms.
    window_ms: u64,
    /// Buffered events.
    pending: Vec<DisplayEvent>,
    /// Clock of the last push.
    last_push_ms: Option<u64>,
}

impl Debouncer {
    /// New debouncer with the given quiet window.
    pub fn new(window_ms: u64) -> Self {
        Self { window_ms, pending: Vec::new(), last_push_ms: None }
    }

    /// Buffer one event at clock `now_ms`.
    pub fn push(&mut self, ev: DisplayEvent, now_ms: u64) {
        self.pending.push(ev);
        self.last_push_ms = Some(now_ms);
    }

    /// Buffered events, if any.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Drain the batch once the line has been quiet for `window_ms`.
    /// Empty while events keep arriving or nothing is buffered.
    pub fn drain_ready(&mut self, now_ms: u64) -> Vec<DisplayEvent> {
        match self.last_push_ms {
            Some(last) if !self.pending.is_empty() && now_ms.saturating_sub(last) >= self.window_ms => {
                self.last_push_ms = None;
                std::mem::take(&mut self.pending)
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesces_bursts_into_one_batch() {
        let mut d = Debouncer::new(750);
        d.push(DisplayEvent::MonitorAdded { name: Some("HDMI-A-1".into()) }, 1000);
        d.push(DisplayEvent::ConfigReloaded, 1200);
        assert!(d.drain_ready(1300).is_empty()); // still within window
        let batch = d.drain_ready(2000);
        assert_eq!(batch.len(), 2); // one reconciliation for the burst
        assert!(d.drain_ready(3000).is_empty());
    }

    #[test]
    fn empty_when_idle() {
        let mut d = Debouncer::new(750);
        assert!(d.drain_ready(9999).is_empty());
    }
}
