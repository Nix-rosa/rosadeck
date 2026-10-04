//! Tolerant Hyprland monitor model → [`display_core`] normalization.
//!
//! Principle kept from F0: own structs with `#[serde(default)]` because the
//! `hyprland` crate lags the compositor (F0: 0.3.13 vs Hyprland 0.56 fields
//! like `solitary`, `sdr*`). Unknown fields are ignored by serde.

use display_core::{ConnectorId, Mirror, Mode, OutputId, OutputState, Position, Scale};
use serde::{Deserialize, Serialize};

/// Active workspace pointer inside a monitor entry.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ActiveWs {
    /// Workspace id.
    #[serde(default)]
    pub id: i32,
    /// Workspace name.
    #[serde(default)]
    pub name: String,
}

/// One entry of `hyprctl monitors all -j` (tolerant subset).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[allow(non_snake_case)]
pub struct HyprMonitor {
    /// Compositor output id.
    #[serde(default)]
    pub id: i32,
    /// Compositor output name (`eDP-1`).
    #[serde(default)]
    pub name: String,
    /// Human description (`Samsung Display Corp. 0x4161`).
    #[serde(default)]
    pub description: String,
    /// Manufacturer string.
    #[serde(default)]
    pub make: String,
    /// Model string.
    #[serde(default)]
    pub model: String,
    /// Serial string (often empty even when EDID has data — never identity).
    #[serde(default)]
    pub serial: String,
    /// Current width.
    #[serde(default)]
    pub width: i32,
    /// Current height.
    #[serde(default)]
    pub height: i32,
    /// Current refresh (observed `59.997` on eDP-1).
    #[serde(default)]
    pub refreshRate: f64,
    /// Layout position.
    #[serde(default)]
    pub x: i32,
    /// Layout position.
    #[serde(default)]
    pub y: i32,
    /// Scale factor.
    #[serde(default)]
    pub scale: f64,
    /// Rotation transform 0–7.
    #[serde(default)]
    pub transform: i32,
    /// VRR enabled.
    #[serde(default)]
    pub vrr: bool,
    /// Active workspace pointer.
    #[serde(default)]
    pub activeWorkspace: ActiveWs,
    /// Special workspace pointer (id 0 = none).
    #[serde(default)]
    pub specialWorkspace: ActiveWs,
    /// Excluded from layout.
    #[serde(default)]
    pub disabled: bool,
    /// Focused at query time.
    #[serde(default)]
    pub focused: bool,
    /// Mirror source name, or `"none"`.
    #[serde(default)]
    pub mirrorOf: String,
    /// Modes the compositor will accept.
    #[serde(rename = "availableModes", default)]
    pub available_modes: Vec<String>,
}

/// Compositor-side metadata kept alongside the normalized state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HyprMonitorInfo {
    /// Human description.
    pub description: String,
    /// Manufacturer string.
    pub make: String,
    /// Model string.
    pub model: String,
    /// Serial string (may be empty).
    pub serial: String,
    /// Raw `availableModes` strings.
    pub available_modes: Vec<String>,
}

/// Normalize one monitor: current mode + metadata, Hz preserved exactly.
pub fn normalize(m: &HyprMonitor) -> (OutputState, HyprMonitorInfo) {
    let mode = if m.disabled {
        None
    } else {
        Mode::new(m.width.max(0) as u32, m.height.max(0) as u32, m.refreshRate.max(0.0))
    };
    let mirror =
        if m.mirrorOf.is_empty() || m.mirrorOf == "none" { Mirror::Disabled } else { Mirror::MirrorOf(OutputId(m.mirrorOf.clone())) };
    let state = OutputState {
        id: OutputId(m.name.clone()),
        connector: Some(ConnectorId(m.name.clone())),
        mode,
        position: Position { x: m.x, y: m.y },
        scale: Scale::new(m.scale).unwrap_or(Scale(1.0)),
        disabled: m.disabled,
        mirror,
    };
    let info = HyprMonitorInfo {
        description: m.description.clone(),
        make: m.make.clone(),
        model: m.model.clone(),
        serial: m.serial.clone(),
        available_modes: m.available_modes.clone(),
    };
    (state, info)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/monitors.json");

    #[test]
    fn normalizes_real_fixture() {
        let mons: Vec<HyprMonitor> = serde_json::from_str(FIXTURE).unwrap();
        let (st, info) = normalize(&mons[0]);
        assert_eq!(st.id.0, "eDP-1");
        let m = st.mode.unwrap();
        assert_eq!((m.width, m.height), (1920, 1080));
        assert!((m.hz - 59.997).abs() < 1e-9);
        assert_eq!(info.make, "Samsung Display Corp.");
        assert_eq!(info.available_modes, vec!["1920x1080@60.00Hz"]);
        assert!(matches!(st.mirror, Mirror::Disabled));
    }

    #[test]
    fn tolerates_absent_fields_and_mirror() {
        let m: HyprMonitor = serde_json::from_str(r#"{"name":"HDMI-A-1","mirrorOf":"eDP-1"}"#).unwrap();
        let (st, _) = normalize(&m);
        assert!(st.mode.is_none()); // 0x0 with 0 Hz is not a mode
        assert!(matches!(st.mirror, Mirror::MirrorOf(_)));
        assert_eq!(st.scale.0, 1.0); // invalid 0.0 scale falls back, never NaN
    }

    #[test]
    fn disabled_output_has_no_mode() {
        let m: HyprMonitor =
            serde_json::from_str(r#"{"name":"DP-1","width":1920,"height":1080,"refreshRate":60.0,"disabled":true}"#).unwrap();
        assert!(normalize(&m).0.mode.is_none());
    }
}
