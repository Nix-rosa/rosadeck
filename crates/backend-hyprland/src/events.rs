//! socket2 event parsing + read-only watch.
//!
//! Observed wire format: `EVENT>>DATA\n` (e.g. `monitoraddedv2>>2,HDMI-A-1,desc`).
//! F2 only classifies; no automatic action follows any event.

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// Hotplug-relevant display event (plus passthrough for the rest).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayEvent {
    /// `monitoradded[v2]>>...` — payload name best-effort parsed.
    MonitorAdded {
        /// Output name when extractable.
        name: Option<String>,
    },
    /// `monitorremoved[v2]>>...`.
    MonitorRemoved {
        /// Output name when extractable.
        name: Option<String>,
    },
    /// `configreloaded>>`.
    ConfigReloaded,
    /// Any other event, preserved verbatim.
    Other(String),
}

/// Parse one `EVENT>>DATA` line. Returns `None` for blank/garbage lines.
pub fn parse_event_line(line: &str) -> Option<DisplayEvent> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let (event, data) = line.split_once(">>").unwrap_or((line, ""));
    // v2 payloads are `ID,NAME,DESCRIPTION`; v1 payload is the bare name.
    let v2_name = || data.split(',').nth(1).filter(|s| !s.is_empty()).map(|s| s.to_owned());
    match event {
        "monitoradded" => Some(DisplayEvent::MonitorAdded { name: if data.contains(',') { v2_name() } else { (!data.is_empty()).then(|| data.to_owned()) } }),
        "monitoraddedv2" => Some(DisplayEvent::MonitorAdded { name: v2_name() }),
        "monitorremoved" => Some(DisplayEvent::MonitorRemoved { name: if data.contains(',') { v2_name() } else { (!data.is_empty()).then(|| data.to_owned()) } }),
        "monitorremovedv2" => Some(DisplayEvent::MonitorRemoved { name: v2_name() }),
        "configreloaded" => Some(DisplayEvent::ConfigReloaded),
        _ => Some(DisplayEvent::Other(line.to_owned())),
    }
}

/// Resolve the `.socket2.sock` path from the environment.
pub fn socket2_path() -> Result<String, String> {
    let runtime = std::env::var("XDG_RUNTIME_DIR").map_err(|_| "XDG_RUNTIME_DIR missing".to_owned())?;
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").map_err(|_| "HYPRLAND_INSTANCE_SIGNATURE missing".to_owned())?;
    Ok(format!("{runtime}/hypr/{sig}/.socket2.sock"))
}

/// Watch events until `timeout` elapses, calling `on_event` per event.
/// Read-only: opens the socket, reads lines, closes on timeout/EOF.
pub fn watch_events(timeout: Duration, mut on_event: impl FnMut(DisplayEvent)) -> Result<(), String> {
    let path = socket2_path()?;
    let stream = UnixStream::connect(&path).map_err(|e| format!("connect {path}: {e}"))?;
    stream.set_read_timeout(Some(timeout)).map_err(|e| format!("set timeout: {e}"))?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return Ok(()), // compositor closed
            Ok(_) => {
                if let Some(ev) = parse_event_line(&line) {
                    on_event(ev);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => return Ok(()),
            Err(e) => return Err(format!("socket2 read: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hotplug_events() {
        assert_eq!(
            parse_event_line("monitoraddedv2>>2,HDMI-A-1,Samsung TV"),
            Some(DisplayEvent::MonitorAdded { name: Some("HDMI-A-1".into()) })
        );
        assert_eq!(
            parse_event_line("monitorremovedv2>>1,HDMI-A-1,Samsung TV"),
            Some(DisplayEvent::MonitorRemoved { name: Some("HDMI-A-1".into()) })
        );
        assert_eq!(parse_event_line("monitoradded>>DP-1"), Some(DisplayEvent::MonitorAdded { name: Some("DP-1".into()) }));
        assert_eq!(parse_event_line("configreloaded>>"), Some(DisplayEvent::ConfigReloaded));
        assert_eq!(
            parse_event_line("workspace>>2"),
            Some(DisplayEvent::Other("workspace>>2".into()))
        );
        assert_eq!(parse_event_line("   "), None);
        assert_eq!(parse_event_line("garbage-without-separator"), Some(DisplayEvent::Other("garbage-without-separator".into())));
    }
}
