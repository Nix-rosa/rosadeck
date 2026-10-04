//! [`Mode`] representation, parsing and intersection.
//!
//! Precision rule (observed in F0): refresh rates such as `59.94`, `59.997`
//! or `119.88` are stored as full `f64` and never silently rounded.
//! `PartialEq` is therefore *exact*. Approximate matching for mode
//! intersection uses an explicit tolerance ([`HZ_TOLERANCE`]).
//!
//! Unknown refresh (e.g. DRM `modes` lines like `1920x1080` carry no Hz)
//! is represented as [`HZ_UNKNOWN]` (`0.0`). It means "no data", never
//! "assume 60 Hz".

use serde::{Deserialize, Serialize};
use std::fmt;

/// Tolerance in Hz for approximate mode matching. Explicit, never implicit.
pub const HZ_TOLERANCE: f64 = 0.5;

/// Sentinel for "refresh unknown" (DRM `modes` has no Hz column).
pub const HZ_UNKNOWN: f64 = 0.0;

/// A display mode: resolution plus exact refresh rate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Mode {
    /// Horizontal pixels, e.g. 1920.
    pub width: u32,
    /// Vertical pixels, e.g. 1080.
    pub height: u32,
    /// Exact refresh in Hz (`59.94`, `59.997`, `119.88` preserved as-is).
    /// `0.0` ([`HZ_UNKNOWN`]) means "unknown", not 60 Hz.
    pub hz: f64,
}

impl Mode {
    /// Build a mode; rejects zero dimensions and non-finite/negative Hz.
    pub fn new(width: u32, height: u32, hz: f64) -> Option<Self> {
        if width == 0 || height == 0 || !hz.is_finite() || hz < 0.0 {
            return None;
        }
        Some(Self { width, height, hz })
    }

    /// True when the refresh rate is unknown ([`HZ_UNKNOWN`]).
    pub fn is_unknown_refresh(self) -> bool {
        self.hz == HZ_UNKNOWN
    }

    /// Pixel area, used to order by "resolution".
    pub fn area(self) -> u64 {
        self.width as u64 * self.height as u64
    }

    /// Approximate match: same WxH and (either side unknown, or Hz within `tol`).
    pub fn matches_within(self, other: Self, tol: f64) -> bool {
        if self.width != other.width || self.height != other.height {
            return false;
        }
        if self.is_unknown_refresh() || other.is_unknown_refresh() {
            return true;
        }
        (self.hz - other.hz).abs() <= tol
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_unknown_refresh() {
            write!(f, "{}x{}@?", self.width, self.height)
        } else {
            write!(f, "{}x{}@{:.3}", self.width, self.height, self.hz)
        }
    }
}

/// Where a mode observation came from. These meanings differ and must not
/// be conflated: EDID/CTA describe panel *capability*, DRM describes what the
/// kernel exposes, Hyprland describes what the compositor will actually use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModeSource {
    /// Compositor's `availableModes` (what can actually be applied).
    Hyprland,
    /// Kernel DRM `modes` (no Hz column; always unknown refresh).
    Drm,
    /// EDID base-block Detailed Timing Descriptor (measured timing).
    EdidDtd,
    /// CTA-861 VIC entry (nominal table value — see [`RefreshKind::Estimated`]).
    CtaVic,
}

/// A mode plus the sources that declared it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourcedMode {
    /// Canonical mode (see [`merge_modes`]).
    pub mode: Mode,
    /// Every source that declared it, first-seen order, no duplicates.
    pub sources: Vec<ModeSource>,
}

/// How trustworthy a sourced refresh rate is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshKind {
    /// Measured or compositor-reported (`Hyprland`, `EdidDtd`).
    Exact,
    /// Nominal table value (`CtaVic` only): the panel may run e.g. 119.88
    /// where the table says 120. Reconcile against observed Hz (±0.5).
    Estimated,
    /// No data (`hz == 0.0`, typically `Drm`).
    Unknown,
}

impl SourcedMode {
    /// Refresh-rate trust level for this mode.
    pub fn refresh_kind(&self) -> RefreshKind {
        if self.mode.is_unknown_refresh() {
            return RefreshKind::Unknown;
        }
        if self.sources.iter().all(|s| *s == ModeSource::CtaVic) {
            RefreshKind::Estimated
        } else {
            RefreshKind::Exact
        }
    }
}

