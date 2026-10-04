//! Layout primitives: outputs, geometry, snapshots.
//!
//! Pure data for F2's `snapshot -> apply -> verify -> restore` loop.
//! Capturing and applying live state belongs to `backend-hyprland`, not here.

use crate::mode::Mode;
use serde::{Deserialize, Serialize};

/// Stable compositor-side output name, e.g. `eDP-1`.
/// Identity hint only: never a persistent display identity (see [`crate::DisplayIdentity`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OutputId(pub String);

/// Kernel DRM connector, e.g. `HDMI-A-1` (without the `cardN-` prefix).
/// Stored as `connector_hint` only; connectors move between GPUs/ports.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConnectorId(pub String);

/// Position in the virtual layout, pixels from top-left (`0x0`, `1920x0`,
/// `0x-1080`; Hyprland uses inverse-Y: negative y is *above*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    /// Horizontal offset in pixels (may be negative).
    pub x: i32,
    /// Vertical offset in pixels (negative = above).
    pub y: i32,
}

/// Output scale factor (e.g. `1.0`, `1.5`). Must be finite and positive.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Scale(pub f64);

impl Scale {
    /// Validated constructor: finite and `> 0.0`.
    pub fn new(v: f64) -> Option<Self> {
        if v.is_finite() && v > 0.0 {
            Some(Self(v))
        } else {
            None
        }
    }
}

/// Mirroring relationship of one output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mirror {
    /// Independent output.
    Disabled,
    /// Mirror of the named output.
    MirrorOf(OutputId),
}

/// Complete per-output state (one entry of a snapshot).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputState {
    /// Compositor output name.
    pub id: OutputId,
    /// Kernel connector hint, when known.
    pub connector: Option<ConnectorId>,
    /// Active mode (`None` = disabled or unknown).
    pub mode: Option<Mode>,
    /// Layout position.
    pub position: Position,
    /// Scale factor.
    pub scale: Scale,
    /// Whether the compositor excludes it from the layout.
    pub disabled: bool,
    /// Mirror relationship.
    pub mirror: Mirror,
}

/// Point-in-time capture of all outputs (serializable for restore).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplaySnapshot {
    /// Unix timestamp (seconds) of capture.
    pub taken_at_unix: u64,
    /// One entry per known output.
    pub outputs: Vec<OutputState>,
}

/// Working layout: the set of output states a plan would apply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayLayout {
    /// Desired per-output states.
    pub outputs: Vec<OutputState>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_rejects_non_positive_and_non_finite() {
        assert!(Scale::new(1.5).is_some());
        assert!(Scale::new(0.0).is_none());
        assert!(Scale::new(-1.0).is_none());
        assert!(Scale::new(f64::NAN).is_none());
        assert!(Scale::new(f64::INFINITY).is_none());
    }

    #[test]
    fn snapshot_roundtrip_preserves_hz() {
        let snap = DisplaySnapshot {
            taken_at_unix: 1_700_000_000,
            outputs: vec![OutputState {
                id: OutputId("eDP-1".into()),
                connector: Some(ConnectorId("eDP-1".into())),
                mode: Some(Mode { width: 1920, height: 1080, hz: 59.997 }),
                position: Position { x: 0, y: 0 },
                scale: Scale(1.5),
                disabled: false,
                mirror: Mirror::Disabled,
            }],
        };
        let json = serde_json::to_string(&snap).unwrap();
        let back: DisplaySnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(snap, back);
    }
}
