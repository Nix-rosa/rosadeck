//! Curated CTA VIC → [`Mode`](display_core::Mode) table.
//!
//! Assumption (documented, not observed on this machine): Hz values are CTA
//! *nominal* timings. Panels may run e.g. 119.88 where the table says 120.0 —
//! that is why CTA-sourced modes carry [`RefreshKind::Estimated`](display_core::RefreshKind)
//! and are reconciled against observed Hz with ±0.5 tolerance.
//! Interlaced VICs are tabled but never converted (no interlace model in F2).

use display_core::Mode;

/// One tabled VIC entry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VicInfo {
    /// CTA VIC number (1–127).
    pub vic: u8,
    /// Horizontal pixels.
    pub width: u32,
    /// Vertical pixels.
    pub height: u32,
    /// Nominal refresh in Hz (full precision, never pre-rounded).
    pub hz: f64,
    /// Interlaced timings cannot become a [`Mode`] yet.
    pub interlaced: bool,
}

/// Curated subset: only timings the project has verified against CTA-861.
/// Unknown VICs return `None` (preserved in raw, never failed).
pub fn vic_info(vic: u8) -> Option<VicInfo> {
    let (w, h, hz, i) = match vic {
        1 => (640, 480, 60.0, false),
        3 => (720, 480, 60.0, false),
        4 => (1280, 720, 60.0, false),
        5 => (1920, 1080, 60.0, true),
        16 => (1920, 1080, 60.0, false),
        19 => (1280, 720, 50.0, false),
        20 => (1920, 1080, 50.0, true),
        31 => (1920, 1080, 50.0, false),
        33 => (1920, 1080, 25.0, false),
        34 => (1920, 1080, 30.0, false),
        63 => (1920, 1080, 120.0, false),
        95 => (3840, 2160, 30.0, false),
        96 => (3840, 2160, 50.0, false),
        97 => (3840, 2160, 60.0, false),
        117 => (3840, 2160, 100.0, false),
        118 => (3840, 2160, 120.0, false),
        _ => return None,
    };
    Some(VicInfo { vic, width: w, height: h, hz, interlaced: i })
}

/// VIC → [`Mode`]. `None` for unknown or interlaced VICs (both preserved
/// upstream, never an error at this layer).
pub fn vic_to_mode(vic: u8) -> Option<Mode> {
    let info = vic_info(vic)?;
    if info.interlaced {
        return None;
    }
    Mode::new(info.width, info.height, info.hz)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_supported_vics_exactly() {
        assert_eq!(vic_to_mode(16).unwrap(), Mode { width: 1920, height: 1080, hz: 60.0 });
        assert_eq!(vic_to_mode(118).unwrap(), Mode { width: 3840, height: 2160, hz: 120.0 });
        assert_eq!(vic_to_mode(97).unwrap(), Mode { width: 3840, height: 2160, hz: 60.0 });
    }

    #[test]
    fn skips_interlaced_and_unknown() {
        assert!(vic_to_mode(5).is_none()); // 1080i60: tabled but not convertible
        assert!(vic_info(5).unwrap().interlaced);
        assert!(vic_to_mode(200).is_none());
        assert!(vic_to_mode(0).is_none());
    }
}