/// Merge `(mode, source)` observations into deduplicated [`SourcedMode`]s.
///
/// Same WxH with Hz within [`HZ_TOLERANCE`] (or unknown on either side) merge
/// into one entry; the canonical Hz is the first known value seen; sources are
/// unioned in first-seen order. Output is deterministic (area desc, Hz desc).
pub fn merge_modes(observations: &[(Mode, ModeSource)]) -> Vec<SourcedMode> {
    let mut out: Vec<SourcedMode> = Vec::new();
    for (m, src) in observations {
        if let Some(e) = out.iter_mut().find(|e| e.mode.matches_within(*m, HZ_TOLERANCE)) {
            if e.mode.is_unknown_refresh() && !m.is_unknown_refresh() {
                e.mode.hz = m.hz;
            }
            if !e.sources.contains(src) {
                e.sources.push(*src);
            }
        } else {
            out.push(SourcedMode { mode: *m, sources: vec![*src] });
        }
    }
    out.sort_by(|a, b| {
        b.mode.area().cmp(&a.mode.area()).then(b.mode.hz.total_cmp(&a.mode.hz))
    });
    out
}
/// Parse failure for [`parse_mode`].
#[derive(Debug, Clone, PartialEq)]
pub struct ModeParseError(pub String);

impl fmt::Display for ModeParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid mode: {}", self.0)
    }
}

impl std::error::Error for ModeParseError {}

/// Parse `1920x1080@60`, `1920x1080@60.00Hz`, `1920x1080@59.94`,
/// `3840x2160@119.88` or bare `1920x1080` (DRM form, Hz unknown).
/// Hz keeps full `f64` precision; nothing is rounded.
pub fn parse_mode(s: &str) -> Result<Mode, ModeParseError> {
    let err = || ModeParseError(s.to_owned());
    let t = s.trim();
    let t = t.strip_suffix("Hz").or_else(|| t.strip_suffix("hz")).unwrap_or(t).trim();
    let (wh, hz) = match t.split_once('@') {
        Some((w, h)) => {
            if h.contains('@') {
                return Err(err());
            }
            (w, h.parse::<f64>().map_err(|_| err())?)
        }
        None => (t, HZ_UNKNOWN),
    };
    if !hz.is_finite() || hz < 0.0 {
        return Err(err());
    }
    let (w, h) = wh.split_once('x').ok_or_else(err)?;
    let width: u32 = w.trim().parse().map_err(|_| err())?;
    let height: u32 = h.trim().parse().map_err(|_| err())?;
    Mode::new(width, height, hz).ok_or_else(err)
}

