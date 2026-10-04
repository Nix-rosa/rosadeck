//! `rosadeck-overlay` runtime: crossterm event loop over pure `ui.rs`.
//!
//! Protocol with the daemon: `--context <json>` in, `--result <file>` out
//! (a `Selection` JSON). Exit 0 = confirmed (result written), 3 = cancelled,
//! 2 = usage/IO error. `--dump-sample` renders a demo without a TTY (for
//! headless validation). This binary never touches Hyprland.

mod ui;
mod wofi;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use rosadeck_selector::SelectionContext;
use crossterm::tty::IsTty;
use std::io::{stdout, Write};
use ui::{handle_key, render, Key, KeyOutcome, OverlayState};

fn map_key(code: KeyCode) -> Option<Key> {
    match code {
        KeyCode::Left => Some(Key::Left),
        KeyCode::Right => Some(Key::Right),
        KeyCode::Up => Some(Key::Up),
        KeyCode::Down => Some(Key::Down),
        KeyCode::Char('m') | KeyCode::Char('M') => Some(Key::Manual),
        KeyCode::Enter => Some(Key::Enter),
        KeyCode::Esc => Some(Key::Esc),
        _ => None,
    }
}

fn sample_context() -> SelectionContext {
    serde_json::from_value(serde_json::json!({
        "connector": "HDMI-A-1",
        "title": "Samsung TV",
        "subtitle": "3840x2160 @ 120 Hz",
        "actions": ["ExternalOnly", "Extend", "Cancel"],
        "profile": null,
        "modes_per_action": [
            [{"width": 3840, "height": 2160, "hz": 120.0}, {"width": 1920, "height": 1080, "hz": 120.0}],
            [{"width": 3840, "height": 2160, "hz": 120.0}],
            []
        ]
    }))
    .expect("sample context")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--dump-sample") {
        let ctx = sample_context();
        print!("{}", render(&ctx, &OverlayState::Selecting { action_idx: 0, mode_idx: 0, manual: None }));
        return;
    }
    let context_path = args.iter().skip_while(|a| *a != "--context").nth(1).cloned();
    let result_path = args.iter().skip_while(|a| *a != "--result").nth(1).cloned();
    let wofi_mode = args.iter().any(|a| a == "--wofi");
    let (Some(cpath), Some(rpath)) = (context_path, result_path) else {
        eprintln!("usage: rosadeck-overlay --context <json> --result <file> [--wofi] | --dump-sample");
        std::process::exit(2);
    };
    let text = std::fs::read_to_string(&cpath).unwrap_or_else(|e| {
        eprintln!("overlay: cannot read context: {e}");
        std::process::exit(2);
    });
    let ctx: SelectionContext = serde_json::from_str(&text).unwrap_or_else(|e| {
        eprintln!("overlay: bad context: {e}");
        std::process::exit(2);
    });
    if wofi_mode {
        match wofi::run_wofi(&ctx) {
            Ok(Some(sel)) => {
                let json = serde_json::to_string_pretty(&sel).unwrap_or_default();
                if std::fs::write(&rpath, json).is_err() {
                    std::process::exit(2);
                }
                std::process::exit(0);
            }
            Ok(None) => std::process::exit(3),
            Err(e) => {
                eprintln!("overlay wofi: {e}");
                std::process::exit(2);
            }
        }
    }
    if !stdout().is_tty() {
        eprintln!("overlay: no TTY (run inside a floating terminal)");
        std::process::exit(2);
    }
    enable_raw_mode().expect("raw mode");
    let mut out = stdout();
    out.execute(EnterAlternateScreen).ok();
    out.execute(Hide).ok();
    let mut action_idx = 0usize;
    let mut mode_idx = 0usize;
    let mut manual = None;
    let code = loop {
        use std::fmt::Write as _;
        let mut screen = String::new();
        let _ = write!(screen, "{}", render(&ctx, &OverlayState::Selecting { action_idx, mode_idx, manual }));
        out.execute(crossterm::terminal::Clear(crossterm::terminal::ClearType::All)).ok();
        out.execute(MoveTo(0, 0)).ok();
        // CRLF + no last column: raw mode clears OPOST (see tui-frame).
        let width = crossterm::terminal::size().map(|(w, _)| w).unwrap_or(80);
        rosadeck_tui_frame::write_frame(&mut out, &screen, width).ok();
        out.flush().ok();
        let outcome = match event::read() {
            // kitty sends key releases too; ignore them (no double steps).
            Ok(Event::Key(k)) if k.kind != KeyEventKind::Release => {
                map_key(k.code).map(|key| handle_key(&ctx, action_idx, mode_idx, manual, key))
            }
            _ => None,
        };
        match outcome {
            None => continue,
            Some((a, m, man, outcome)) => {
                action_idx = a;
                mode_idx = m;
                manual = man;
                match outcome {
                    KeyOutcome::Continue => continue,
                    KeyOutcome::Cancelled => break 3,
                    KeyOutcome::Confirmed(sel) => {
                        let json = serde_json::to_string_pretty(&sel).unwrap_or_default();
                        if std::fs::write(&rpath, json).is_err() {
                            break 2;
                        }
                        break 0;
                    }
                }
            }
        }
    };
    out.execute(LeaveAlternateScreen).ok();
    out.execute(Show).ok();
    disable_raw_mode().ok();
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    #[test]
    fn runtime_writes_frames_through_tui_frame() {
        // Bare write! in raw mode stair-steps the frame (cfmakeraw clears OPOST).
        let prod = include_str!("main.rs").split("#[cfg(test)]").next().unwrap();
        assert!(prod.contains("rosadeck_tui_frame::write_frame"), "overlay must write frames via tui-frame");
        assert!(!prod.contains("write!(out, \"{screen}\")"), "overlay must not write frames with bare write!");
    }
}
