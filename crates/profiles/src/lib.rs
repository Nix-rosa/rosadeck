//! `rosadeck-profiles`: TOML display profiles — load, validate, save, match.
//!
//! No backend code: matching is pure (`connector_hint` is never consulted).
//! Priority: 1. EDID hash / stable-id exact, 2. PNP + product + serial
//! (profile serial `None` = wildcard), 3. manual alias.
//! F5 adds emulator + launch profiles (persistence only, no execution).

pub mod emulator;

pub use emulator::{
    find_emulator, load_emulator, load_launch, parse_emulator, parse_launch, EmulatorGraphicsSection,
    EmulatorLaunchSection, EmulatorProfile, EmulatorProfileError, EmulatorSection, EmulatorShaderSection,
    LaunchBinding, LaunchDisplay, LaunchGame, LaunchProfile,
};

use display_core::Mode;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::Path;

/// Screen disposition requested by a profile (data only; applied in F2+).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScreenMode {
    /// Mirror the internal output.
    Duplicate,
    /// Internal off, external on.
    ExternalOnly,
    /// Side-by-side outputs.
    Extend,
}

/// `[display]` section: who the panel is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplaySection {
    /// Canonical `<pnp>-<product>-<hash8>`.
    pub stable_id: String,
    /// Human alias (e.g. `samsung-tv`).
    pub alias: Option<String>,
    /// 3-letter PNP (e.g. `SDC`).
    pub pnp: String,
    /// Product code as hex (`0x4161` or `4161`).
    pub product: String,
    /// 8-hex EDID hash (first 128 bytes, SHA-256).
    pub edid_hash: String,
    /// 32-bit serial when known.
    pub serial: Option<u32>,
    /// Last-seen connector; hint only, never identity.
    pub connector_hint: Option<String>,
}

/// `[preferred]` section: what to use (selection itself is F2's job).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreferredSection {
    /// Desired disposition.
    pub mode: ScreenMode,
    /// Preferred resolution `WxH`.
    pub resolution: Option<String>,
    /// Preferred refresh in Hz.
    pub refresh: Option<f64>,
    /// Selection policy name (`max-resolution`, `max-refresh`, `balanced`,
    /// `match-internal`, `match-external`, `manual`).
    pub policy: String,
}

/// A complete display profile (one TOML file).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayProfile {
    /// Identity section.
    pub display: DisplaySection,
    /// Preference section.
    pub preferred: PreferredSection,
}

/// Failure to load, parse or validate a profile.
#[derive(Debug)]
pub enum ProfileError {
    /// Filesystem read failure.
    Io(std::io::Error),
    /// TOML syntax/type failure.
    Toml(toml::de::Error),
    /// TOML serialization failure (save).
    TomlSer(toml::ser::Error),
    /// Semantic validation failure (message).
    Invalid(String),
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Toml(e) => write!(f, "toml: {e}"),
            Self::TomlSer(e) => write!(f, "toml serialize: {e}"),
            Self::Invalid(m) => write!(f, "invalid profile: {m}"),
        }
    }
}

impl std::error::Error for ProfileError {}

/// Parse the `0x4161`/`4161` product form.
fn parse_product(s: &str) -> Option<u16> {
    let t = s.trim().strip_prefix("0x").or_else(|| s.trim().strip_prefix("0X")).unwrap_or(s.trim());
    u16::from_str_radix(t, 16).ok()
}

