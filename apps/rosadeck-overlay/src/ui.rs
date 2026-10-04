//! Pure overlay UI: render + key handling (no terminal IO here).
//!
//! States mirror the required overlay lifecycle: Selecting → Applying →
//! Success / Error. The runtime (`main.rs`) only drives these functions.

use display_core::{Mode, ModePolicy};
use rosadeck_selector::{Selection, SelectionContext, SelectorAction};

/// Visible overlay state.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Applying/Success/Error complete the protocol; driven by the daemon.
pub enum OverlayState {
    /// Browsing actions/modes.
    Selecting {
        /// Action cursor.
        action_idx: usize,
        /// Mode cursor within the current action's options.
        mode_idx: usize,
        /// Whether a manual mode is picked (`None` = policy decides).
        manual: Option<Mode>,
    },
    /// Transition running (set by the daemon protocol, shown while waiting).
    Applying {
        /// Summary line.
        summary: String,
    },
    /// Transition verified.
    Success,
    /// Transition failed, previous display restored (or restore failed).
    Error {
        /// Message to show.
        message: String,
    },
}

/// Result of one key press.
#[derive(Debug, Clone, PartialEq)]
pub enum KeyOutcome {
    /// Keep selecting.
    Continue,
    /// User confirmed with this selection.
    Confirmed(Selection),
    /// User cancelled (Esc / Cancel action).
    Cancelled,
}

/// Move cursors / confirm / cancel. Pure; crossterm keys map in `main.rs`.
/// `action_idx`/`mode_idx` wrap around. Left/Right switch actions, Up/Down
/// switch modes, `m` toggles manual pick of the highlighted mode, Enter
/// confirms, Esc cancels.
pub fn handle_key(
    ctx: &SelectionContext,
    action_idx: usize,
    mode_idx: usize,
    manual: Option<Mode>,
    key: Key,
) -> (usize, usize, Option<Mode>, KeyOutcome) {
    let n_actions = ctx.actions.len().max(1);
    let modes = &ctx.modes_per_action[action_idx.min(n_actions - 1)];
    let n_modes = modes.len().max(1);
    match key {
        Key::Left => ((action_idx + n_actions - 1) % n_actions, 0, None, KeyOutcome::Continue),
        Key::Right => ((action_idx + 1) % n_actions, 0, None, KeyOutcome::Continue),
        Key::Up => (action_idx, (mode_idx + n_modes - 1) % n_modes, manual, KeyOutcome::Continue),
        Key::Down => (action_idx, (mode_idx + 1) % n_modes, manual, KeyOutcome::Continue),
        Key::Manual => {
            let pick = modes.get(mode_idx).copied();
            (action_idx, mode_idx, pick, KeyOutcome::Continue)
        }
        Key::Enter => {
            let action = ctx.actions[action_idx];
            if action == SelectorAction::Cancel {
                return (action_idx, mode_idx, manual, KeyOutcome::Cancelled);
            }
            let (policy, mode) = match manual {
                Some(m) => (ModePolicy::Manual(m), Some(m)),
                None => (ModePolicy::MaxRefresh, None),
            };
            (action_idx, mode_idx, manual, KeyOutcome::Confirmed(Selection { action, mode, policy }))
        }
        Key::Esc => (action_idx, mode_idx, manual, KeyOutcome::Cancelled),
    }
}

/// Toolkit-agnostic key (mapped from crossterm in `main.rs`; testable here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// ← previous action.
    Left,
    /// → next action.
    Right,
    /// ↑ previous mode.
    Up,
    /// ↓ next mode.
    Down,
    /// Toggle manual mode pick.
    Manual,
    /// Confirm.
    Enter,
    /// Cancel.
    Esc,
}

/// Inner content width (box borders add 2 → 38 total).
const INNER: usize = 36;

/// Pad/truncate to exactly `INNER` chars (box-drawing and arrows are
/// single-width; CJK would need wcwidth — out of scope, noted).
fn pad(s: &str) -> String {
    let mut out: String = s.chars().take(INNER).collect();
    while out.chars().count() < INNER {
        out.push(' ');
    }
    out
}

fn line(s: &str) -> String {
    format!("│{}│\n", pad(s))
}

