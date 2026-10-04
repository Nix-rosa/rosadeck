//! wofi frontend: pure menu builders + thin dmenu runner.
//!
//! Two passes: actions first, then the action's real modes (with an `Auto`
//! entry meaning "policy decides"). wofi exit≠0 / empty output = cancel.
//! `--cache-file /dev/null` avoids polluting wofi history with modes.

use display_core::Mode;
use rosadeck_selector::{Selection, SelectionContext, SelectorAction};
use std::io::Write;

/// First entry of the mode pass: let the policy decide.
pub const AUTO_ENTRY: &str = "Auto (policy decides)";

/// Action labels in presentation order.
pub fn action_entries(ctx: &SelectionContext) -> Vec<String> {
    ctx.actions.iter().map(|a| a.label().to_owned()).collect()
}

/// Mode entries for one action (`Auto` first, then real modes).
pub fn mode_entries(ctx: &SelectionContext, action_idx: usize) -> Vec<String> {
    let mut out = vec![AUTO_ENTRY.to_owned()];
    if let Some(modes) = ctx.modes_per_action.get(action_idx) {
        out.extend(modes.iter().map(|m| m.to_string()));
    }
    out
}

/// Match a wofi action line back to its index.
pub fn parse_action(ctx: &SelectionContext, line: &str) -> Option<usize> {
    ctx.actions.iter().position(|a| a.label() == line.trim())
}

/// Match a wofi mode line to `Some(mode)` (`Auto`/empty → `None` = policy).
pub fn parse_mode_pick(ctx: &SelectionContext, action_idx: usize, line: &str) -> Option<Option<Mode>> {
    let line = line.trim();
    if line.is_empty() || line == AUTO_ENTRY {
        return Some(None);
    }
    ctx.modes_per_action
        .get(action_idx)?
        .iter()
        .find(|m| m.to_string() == line)
        .copied()
        .map(Some)
}

/// Run one wofi pass; `Ok(None)` = cancelled.
pub fn wofi_pick(prompt: &str, entries: &[String], lines: usize) -> Result<Option<String>, String> {
    let mut child = std::process::Command::new("wofi")
        .args([
            "--dmenu".to_owned(),
            "--prompt".to_owned(),
            prompt.to_owned(),
            "--lines".to_owned(),
            lines.to_string(),
            "--cache-file".to_owned(),
            "/dev/null".to_owned(),
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn wofi: {e}"))?;
    {
        let stdin = child.stdin.as_mut().ok_or("wofi stdin")?;
        stdin.write_all(entries.join("\n").as_bytes()).map_err(|e| format!("wofi stdin: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("wofi wait: {e}"))?;
    if !out.status.success() {
        return Ok(None); // Esc / close = cancel
    }
    let line = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    Ok(if line.is_empty() { None } else { Some(line) })
}

/// Full wofi flow for a context: action pass, then mode pass when the action
/// offers real modes. Returns the `Selection` (policy MaxRefresh unless a
/// manual mode was picked).
pub fn run_wofi(ctx: &SelectionContext) -> Result<Option<Selection>, String> {
    let actions = action_entries(ctx);
    let Some(line) = wofi_pick(&format!("Display: {}", ctx.title), &actions, actions.len().max(3))? else {
        return Ok(None);
    };
    let Some(idx) = parse_action(ctx, &line) else { return Ok(None) };
    let action = ctx.actions[idx];
    if action == SelectorAction::Cancel {
        return Ok(None);
    }
    let modes = mode_entries(ctx, idx);
    let pick = if modes.len() > 1 {
        let Some(mline) = wofi_pick(&format!("{}: mode", action.label()), &modes, modes.len().min(10))? else {
            return Ok(None);
        };
        parse_mode_pick(ctx, idx, &mline).unwrap_or(None)
    } else {
        None
    };
    let (policy, mode) = match pick {
        Some(m) => (display_core::ModePolicy::Manual(m), Some(m)),
        None => (display_core::ModePolicy::MaxRefresh, None),
    };
    Ok(Some(Selection { action, mode, policy }))
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
    fn entries_roundtrip() {
        let c = ctx();
        let actions = action_entries(&c);
        assert!(actions.contains(&"External Only".to_owned()));
        let idx = parse_action(&c, "External Only").unwrap();
        assert_eq!(c.actions[idx], SelectorAction::ExternalOnly);
        assert!(parse_action(&c, "Nope").is_none());
        let modes = mode_entries(&c, idx);
        assert_eq!(modes[0], AUTO_ENTRY);
        assert_eq!(parse_mode_pick(&c, idx, AUTO_ENTRY), Some(None));
        assert_eq!(parse_mode_pick(&c, idx, "1920x1080@60.000"), Some(Some(parse_mode("1920x1080@60").unwrap())));
        assert_eq!(parse_mode_pick(&c, idx, "bogus"), None);
    }
}
