//! F0 prototype 2: strictly read-only walk of /sys/class/drm.
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
struct Connector {
    drm_name: String, // card1-HDMI-A-1
    name: String,     // HDMI-A-1 (sin prefijo cardN-)
    status: String,
    enabled: String,
    modes: Vec<String>,
    edid_bytes: usize,
}

fn short_name(drm: &str) -> &str {
    drm.split_once('-').map(|(_, r)| r).unwrap_or(drm)
}

fn is_connector(drm: &str) -> bool {
    let s = short_name(drm);
    s.starts_with("eDP-") || s.starts_with("HDMI-") || s.starts_with("DP-")
}

fn read_trim(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_default().trim().to_string()
}

fn probe_one(dir: &Path) -> Option<Connector> {
    let drm_name = dir.file_name()?.to_string_lossy().into_owned();
    if !is_connector(&drm_name) {
        return None;
    }
    let modes = read_trim(&dir.join("modes"));
    let edid = fs::read(dir.join("edid")).unwrap_or_default();
    Some(Connector {
        name: short_name(&drm_name).into(),
        drm_name,
        status: read_trim(&dir.join("status")),
        enabled: read_trim(&dir.join("enabled")),
        modes: modes.lines().map(|l| l.trim().into()).filter(|l: &String| !l.is_empty()).collect(),
        edid_bytes: edid.len(),
    })
}

fn probe_sysfs(base: &Path) -> Vec<Connector> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(base) else { return out };
    let mut names: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    names.sort();
    for d in names {
        if let Some(c) = probe_one(&d) {
            out.push(c);
        }
    }
    out
}

fn main() {
    for c in probe_sysfs(Path::new("/sys/class/drm")) {
        println!("Connector: {} ({})", c.name, c.drm_name);
        println!("  Status : {}", c.status);
        println!("  Enabled: {}", c.enabled);
        println!("  Modes  : {} {}", c.modes.len(), if c.modes.is_empty() { "".into() } else { format!("({})", c.modes.join(", ")) });
        println!("  EDID   : {}", if c.edid_bytes == 0 { "absent".into() } else { format!("{} bytes", c.edid_bytes) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn classifies_only_display_connectors() {
        assert!(is_connector("card1-eDP-1"));
        assert!(is_connector("card1-HDMI-A-1"));
        assert!(is_connector("card0-DP-2"));
        assert!(!is_connector("card1"));
        assert!(!is_connector("renderD128"));
        assert!(!is_connector("version"));
    }

    #[test]
    fn strips_card_prefix() {
        assert_eq!(short_name("card1-HDMI-A-1"), "HDMI-A-1");
        assert_eq!(short_name("card1-eDP-1"), "eDP-1");
    }

    #[test]
    fn parses_modes_lines() {
        let dir = std::env::temp_dir().join("drm-probe-test");
        let c = dir.join("card9-HDMI-A-9");
        fs::create_dir_all(&c).unwrap();
        fs::write(c.join("status"), "connected\n").unwrap();
        fs::write(c.join("enabled"), "enabled\n").unwrap();
        fs::write(c.join("modes"), "1920x1080\n3840x2160\n").unwrap();
        fs::write(c.join("edid"), vec![0u8; 128]).unwrap();
        let got = probe_sysfs(&dir).pop().unwrap();
        assert_eq!(got.modes, vec!["1920x1080", "3840x2160"]);
        assert_eq!(got.edid_bytes, 128);
        fs::remove_dir_all(&dir).ok();
    }
}
