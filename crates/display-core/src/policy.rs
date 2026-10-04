//! Pure mode-selection policies: `Vec<Mode> -> ModePolicy -> Option<Mode>`.
//!
//! Deterministic rules (no default policy is chosen here; that is an F2+
//! product decision):
//!
//! - [`ModePolicy::MaxResolution`]: largest area; ties broken by higher Hz.
//! - [`ModePolicy::MaxRefresh`]: highest known Hz; ties broken by larger area.
//!   Modes with unknown Hz only win if *all* candidates are unknown.
//! - [`ModePolicy::Balanced`]: largest area among modes whose Hz is at or
//!   above the median of known Hz values (ties: higher Hz). If no Hz is
//!   known, falls back to [`ModePolicy::MaxResolution`].
//! - [`ModePolicy::MatchInternal`] / [`ModePolicy::MatchExternal`]: same WxH
//!   as the reference set's max-resolution mode (highest Hz among those);
//!   falls back to [`ModePolicy::MaxResolution`] when absent. Require a
//!   [`PolicyContext`]; without one they return `None` (explicit, not silent).
//! - [`ModePolicy::Manual`]: the carried mode iff a candidate matches it
//!   (same WxH, Hz within [`HZ_TOLERANCE`](crate::mode::HZ_TOLERANCE));
//!   the canonical candidate value is returned.

use crate::mode::{Mode, HZ_TOLERANCE};
use serde::{Deserialize, Serialize};

/// Display-mode selection policy (pure data; application is F2's job).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModePolicy {
    /// Largest area, then highest Hz.
    MaxResolution,
    /// Highest known Hz, then largest area.
    MaxRefresh,
    /// Largest area at/above median Hz (see module docs).
    Balanced,
    /// Same WxH as the internal output's preferred mode.
    MatchInternal,
    /// Same WxH as the external output's preferred mode.
    MatchExternal,
    /// Explicit mode; must exist among candidates (within tolerance).
    Manual(Mode),
}

/// Reference mode sets needed by match-policies.
#[derive(Debug, Clone)]
pub struct PolicyContext<'a> {
    /// Modes of the internal (laptop) output.
    pub internal: &'a [Mode],
    /// Modes of the external output.
    pub external: &'a [Mode],
}

fn max_resolution(candidates: &[Mode]) -> Option<Mode> {
    candidates.iter().max_by(|a, b| a.area().cmp(&b.area()).then(a.hz.total_cmp(&b.hz))).copied()
}

fn max_refresh(candidates: &[Mode]) -> Option<Mode> {
    // Unknown-Hz modes rank below any known Hz.
    candidates
        .iter()
        .max_by(|a, b| {
            (!a.is_unknown_refresh())
                .cmp(&!b.is_unknown_refresh())
                .then(a.hz.total_cmp(&b.hz))
                .then(a.area().cmp(&b.area()))
        })
        .copied()
}

fn balanced(candidates: &[Mode]) -> Option<Mode> {
    let mut hz: Vec<f64> = candidates.iter().filter(|m| !m.is_unknown_refresh()).map(|m| m.hz).collect();
    if hz.is_empty() {
        return max_resolution(candidates);
    }
    hz.sort_by(|a, b| a.total_cmp(b));
    let median = hz[hz.len() / 2];
    let eligible: Vec<Mode> = candidates.iter().filter(|m| !m.is_unknown_refresh() && m.hz >= median).copied().collect();
    max_resolution(&eligible)
}

fn match_reference(candidates: &[Mode], reference: &[Mode]) -> Option<Mode> {
    let target = max_resolution(reference)?;
    let same: Vec<Mode> =
        candidates.iter().filter(|m| m.width == target.width && m.height == target.height).copied().collect();
    if same.is_empty() {
        return max_resolution(candidates);
    }
    max_refresh(&same)
}

