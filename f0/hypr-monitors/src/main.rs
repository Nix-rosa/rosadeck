//! F0 prototype 1: read-only Hyprland monitors via .socket.sock (no `keyword`, no writes).
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

#[derive(Debug, Deserialize, Serialize, Clone)]
#[allow(non_snake_case)] // nombres exactos del IPC Hyprland
struct Monitor {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    make: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    serial: String,
    #[serde(default)]
    width: i32,
    #[serde(default)]
    height: i32,
    #[serde(default)]
    refreshRate: f64,
    #[serde(default)]
    x: i32,
    #[serde(default)]
    y: i32,
    #[serde(default)]
    scale: f64,
    #[serde(default)]
    disabled: bool,
    #[serde(default)]
    mirrorOf: String,
    #[serde(rename = "availableModes", default)]
    available_modes: Vec<String>,
}

fn socket_path() -> Result<String, String> {
    let runtime = std::env::var("XDG_RUNTIME_DIR").map_err(|_| "XDG_RUNTIME_DIR missing".to_string())?;
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").map_err(|_| "HIS missing".to_string())?;
    Ok(format!("{runtime}/hypr/{sig}/.socket.sock"))
}

fn fetch_monitors() -> Result<Vec<Monitor>, String> {
    let path = socket_path()?;
    let mut s = UnixStream::connect(&path).map_err(|e| format!("connect {path}: {e}"))?;
    s.write_all(b"j/monitors all").map_err(|e| format!("write: {e}"))?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).map_err(|e| format!("read: {e}"))?;
    serde_json::from_slice(&buf).map_err(|e| format!("json: {e}"))
}

fn main() {
    let as_json = std::env::args().any(|a| a == "--json");
    match fetch_monitors() {
        Ok(mons) => {
            if as_json {
                println!("{}", serde_json::to_string_pretty(&mons).unwrap());
            } else {
                for m in &mons {
                    println!("name         : {}", m.name);
                    println!("  description: {}", m.description);
                    println!("  make       : {}", m.make);
                    println!("  model      : {}", m.model);
                    println!("  serial     : {}", if m.serial.is_empty() { "(empty)" } else { &m.serial });
                    println!("  geometry   : {}x{} @ {:.3} Hz at {},{}", m.width, m.height, m.refreshRate, m.x, m.y);
                    println!("  scale      : {}", m.scale);
                    println!("  disabled   : {}", m.disabled);
                    println!("  mirrorOf   : {}", m.mirrorOf);
                    println!("  modes      : {}", if m.available_modes.is_empty() { "(none)".into() } else { m.available_modes.join(", ") });
                }
            }
        }
        Err(e) => {
            eprintln!("ERROR: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const FIXTURE: &str = include_str!("../../hypr-monitors/tests/fixtures/monitors.json");

    #[test]
    fn parses_all_required_fields() {
        let mons: Vec<Monitor> = serde_json::from_str(FIXTURE).unwrap();
        assert!(!mons.is_empty());
        let m = &mons[0];
        assert_eq!(m.name, "eDP-1");
        assert_eq!((m.width, m.height), (1920, 1080));
        assert!((m.refreshRate - 59.997).abs() < 0.01);
        assert!(!m.available_modes.is_empty());
    }

    #[test]
    fn tolerates_missing_optionals() {
        let m: Monitor = serde_json::from_str(r#"{"name":"HDMI-A-1"}"#).unwrap();
        assert_eq!(m.name, "HDMI-A-1");
        assert!(!m.disabled);
        assert!(m.available_modes.is_empty());
    }

    #[test]
    fn json_roundtrip_preserves_hz_precision() {
        let mons: Vec<Monitor> = serde_json::from_str(FIXTURE).unwrap();
        let v = serde_json::to_value(&mons[0]).unwrap();
        let hz = v["refreshRate"].as_f64().unwrap();
        assert!((hz - 59.997).abs() < 1e-6, "lost precision: {hz}");
    }
}