/// Render the whole overlay as text (a `String` so tests run headless).
pub fn render(ctx: &SelectionContext, state: &OverlayState) -> String {
    let mut out = String::new();
    match state {
        OverlayState::Selecting { action_idx, mode_idx, manual } => {
            out.push_str("╭────────────────────────────────────╮\n");
            out.push_str(&line("          DISPLAY DETECTED"));
            out.push_str(&line(""));
            out.push_str(&line(&format!("  {}", truncate(&ctx.title, 32))));
            out.push_str(&line(&format!("  {}", truncate(&ctx.subtitle, 32))));
            out.push_str(&line(""));
            for (i, a) in ctx.actions.iter().enumerate() {
                let mark = if i == *action_idx { "▶" } else { " " };
                out.push_str(&line(&format!("  {mark} {}", a.label())));
            }
            let modes = &ctx.modes_per_action[*action_idx.min(&ctx.actions.len().saturating_sub(1))];
            if modes.is_empty() {
                out.push_str(&line("  policy decides the mode"));
            } else {
                out.push_str(&line("  Modes (m = pick manually):"));
                for (i, m) in modes.iter().enumerate().take(4) {
                    let mark = if Some(*m) == *manual { "●" } else if i == *mode_idx { "▸" } else { " " };
                    out.push_str(&line(&format!("  {mark} {m}")));
                }
                if modes.len() > 4 {
                    out.push_str(&line(&format!("    … +{} more", modes.len() - 4)));
                }
            }
            out.push_str(&line(""));
            out.push_str(&line("  < > action - ^ v mode - m manual"));
            out.push_str(&line("  Enter confirm - Esc cancel"));
            out.push_str("╰────────────────────────────────────╯\n");
        }
        OverlayState::Applying { summary } => {
            out.push_str("╭────────────────────────────────────╮\n");
            out.push_str(&line("            APPLYING..."));
            out.push_str(&line(&format!("  {}", truncate(summary, 32))));
            out.push_str(&line("  Verifying display..."));
            out.push_str("╰────────────────────────────────────╯\n");
        }
        OverlayState::Success => {
            out.push_str("╭────────────────────────────────────╮\n");
            out.push_str(&line("        TRANSITION COMPLETE"));
            out.push_str("╰────────────────────────────────────╯\n");
        }
        OverlayState::Error { message } => {
            out.push_str("╭────────────────────────────────────╮\n");
            out.push_str(&line("         TRANSITION FAILED"));
            out.push_str(&line(&format!("  {}", truncate(message, 32))));
            out.push_str(&line("  Enter  Continue"));
            out.push_str("╰────────────────────────────────────╯\n");
        }
    }
    out
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_owned()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use display_core::{parse_mode, OutputId};
    use rosadeck_selector::{build_context, ContextArgs};

    fn ctx() -> SelectionContext {
        build_context(&ContextArgs {
            connector: OutputId("HDMI-A-1".into()),
            edid_name: Some("Samsung TV".into()),
            identity: None,
            current: None,
            internal_modes: vec![parse_mode("1920x1080@60").unwrap()],
            external_modes: vec![parse_mode("3840x2160@120").unwrap(), parse_mode("1920x1080@60").unwrap()],
            profile: None,
        })
    }

    #[test]
    fn renders_all_states_headless() {
        let c = ctx();
        let s = render(&c, &OverlayState::Selecting { action_idx: 0, mode_idx: 0, manual: None });
        assert!(s.contains("DISPLAY DETECTED") && s.contains("Samsung TV"));
        assert!(render(&c, &OverlayState::Applying { summary: "External Only".into() }).contains("APPLYING"));
        assert!(render(&c, &OverlayState::Success).contains("COMPLETE"));
        assert!(render(&c, &OverlayState::Error { message: "NO_COMMON_MODE".into() }).contains("FAILED"));
    }

    #[test]
    fn every_line_has_exact_box_width() {
        let c = ctx();
        // Long titles must not break the box either.
        let mut wide = c.clone();
        wide.title = "A very long monitor name that exceeds the inner width easily".into();
        for state in [
            OverlayState::Selecting { action_idx: 0, mode_idx: 0, manual: None },
            OverlayState::Selecting { action_idx: 2, mode_idx: 1, manual: Some(parse_mode("1920x1080@60").unwrap()) },
            OverlayState::Applying { summary: "External Only".into() },
            OverlayState::Success,
            OverlayState::Error { message: "NO_COMMON_MODE".into() },
        ] {
            for target in [&c, &wide] {
                let out = render(target, &state);
                let widths: std::collections::HashSet<usize> =
                    out.lines().map(|l| l.chars().count()).collect();
                assert_eq!(widths.len(), 1, "ragged box in {state:?}:\n{out}");
            }
        }
    }

    #[test]
    fn keys_navigate_confirm_cancel() {
        let c = ctx();
        // Right wraps through actions; Enter on non-cancel confirms.
        let (a, _, _, o) = handle_key(&c, 0, 0, None, Key::Right);
        assert_eq!(o, KeyOutcome::Continue);
        assert!(a < c.actions.len());
        let (_, _, _, o) = handle_key(&c, 0, 0, None, Key::Enter);
        assert!(matches!(o, KeyOutcome::Confirmed(_)));
        let cancel_idx = c.actions.iter().position(|a| *a == SelectorAction::Cancel).unwrap();
        let (_, _, _, o) = handle_key(&c, cancel_idx, 0, None, Key::Enter);
        assert_eq!(o, KeyOutcome::Cancelled);
        let (_, _, _, o) = handle_key(&c, 0, 0, None, Key::Esc);
        assert_eq!(o, KeyOutcome::Cancelled);
    }

    #[test]
    fn manual_pick_flows_into_selection() {
        let c = ctx();
        let ext_idx = c.actions.iter().position(|a| *a == SelectorAction::ExternalOnly).unwrap();
        let (_, _, manual, _) = handle_key(&c, ext_idx, 0, None, Key::Manual);
        assert!(manual.is_some());
        let (_, _, _, o) = handle_key(&c, ext_idx, 0, manual, Key::Enter);
        match o {
            KeyOutcome::Confirmed(s) => assert!(matches!(s.policy, ModePolicy::Manual(_))),
            _ => panic!("expected confirm"),
        }
    }
}
