//! F0 prototype 5: read-only reconciler (Hyprland + DRM + EDID). Never writes.
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

#[derive(Debug, Deserialize, Clone)]
#[allow(non_snake_case)] // nombres exactos del IPC Hyprland
struct Monitor {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    width: i32,
    #[serde(default)]
    height: i32,
    #[serde(default)]
    refreshRate: f64,
    #[serde(default)]
    disabled: bool,
    #[serde(default)]
    mirrorOf: String,
    #[serde(rename = "availableModes", default)]
    available_modes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct Mode {
    width: u32,
    height: u32,
    hz: f64, // 0.0 = sin dato; jamás redondear: tolerancia en F1
}

fn parse_mode(s: &str) -> Option<Mode> {
    let s = s.trim().trim_end_matches("Hz").trim().to_string();
    let (wh, hz) = match s.split_once('@') {
        Some((w, h)) => (w, h.parse::<f64>().unwrap_or(0.0)),
        None => (s.as_str(), 0.0),
    };
    let (w, h) = wh.split_once('x')?;
    Some(Mode { width: w.trim().parse().ok()?, height: h.trim().parse().ok()?, hz })
}

fn fnv8(data: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)[..8].to_owned()
}

fn fetch_monitors() -> Vec<Monitor> {
    let path = format!(
        "{}/hypr/{}/.socket.sock",
        std::env::var("XDG_RUNTIME_DIR").unwrap_or_default(),
        std::env::var("HYPRLAND_INSTANCE_SIGNATURE").unwrap_or_default()
    );
    let mut s = UnixStream::connect(&path).expect("connect .socket.sock");
    s.write_all(b"j/monitors all").unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    serde_json::from_slice(&buf).unwrap_or_default()
}

fn main() {
    let mons = fetch_monitors();
    let mut by_name: BTreeMap<String, Monitor> = mons.into_iter().map(|m| (m.name.clone(), m)).collect();
    // DRM: drm_name -> (status, modes, edid_len, edid_hash?)
    let mut drm: BTreeMap<String, (String, Vec<String>, usize, Option<String>)> = BTreeMap::new();
    if let Ok(rd) = fs::read_dir(Path::new("/sys/class/drm")) {
        let mut paths: Vec<_> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for d in paths {
            let drm_name = d.file_name().unwrap().to_string_lossy().into_owned();
            let short = drm_name.split_once('-').map(|(_, r)| r.to_owned()).unwrap_or_default();
            if !(short.starts_with("eDP-") || short.starts_with("HDMI-") || short.starts_with("DP-")) {
                continue;
            }
            let status = fs::read_to_string(d.join("status")).unwrap_or_default().trim().to_owned();
            let modes: Vec<String> = fs::read_to_string(d.join("modes"))
                .unwrap_or_default()
                .lines()
                .map(|l| l.trim().to_owned())
                .filter(|l| !l.is_empty())
                .collect();
            let edid = fs::read(d.join("edid")).unwrap_or_default();
            let hash = if edid.len() >= 128 { Some(fnv8(&edid[..128])) } else { None };
            drm.insert(short, (status, modes, edid.len(), hash));
        }
    }

    let mut names: Vec<String> = by_name.keys().cloned().collect();
    for k in drm.keys() {
        if !names.contains(k) {
            names.push(k.clone());
        }
    }
    names.sort();

    for n in names {
        let hy = by_name.remove(&n);
        let d = drm.remove(&n);
        println!("Display: {n}");
        match (&hy, &d) {
            (Some(m), Some((st, modes, edid_len, hash))) => {
                println!("  Hyprland : {}x{} @{:.3} disabled={} mirror={} modes={}", m.width, m.height, m.refreshRate, m.disabled, m.mirrorOf, m.available_modes.join(","));
                println!("  DRM      : status={} modes={} edid={}b hash={}", st, modes.join(","), edid_len, hash.as_deref().unwrap_or("-"));
                let hm: Vec<Mode> = m.available_modes.iter().filter_map(|s| parse_mode(s)).collect();
                let dm: Vec<Mode> = modes.iter().filter_map(|s| parse_mode(s)).collect();
                // DRM `modes` no trae Hz: comparar solo WxH contra Hyprland
                let drm_wh: Vec<(u32, u32)> = dm.iter().map(|x| (x.width, x.height)).collect();
                for h in &hm {
                    if !drm_wh.contains(&(h.width, h.height)) {
                        println!("  WARNING: Hyprland mode {}x{} not in DRM modes", h.width, h.height);
                    }
                }
                if *edid_len == 0 {
                    println!("  WARNING: DRM EDID absent but Hyprland knows this output");
                }
            }
            (Some(m), None) => println!("  Hyprland only: {} (no DRM connector match) WARNING", m.description),
            (None, Some((st, modes, edid_len, _))) => {
                println!("  DRM only: status={} modes={} edid={}b", st, modes.join(","), edid_len);
                println!("  NOTE: output invisible to compositor (disconnected/disabled) — expected for HDMI unplugged");
            }
            (None, None) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_all_mode_forms_without_rounding() {
        for (s, w, h, hz) in [
            ("1920x1080@60", 1920, 1080, 60.0),
            ("1920x1080@59.94", 1920, 1080, 59.94),
            ("1920x1080@60.00Hz", 1920, 1080, 60.0),
            ("3840x2160@60", 3840, 2160, 60.0),
            ("3840x2160@119.88", 3840, 2160, 119.88),
            ("1920x1080@59.997", 1920, 1080, 59.997),
        ] {
            let m = parse_mode(s).unwrap();
            assert_eq!((m.width, m.height), (w, h), "{s}");
            assert!((m.hz - hz).abs() < 1e-9, "{s}: got {}", m.hz);
        }
    }
    #[test]
    fn distinct_hz_stay_distinct() {
        assert_ne!(parse_mode("1920x1080@59.94").unwrap().hz, parse_mode("1920x1080@60").unwrap().hz);
    }
    #[test]
    fn bare_drm_mode_has_no_hz() {
        let m = parse_mode("1920x1080").unwrap();
        assert_eq!(m.hz, 0.0);
    }
}