/// Select one mode from `candidates` under `policy` (deterministic).
pub fn select_mode(candidates: &[Mode], policy: &ModePolicy, ctx: Option<&PolicyContext>) -> Option<Mode> {
    if candidates.is_empty() {
        return None;
    }
    match policy {
        ModePolicy::MaxResolution => max_resolution(candidates),
        ModePolicy::MaxRefresh => max_refresh(candidates),
        ModePolicy::Balanced => balanced(candidates),
        ModePolicy::MatchInternal => match_reference(candidates, ctx?.internal),
        ModePolicy::MatchExternal => match_reference(candidates, ctx?.external),
        ModePolicy::Manual(want) => {
            candidates.iter().find(|m| m.matches_within(*want, HZ_TOLERANCE)).copied()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::parse_mode;

    fn modes(ss: &[&str]) -> Vec<Mode> {
        ss.iter().map(|s| parse_mode(s).unwrap()).collect()
    }

    #[test]
    fn max_resolution_picks_largest_area_then_hz() {
        let c = modes(&["1920x1080@120", "2560x1440@60", "2560x1440@144"]);
        assert_eq!(select_mode(&c, &ModePolicy::MaxResolution, None).unwrap(), parse_mode("2560x1440@144").unwrap());
    }

    #[test]
    fn max_refresh_picks_highest_hz() {
        let c = modes(&["3840x2160@60", "1920x1080@120", "2560x1440@144"]);
        assert_eq!(select_mode(&c, &ModePolicy::MaxRefresh, None).unwrap(), parse_mode("2560x1440@144").unwrap());
    }

    #[test]
    fn max_refresh_ignores_unknown_hz_when_known_exist() {
        let c = modes(&["1920x1080", "1280x720@60"]);
        assert_eq!(select_mode(&c, &ModePolicy::MaxRefresh, None).unwrap(), parse_mode("1280x720@60").unwrap());
    }

    #[test]
    fn balanced_prefers_large_area_at_good_hz() {
        // Hz: 60,60,120,144 -> median 120 -> eligible 120,144 -> largest area wins.
        let c = modes(&["3840x2160@60", "1920x1080@60", "1920x1080@120", "2560x1440@144"]);
        assert_eq!(select_mode(&c, &ModePolicy::Balanced, None).unwrap(), parse_mode("2560x1440@144").unwrap());
    }

    #[test]
    fn match_internal_falls_back_without_context() {
        let c = modes(&["1920x1080@60"]);
        assert_eq!(select_mode(&c, &ModePolicy::MatchInternal, None), None);
    }

    #[test]
    fn match_internal_picks_same_wxh_then_falls_back() {
        let c = modes(&["1920x1080@120", "3840x2160@60"]);
        let internal = modes(&["2560x1440@144", "1920x1080@60"]);
        let external = modes(&["3840x2160@120"]);
        let ctx = PolicyContext { internal: &internal, external: &external };
        // internal preferred WxH = 2560x1440, absent -> MaxResolution fallback.
        assert_eq!(
            select_mode(&c, &ModePolicy::MatchInternal, Some(&ctx)).unwrap(),
            parse_mode("3840x2160@60").unwrap()
        );
        let c2 = modes(&["2560x1440@60", "3840x2160@60"]);
        assert_eq!(
            select_mode(&c2, &ModePolicy::MatchInternal, Some(&ctx)).unwrap(),
            parse_mode("2560x1440@60").unwrap()
        );
    }

    #[test]
    fn manual_requires_membership_within_tolerance() {
        let c = modes(&["1920x1080@59.94", "1280x720@60"]);
        let want = parse_mode("1920x1080@60").unwrap();
        assert_eq!(
            select_mode(&c, &ModePolicy::Manual(want), None).unwrap(),
            parse_mode("1920x1080@59.94").unwrap()
        );
        let missing = parse_mode("3840x2160@60").unwrap();
        assert_eq!(select_mode(&c, &ModePolicy::Manual(missing), None), None);
    }

    #[test]
    fn empty_candidates_is_none() {
        assert_eq!(select_mode(&[], &ModePolicy::MaxRefresh, None), None);
    }
}
