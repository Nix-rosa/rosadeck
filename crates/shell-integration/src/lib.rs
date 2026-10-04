//! Read-only shell probes: Quickshell detection/IPC and wallpaper providers.
//!
//! Boundary: this crate PROBES (process list, `qs ipc show`, `awww query`)
//! and reports [`Capability`] states. It never writes user configs, never
//! calls state-changing IPC, never kills/restarts shells. Repositioning a
//! shell is the shell's own job (screen-aware config); Rosadeck only asks
//! through existing handlers when they exist.

use rosadeck_session_core::Capability;
use serde::{Deserialize, Serialize};

/// What the running Quickshell looks like (observed, not assumed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickshellState {
    /// A quickshell process exists.
    pub running: bool,
    /// PIDs observed (for logs only).
    pub pids: Vec<u32>,
    /// `qs` CLI available.
    pub cli_available: bool,
    /// Config file in use (`~/.config/quickshell/shell.qml` default).
    pub config_path: Option<String>,
    /// IPC targets from `qs ipc show` (empty when unprobed/failed).
    pub ipc_targets: Vec<String>,
    /// Screen-migration capability: `Available` only when a handler for it
    /// is observed; the reference shell exposes toggles, so real shells land
    /// on `NotConfigured` (warn + continue, never a requirement).
    pub screen_migration: Capability,
}

/// Detected wallpaper provider (observed via binaries/processes/queries).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WallpaperState {
    /// Provider kind.
    pub provider: rosadeck_session_core::WallpaperProviderKind,
    /// Per-output current image (`output → image`), when queryable.
    pub current: Vec<(String, String)>,
    /// Re-application capability.
    pub reapply: Capability,
}

/// Scan `/proc` for a process name (read-only, no `pgrep` dependency).
fn pids_named(name: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/proc") else { return out };
    for e in rd.filter_map(|e| e.ok()) {
        let pid: u32 = match e.file_name().to_string_lossy().parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let comm = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default();
        if comm.trim() == name {
            out.push(pid);
        }
    }
    out.sort();
    out
}

fn in_path(binary: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|d| d.join(binary).is_file())
    })
}

/// Parse `qs ipc show` output into target names (pure, fixture-testable).
/// Observed format:
/// ```text
/// target wifi
///   function toggle(): void
/// ```
pub fn parse_ipc_targets(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("target ").map(|s| s.trim().to_owned()))
        .filter(|s| !s.is_empty())
        .collect()
}

/// Parse `awww query` output into `(output, image)` pairs (pure).
/// Observed format:
/// ```text
/// : eDP-1: 1280x720, scale: 1.5, currently displaying: image: /path.png
/// ```
pub fn parse_awww_query(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim().trim_start_matches(':').trim();
        let Some((output, rest)) = line.split_once(':') else { continue };
        if let Some((_, image)) = rest.split_once("currently displaying: image:") {
            out.push((output.trim().to_owned(), image.trim().to_owned()));
        }
    }
    out
}

/// Detect the running Quickshell (process scan + config presence; IPC
/// targets filled by [`probe_ipc_targets`] on demand).
pub fn detect_quickshell() -> QuickshellState {
    let pids = pids_named("quickshell");
    let config = std::env::var("HOME").ok().map(|h| format!("{h}/.config/quickshell/shell.qml"));
    let config_path = config.filter(|p| std::path::Path::new(p).is_file());
    QuickshellState {
        running: !pids.is_empty(),
        pids,
        cli_available: in_path("qs"),
        config_path,
        ipc_targets: Vec::new(),
        screen_migration: Capability::Unsupported,
    }
}

/// Read-only IPC probe: `qs ipc show` (never `call`/`prop` here).
pub fn probe_ipc_targets(state: &mut QuickshellState) {
    if !state.cli_available || !state.running {
        return;
    }
    let Ok(out) = std::process::Command::new("qs").arg("ipc").arg("show").output() else { return };
    if !out.status.success() {
        return;
    }
    state.ipc_targets = parse_ipc_targets(&String::from_utf8_lossy(&out.stdout));
    // Screen migration needs a handler for it; toggle-only shells stay
    // NotConfigured (warn + continue, never a requirement).
    state.screen_migration = if state
        .ipc_targets
        .iter()
        .any(|t| t.contains("screen") || t.contains("monitor") || t.contains("output"))
    {
        Capability::Available
    } else if state.ipc_targets.is_empty() {
        Capability::Unsupported
    } else {
        Capability::NotConfigured
    };
}

/// Detect the wallpaper provider (binary + daemon presence + query).
pub fn detect_wallpaper() -> WallpaperState {
    use rosadeck_session_core::WallpaperProviderKind as Kind;
    if in_path("awww") && !pids_named("awww-daemon").is_empty() {
        let current = std::process::Command::new("awww")
            .arg("query")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| parse_awww_query(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or_default();
        return WallpaperState { provider: Kind::Awww, reapply: Capability::Available, current };
    }
    if in_path("hyprpaper") {
        return WallpaperState { provider: Kind::Hyprpaper, reapply: Capability::NotConfigured, current: vec![] };
    }
    WallpaperState { provider: Kind::None, reapply: Capability::Unsupported, current: vec![] }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IPC_SHOW: &str = "target wifi\n  function toggle(): void\ntarget randomwallpaper\n  function apply(path: string): void\n";
    const AWWW_QUERY: &str = ": eDP-1: 1280x720, scale: 1.5, currently displaying: image: /home/rosa/wallpapers/zelda.png\n";

    #[test]
    fn parses_ipc_targets() {
        assert_eq!(parse_ipc_targets(IPC_SHOW), vec!["wifi", "randomwallpaper"]);
        assert!(parse_ipc_targets("").is_empty());
    }

    #[test]
    fn parses_awww_query() {
        assert_eq!(
            parse_awww_query(AWWW_QUERY),
            vec![("eDP-1".to_owned(), "/home/rosa/wallpapers/zelda.png".to_owned())]
        );
    }

    #[test]
    fn detects_live_environment() {
        // Observed on this machine: quickshell + awww-daemon running.
        let qs = detect_quickshell();
        assert!(qs.running, "quickshell expected running on this machine");
        assert!(qs.cli_available);
        let wp = detect_wallpaper();
        assert_eq!(wp.provider, rosadeck_session_core::WallpaperProviderKind::Awww);
        assert_eq!(wp.reapply, Capability::Available);
        assert!(wp.current.iter().any(|(o, _)| o == "eDP-1"));
    }

    #[test]
    fn ipc_probe_marks_toggle_shell_not_configured() {
        let mut qs = detect_quickshell();
        probe_ipc_targets(&mut qs);
        if qs.ipc_targets.is_empty() {
            assert_eq!(qs.screen_migration, Capability::Unsupported);
        } else {
            // Reference shell: toggles only → migration not configured.
            assert_eq!(qs.screen_migration, Capability::NotConfigured);
        }
    }
}