/// Common modes of `a` and `b` (same WxH, Hz within [`HZ_TOLERANCE`]).
///
/// The canonical value is the `a`-side mode, except when `a` has unknown Hz
/// and the matching `b` mode knows it: then the known Hz is adopted.
/// Output is deduplicated and deterministic (area desc, Hz desc).
/// Unknown Hz on either side matches any Hz of the same WxH.
pub fn intersect_modes(a: &[Mode], b: &[Mode]) -> Vec<Mode> {
    let mut out: Vec<Mode> = Vec::new();
    for m in a {
        let hit = b.iter().find(|o| m.matches_within(**o, HZ_TOLERANCE));
        if let Some(o) = hit {
            let canon = if m.is_unknown_refresh() && !o.is_unknown_refresh() {
                Mode { hz: o.hz, ..*m }
            } else {
                *m
            };
            if !out.iter().any(|e: &Mode| e.matches_within(canon, HZ_TOLERANCE)) {
                out.push(canon);
            }
        }
    }
    out.sort_by(|x, y| y.area().cmp(&x.area()).then(y.hz.total_cmp(&x.hz)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_required_forms_with_full_precision() {
        for (s, w, h, hz) in [
            ("1920x1080@60", 1920, 1080, 60.0),
            ("1920x1080@60.00", 1920, 1080, 60.0),
            ("1920x1080@60.00Hz", 1920, 1080, 60.0),
            ("1920x1080@59.94", 1920, 1080, 59.94),
            ("1920x1080@59.997", 1920, 1080, 59.997),
            ("1920x1080@59.997Hz", 1920, 1080, 59.997),
            ("3840x2160@60", 3840, 2160, 60.0),
            ("3840x2160@119.88", 3840, 2160, 119.88),
        ] {
            let m = parse_mode(s).unwrap();
            assert_eq!((m.width, m.height), (w, h), "{s}");
            assert!((m.hz - hz).abs() < 1e-9, "{s}: {}", m.hz);
        }
    }

    #[test]
    fn exact_equality_distinguishes_close_rates() {
        assert_ne!(parse_mode("1920x1080@59.94").unwrap(), parse_mode("1920x1080@60").unwrap());
        assert_ne!(parse_mode("1920x1080@59.997").unwrap(), parse_mode("1920x1080@60").unwrap());
        assert_eq!(parse_mode("1920x1080@60").unwrap(), parse_mode("1920x1080@60.00Hz").unwrap());
    }

    #[test]
    fn bare_drm_form_means_unknown_not_sixty() {
        let m = parse_mode("1920x1080").unwrap();
        assert!(m.is_unknown_refresh());
        assert_ne!(m.hz, 60.0);
    }

    #[test]
    fn merges_sources_deterministically() {
        use crate::mode::{merge_modes, ModeSource};
        let a = parse_mode("1920x1080@60").unwrap();
        let b = parse_mode("1920x1080@59.94").unwrap();
        let c = parse_mode("1920x1080").unwrap();
        let r = merge_modes(&[(a, ModeSource::Hyprland), (b, ModeSource::CtaVic), (c, ModeSource::Drm)]);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].mode, a);
        assert_eq!(r[0].sources, vec![ModeSource::Hyprland, ModeSource::CtaVic, ModeSource::Drm]);
        assert_eq!(r[0].refresh_kind(), crate::mode::RefreshKind::Exact);
    }

    #[test]
    fn vic_only_is_estimated_and_unknown_stays_unknown() {
        use crate::mode::{merge_modes, ModeSource, RefreshKind};
        let v = parse_mode("3840x2160@120").unwrap();
        let r = merge_modes(&[(v, ModeSource::CtaVic)]);
        assert_eq!(r[0].refresh_kind(), RefreshKind::Estimated);
        let u = parse_mode("1920x1080").unwrap();
        let r2 = merge_modes(&[(u, ModeSource::Drm)]);
        assert_eq!(r2[0].refresh_kind(), RefreshKind::Unknown);
    }

    #[test]
    fn rejects_garbage() {
        for s in ["", "1080", "1920x", "x1080@60", "1920x1080@", "1920x1080@abc", "0x1080@60", "1920x1080@-1", "1920x1080@60@30"] {
            assert!(parse_mode(s).is_err(), "{s}");
        }
    }

    #[test]
    fn intersect_finds_common_modes_with_tolerance() {
        let a = ["1920x1080@60", "1920x1080@120", "2560x1440@144"].map(|s| parse_mode(s).unwrap());
        let b = ["1920x1080@59.94", "1920x1080@119.88", "1280x720@60"].map(|s| parse_mode(s).unwrap());
        let r = intersect_modes(&a, &b);
        assert_eq!(r.len(), 2);
        // canonical = a-side values, ordered by area then Hz
        assert_eq!(r[0], parse_mode("1920x1080@120").unwrap());
        assert_eq!(r[1], parse_mode("1920x1080@60").unwrap());
    }

    #[test]
    fn intersect_no_common_mode_is_empty() {
        let a = [parse_mode("2560x1440@144").unwrap()];
        let b = [parse_mode("1280x720@60").unwrap()];
        assert!(intersect_modes(&a, &b).is_empty());
    }

    #[test]
    fn intersect_unknown_hz_adopts_known_side() {
        let a = [parse_mode("1920x1080").unwrap()];
        let b = [parse_mode("1920x1080@60").unwrap()];
        let r = intersect_modes(&a, &b);
        assert_eq!(r, vec![parse_mode("1920x1080@60").unwrap()]);
    }

    #[test]
    fn intersect_outside_tolerance_does_not_match() {
        let a = [parse_mode("1920x1080@50").unwrap()];
        let b = [parse_mode("1920x1080@60").unwrap()];
        assert!(intersect_modes(&a, &b).is_empty());
    }

    #[test]
    fn intersect_dedupes_and_orders_deterministically() {
        let a = ["1920x1080@60", "1920x1080@60.0", "1280x720@60"].map(|s| parse_mode(s).unwrap());
        let b = ["1920x1080@59.94", "1280x720@60"].map(|s| parse_mode(s).unwrap());
        let r = intersect_modes(&a, &b);
        assert_eq!(r.len(), 2);
        assert!(r[0].area() >= r[1].area());
    }
}