/// Validate one profile's semantics (called by [`parse_str`] and [`load`]).
pub fn validate(p: &DisplayProfile) -> Result<(), ProfileError> {
    let bad = |m: &str| ProfileError::Invalid(m.to_owned());
    let parts: Vec<&str> = p.display.stable_id.split('-').collect();
    if parts.len() != 3 || parts[0].len() != 3 || parts[1].len() != 4 || parts[2].len() != 8 {
        return Err(bad("display.stable_id must be <pnp>-<product>-<hash8>"));
    }
    if p.display.pnp.len() != 3 || !p.display.pnp.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err(bad("display.pnp must be 3 ASCII letters"));
    }
    let product = parse_product(&p.display.product).ok_or_else(|| bad("display.product must be hex like 0x4161"))?;
    if p.display.edid_hash.len() != 8 || !p.display.edid_hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad("display.edid_hash must be 8 hex chars"));
    }
    // Cross-check: stable_id must agree with pnp/product/hash parts.
    if parts[0].to_ascii_lowercase() != p.display.pnp.to_ascii_lowercase()
        || u16::from_str_radix(parts[1], 16).ok() != Some(product)
        || parts[2].to_ascii_lowercase() != p.display.edid_hash.to_ascii_lowercase()
    {
        return Err(bad("display.stable_id disagrees with pnp/product/edid_hash"));
    }
    if let Some(r) = &p.preferred.resolution {
        let (w, h) = r.split_once('x').ok_or_else(|| bad("preferred.resolution must be WxH"))?;
        if w.trim().parse::<u32>().map(|v| v == 0).unwrap_or(true) || h.trim().parse::<u32>().map(|v| v == 0).unwrap_or(true) {
            return Err(bad("preferred.resolution must be WxH with positive integers"));
        }
    }
    if let Some(hz) = p.preferred.refresh {
        if !hz.is_finite() || hz <= 0.0 {
            return Err(bad("preferred.refresh must be positive"));
        }
    }
    match p.preferred.policy.as_str() {
        "max-resolution" | "max-refresh" | "balanced" | "match-internal" | "match-external" | "manual" => {}
        other => return Err(bad(&format!("unknown preferred.policy: {other}"))),
    }
    Ok(())
}

/// Parse + validate TOML text.
pub fn parse_str(text: &str) -> Result<DisplayProfile, ProfileError> {
    let p: DisplayProfile = toml::from_str(text).map_err(ProfileError::Toml)?;
    validate(&p)?;
    Ok(p)
}

/// Load + validate one TOML file (read-only).
pub fn load(path: &Path) -> Result<DisplayProfile, ProfileError> {
    let text = fs::read_to_string(path).map_err(ProfileError::Io)?;
    parse_str(&text)
}

/// Serialize a profile to TOML text (validated first).
pub fn dump(p: &DisplayProfile) -> Result<String, ProfileError> {
    validate(p)?;
    toml::to_string_pretty(p).map_err(ProfileError::TomlSer)
}

/// Save a profile to `path` (the only writer; used by CLI/tests, never by F1 logic).
pub fn save(path: &Path, p: &DisplayProfile) -> Result<(), ProfileError> {
    fs::write(path, dump(p)?).map_err(ProfileError::Io)
}

/// Preferred resolution as a [`Mode`] when both parts are present
/// (refresh missing → unknown Hz, never assumed).
pub fn preferred_mode(p: &DisplayProfile) -> Option<Mode> {
    let r = p.preferred.resolution.as_ref()?;
    let (w, h) = r.split_once('x')?;
    Mode::new(w.trim().parse().ok()?, h.trim().parse().ok()?, p.preferred.refresh.unwrap_or(0.0))
}

/// Observed display used as a match query. `connector` is accepted for API
/// ergonomics but deliberately ignored by [`find_best`].
#[derive(Debug, Clone, Default)]
pub struct MatchQuery {
    /// Full stable id, when computed from EDID.
    pub stable_id: Option<String>,
    /// 8-hex EDID hash.
    pub edid_hash: Option<String>,
    /// PNP manufacturer.
    pub pnp: Option<String>,
    /// Product code.
    pub product: Option<u16>,
    /// Serial (None = unknown).
    pub serial: Option<u32>,
    /// Manually assigned alias.
    pub alias: Option<String>,
    /// Present but NEVER used for matching (regression guard).
    pub connector: Option<String>,
}

/// Which tier matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchTier {
    /// Exact EDID hash / stable-id.
    EdidHash,
    /// PNP + product + serial (profile serial None = wildcard).
    PnpProductSerial,
    /// Manual alias.
    Alias,
}

