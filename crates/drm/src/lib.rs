//! `rosadeck-drm`: strictly read-only walk of `/sys/class/drm`.
//!
//! Promoted from the F0 `drm-probe` prototype. Only reads `status`, `enabled`,
//! `modes` and `edid`. `modes` lines carry no Hz, so [`Mode`](display_core::Mode)
//! values from here always have unknown refresh.

use display_core::Mode;
use std::fs;
use std::path::Path;

/// Kernel connector state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorStatus {
    /// Cable/panel present.
    Connected,
    /// Nothing attached.
    Disconnected,
    /// Any other sysfs value (preserved, not normalized away).
    Unknown,
}

/// One DRM connector snapshot (pure data).
#[derive(Debug, Clone)]
pub struct ConnectorState {
    /// Full sysfs name, e.g. `card1-HDMI-A-1`.
    pub drm_name: String,
    /// Short name, e.g. `HDMI-A-1` (hint only, never identity).
    pub name: String,
    /// Attachment state.
    pub status: ConnectorStatus,
    /// Whether the kernel reports the connector enabled.
    pub enabled: bool,
    /// Kernel-filtered modes (refresh always unknown here).
    pub modes: Vec<Mode>,
    /// Raw EDID bytes (empty when absent).
    pub edid: Vec<u8>,
}

/// True for display connectors (`eDP-*`, `HDMI-*`, `DP-*`).
pub fn is_display_connector(drm_name: &str) -> bool {
    let short = drm_name.split_once('-').map(|(_, r)| r).unwrap_or(drm_name);
    short.starts_with("eDP-") || short.starts_with("HDMI-") || short.starts_with("DP-")
}

fn read_trim(dir: &Path, file: &str) -> String {
    fs::read_to_string(dir.join(file)).unwrap_or_default().trim().to_owned()
}

/// Walk `base` (normally `/sys/class/drm`), sorted, read-only.
pub fn walk(base: &Path) -> Vec<ConnectorState> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(base) else { return out };
    let mut paths: Vec<_> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for dir in paths {
        let drm_name = dir.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if !is_display_connector(&drm_name) {
            continue;
        }
        let name = drm_name.split_once('-').map(|(_, r)| r.to_owned()).unwrap_or_default();
        let status = match read_trim(&dir, "status").as_str() {
            "connected" => ConnectorStatus::Connected,
            "disconnected" => ConnectorStatus::Disconnected,
            _ => ConnectorStatus::Unknown,
        };
        let modes = read_trim(&dir, "modes")
            .lines()
            .filter_map(|l| display_core::parse_mode(l).ok())
            .collect();
        out.push(ConnectorState {
            drm_name,
            name,
            status,
            enabled: read_trim(&dir, "enabled").starts_with("enabled"),
            modes,
            edid: fs::read(dir.join("edid")).unwrap_or_default(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fake_sysfs() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("rosadeck-drm-test");
        std::fs::remove_dir_all(&dir).ok();
        let c = dir.join("card9-HDMI-A-9");
        fs::create_dir_all(&c).unwrap();
        fs::write(c.join("status"), "connected\n").unwrap();
        fs::write(c.join("enabled"), "enabled\n").unwrap();
        fs::write(c.join("modes"), "1920x1080\n3840x2160\n").unwrap();
        fs::write(c.join("edid"), vec![0u8; 128]).unwrap();
        fs::create_dir_all(dir.join("renderD128")).unwrap(); // must be ignored
        dir
    }

    #[test]
    fn walks_only_display_connectors() {
        let dir = fake_sysfs();
        let cs = walk(&dir);
        assert_eq!(cs.len(), 1);
        let c = &cs[0];
        assert_eq!((c.drm_name.as_str(), c.name.as_str()), ("card9-HDMI-A-9", "HDMI-A-9"));
        assert_eq!(c.status, ConnectorStatus::Connected);
        assert!(c.enabled);
        assert_eq!(c.modes.len(), 2);
        assert!(c.modes.iter().all(|m| m.is_unknown_refresh()));
        assert_eq!(c.edid.len(), 128);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn classifies_connector_names() {
        assert!(is_display_connector("card1-eDP-1"));
        assert!(is_display_connector("card0-DP-2"));
        assert!(!is_display_connector("card1"));
        assert!(!is_display_connector("renderD128"));
    }
}