/// Best match across `profiles` by priority tier (first profile wins per tier).
pub fn find_best<'a>(profiles: &'a [DisplayProfile], q: &MatchQuery) -> Option<(&'a DisplayProfile, MatchTier)> {
    // Tier 1: exact EDID hash or full stable-id.
    if q.stable_id.is_some() || q.edid_hash.is_some() {
        for p in profiles {
            let hash_eq = q.edid_hash.as_ref().is_some_and(|h| h.eq_ignore_ascii_case(&p.display.edid_hash));
            let id_eq = q.stable_id.as_ref().is_some_and(|id| id.eq_ignore_ascii_case(&p.display.stable_id));
            if hash_eq || id_eq {
                return Some((p, MatchTier::EdidHash));
            }
        }
    }
    // Tier 2: PNP + product + serial (profile serial None matches any).
    if let (Some(pnp), Some(prod)) = (q.pnp.as_ref(), q.product) {
        for p in profiles {
            if !p.display.pnp.eq_ignore_ascii_case(pnp) {
                continue;
            }
            if parse_product(&p.display.product) != Some(prod) {
                continue;
            }
            let serial_ok = match (p.display.serial, q.serial) {
                (Some(a), Some(b)) => a == b,
                (None, _) | (_, None) => true,
            };
            if serial_ok {
                return Some((p, MatchTier::PnpProductSerial));
            }
        }
    }
    // Tier 3: manual alias.
    if let Some(alias) = q.alias.as_ref() {
        for p in profiles {
            if p.display.alias.as_ref().is_some_and(|a| a == alias) {
                return Some((p, MatchTier::Alias));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../tests/fixtures/samsung-tv.toml");

    fn sample() -> DisplayProfile {
        parse_str(SAMPLE).unwrap()
    }

    #[test]
    fn parses_sample_toml() {
        let p = sample();
        assert_eq!(p.display.alias.as_deref(), Some("samsung-tv"));
        assert_eq!(p.preferred.policy, "max-refresh");
        assert!(matches!(p.preferred.mode, ScreenMode::ExternalOnly));
    }

    #[test]
    fn preferred_mode_keeps_exact_refresh() {
        let m = preferred_mode(&sample()).unwrap();
        assert_eq!((m.width, m.height), (1920, 1080));
        assert!((m.hz - 120.0).abs() < 1e-9);
    }

    #[test]
    fn rejects_invalid_profiles() {
        assert!(parse_str("mode = 1").is_err()); // wrong shape
        let mut bad = SAMPLE.replace("max-refresh", "turbo");
        assert!(parse_str(&bad).is_err());
        bad = SAMPLE.replace("1920x1080", "1920x");
        assert!(parse_str(&bad).is_err());
        bad = SAMPLE.replace("sdc-4161-", "sdc-9999-");
        assert!(parse_str(&bad).is_err()); // stable_id cross-check
    }

    #[test]
    fn tier1_edid_hash_wins() {
        let p = sample();
        let q = MatchQuery {
            edid_hash: Some(p.display.edid_hash.clone()),
            pnp: Some("XXX".into()),
            ..Default::default()
        };
        assert_eq!(find_best(&[p], &q).unwrap().1, MatchTier::EdidHash);
    }

    #[test]
    fn tier2_pnp_product_serial_with_wildcard() {
        let p = sample(); // sample serial None -> wildcard
        let q = MatchQuery { pnp: Some("SDC".into()), product: Some(0x4161), serial: Some(123), ..Default::default() };
        assert_eq!(find_best(&[p], &q).unwrap().1, MatchTier::PnpProductSerial);
    }

    #[test]
    fn tier2_serial_mismatch_rejects_when_both_known() {
        let mut prof = sample();
        prof.display.serial = Some(1);
        let q = MatchQuery { pnp: Some("SDC".into()), product: Some(0x4161), serial: Some(2), ..Default::default() };
        assert!(find_best(&[prof], &q).is_none());
    }

    #[test]
    fn tier3_alias_fallback() {
        let p = sample();
        let q = MatchQuery { alias: Some("samsung-tv".into()), ..Default::default() };
        assert_eq!(find_best(&[p], &q).unwrap().1, MatchTier::Alias);
    }

    #[test]
    fn connector_change_never_affects_matching() {
        let p = sample();
        let base = MatchQuery { edid_hash: Some(p.display.edid_hash.clone()), ..Default::default() };
        let on_a1 = MatchQuery { connector: Some("HDMI-A-1".into()), ..base.clone() };
        let on_a2 = MatchQuery { connector: Some("HDMI-A-2".into()), ..base };
        assert_eq!(find_best(&[p.clone()], &on_a1).unwrap().1, MatchTier::EdidHash);
        assert_eq!(find_best(&[p], &on_a2).unwrap().1, MatchTier::EdidHash);
    }

    #[test]
    fn save_load_roundtrip_in_tempdir() {
        let p = sample();
        let dir = std::env::temp_dir().join("rosadeck-profiles-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rt.toml");
        save(&path, &p).unwrap();
        assert_eq!(load(&path).unwrap(), p);
        std::fs::remove_dir_all(&dir).ok();
    }
}
