//! Read-only reconciliation: Hyprland + DRM + EDID/CTA → [`UnifiedDisplay`].
//!
//! Precedence (conceptual, from research):
//! Hyprland = current compositor state; DRM = connector/kernel state;
//! EDID = physical identity + base capabilities; CTA = extra capabilities.
//! Discrepancies become `warnings`/`notes`. Nothing is modified or invented.
//!
//! [`unify_with`] is pure (fixture-testable); [`unify`] only fetches live
//! sources and delegates.

use crate::backend::{BackendResult, DisplayBackend, HyprlandBackend};
use crate::model::HyprMonitorInfo;
use display_core::{merge_modes, ConnectorId, DisplayIdentity, Mode, ModeSource, OutputState, SourcedMode};
use rosadeck_drm::{walk, ConnectorState, ConnectorStatus};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Default sysfs DRM base (overridable in tests).
pub const DEFAULT_SYSFS: &str = "/sys/class/drm";

/// One physical/logical display as seen from every source at once.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedDisplay {
    /// Compositor state (`None` when invisible to Hyprland, e.g. unplugged HDMI).
    pub output: Option<OutputState>,
    /// Compositor metadata (`None` with `output`).
    pub info: Option<HyprMonitorInfo>,
    /// Kernel connector short name (`HDMI-A-1`); hint only.
    pub connector: ConnectorId,
    /// DRM attachment state.
    pub drm_connected: bool,
    /// Stable EDID identity (`None` when EDID absent/unparseable).
    pub identity: Option<DisplayIdentity>,
    /// Monitor name from EDID text descriptor, when present.
    pub edid_name: Option<String>,
    /// Merged modes with provenance (deterministic order).
    pub modes: Vec<SourcedMode>,
    /// Mismatches between sources (never auto-corrected).
    pub warnings: Vec<String>,
    /// Expected absences (e.g. unplugged HDMI invisible to compositor).
    pub notes: Vec<String>,
}

/// Pure join of compositor outputs and DRM connectors (no IO; fully testable).
pub fn unify_with(outputs: &[(OutputState, HyprMonitorInfo)], connectors: &[ConnectorState]) -> Vec<UnifiedDisplay> {
    let mut names: Vec<String> = connectors.iter().map(|c| c.name.clone()).collect();
    for (st, _) in outputs {
        if !names.contains(&st.id.0) {
            names.push(st.id.0.clone());
        }
    }
    names.sort();

    let mut out = Vec::new();
    for name in names {
        let hy = outputs.iter().find(|(st, _)| st.id.0 == name).cloned();
        let drm = connectors.iter().find(|c| c.name == name);
        let mut warnings = Vec::new();
        let mut notes = Vec::new();
        let mut observations: Vec<(Mode, ModeSource)> = Vec::new();

        if let Some((_, info)) = &hy {
            for s in &info.available_modes {
                match display_core::parse_mode(s) {
                    Ok(m) => observations.push((m, ModeSource::Hyprland)),
                    Err(_) => warnings.push(format!("{name}: Hyprland mode unparseable: {s}")),
                }
            }
        }
        if let Some(c) = drm {
            observations.extend(c.modes.iter().map(|m| (*m, ModeSource::Drm)));
        }

        // EDID identity + capabilities (failures degrade to warnings, never fatal).
        let mut identity = None;
        let mut edid_name = None;
        if let Some(c) = drm {
            if c.edid.len() >= 128 {
                match rosadeck_edid::parse_edid(&c.edid) {
                    Ok(edid) => {
                        // Serial 0 conventionally means "no serial": keep None.
                        let serial = if edid.serial == 0 { None } else { Some(edid.serial) };
                        match rosadeck_edid::compute_stable_id(&c.edid) {
                            Ok(sid) => {
                                let hash8 = sid.rsplit('-').next().unwrap_or("").to_owned();
                                match DisplayIdentity::new(&edid.pnp, edid.product, serial, &hash8) {
                                    Some(mut id) => {
                                        id.connector_hint = Some(ConnectorId(name.clone()));
                                        identity = Some(id);
                                    }
                                    None => warnings.push(format!("{name}: EDID identity construction failed")),
                                }
                            }
                            Err(e) => warnings.push(format!("{name}: stable-id failed: {e}")),
                        }
                        if !edid.name.is_empty() {
                            edid_name = Some(edid.name.clone());
                        }
                        for dtd in edid.detailed_timings() {
                            if let Some(m) = dtd.as_mode() {
                                observations.push((m, ModeSource::EdidDtd));
                            }
                        }
                        for ext in &edid.extensions {
                            if let rosadeck_edid::Extension::Cta861(cta) = ext {
                                for (vic, _) in cta.vics() {
                                    match rosadeck_edid::vic_to_mode(vic) {
                                        Some(m) => observations.push((m, ModeSource::CtaVic)),
                                        None => notes.push(format!(
                                            "{name}: CTA VIC {vic} not in table (preserved, not interpreted)"
                                        )),
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => warnings.push(format!("{name}: EDID parse failed: {e}")),
                }
            } else if hy.is_some() {
                warnings.push(format!("{name}: DRM EDID absent but Hyprland knows this output"));
            }
        }

        // Cross-source checks (WxH-level: DRM carries no Hz).
        if let (Some((_, info)), Some(c)) = (&hy, drm) {
            let drm_wh: Vec<(u32, u32)> = c.modes.iter().map(|m| (m.width, m.height)).collect();
            for s in &info.available_modes {
                if let Ok(m) = display_core::parse_mode(s) {
                    if !drm_wh.contains(&(m.width, m.height)) {
                        warnings.push(format!("{name}: Hyprland mode {}x{} not in DRM modes", m.width, m.height));
                    }
                }
            }
        }
        if hy.is_none() {
            notes.push(format!("{name}: invisible to compositor (disconnected/disabled)"));
        }

        out.push(UnifiedDisplay {
            output: hy.clone().map(|(st, _)| st),
            info: hy.map(|(_, info)| info),
            connector: ConnectorId(name),
            drm_connected: matches!(drm.map(|c| c.status), Some(ConnectorStatus::Connected)),
            identity,
            edid_name,
            modes: merge_modes(&observations),
            warnings,
            notes,
        });
    }
    out
}

/// Live DRM connector short names (read-only; for the hotplug-disconnect rule).
pub fn connected_connector_names(sysfs_base: &Path) -> Vec<String> {
    walk(sysfs_base).into_iter().filter(|c| c.status == ConnectorStatus::Connected).map(|c| c.name).collect()
}

/// Live read-only reconciliation: fetch Hyprland + sysfs, then [`unify_with`].
pub fn unify(sysfs_base: &Path) -> BackendResult<Vec<UnifiedDisplay>> {
    let outputs = HyprlandBackend.list_outputs()?;
    let connectors = walk(sysfs_base);
    Ok(unify_with(&outputs, &connectors))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{normalize, HyprMonitor};

    const MONITORS: &str = include_str!("../tests/fixtures/monitors.json");
    const REAL_EDID: &[u8] = include_bytes!("../../edid/tests/fixtures/edp1-edid.bin");

    fn live_shaped_inputs() -> (Vec<(OutputState, HyprMonitorInfo)>, Vec<ConnectorState>) {
        // Same shape as the real machine: eDP-1 everywhere + 2 absent HDMIs.
        let mons: Vec<HyprMonitor> = serde_json::from_str(MONITORS).unwrap();
        let outputs = mons.iter().map(normalize).collect();
        let connectors = vec![
            ConnectorState {
                drm_name: "card1-eDP-1".into(),
                name: "eDP-1".into(),
                status: ConnectorStatus::Connected,
                enabled: true,
                modes: vec![display_core::parse_mode("1920x1080").unwrap()],
                edid: REAL_EDID.to_vec(),
            },
            ConnectorState {
                drm_name: "card1-HDMI-A-1".into(),
                name: "HDMI-A-1".into(),
                status: ConnectorStatus::Disconnected,
                enabled: false,
                modes: vec![],
                edid: vec![],
            },
        ];
        (outputs, connectors)
    }

    #[test]
    fn unifies_identity_modes_and_provenance() {
        let (outputs, connectors) = live_shaped_inputs();
        let u = unify_with(&outputs, &connectors);
        assert_eq!(u.len(), 2);
        let edp = u.iter().find(|d| d.connector.0 == "eDP-1").unwrap();
        assert!(edp.drm_connected);
        assert!(edp.output.is_some());
        let id = edp.identity.as_ref().unwrap();
        assert!(id.stable_id().starts_with("sdc-4161-"));
        assert_eq!(edp.edid_name.as_deref(), Some("ATNA56YX03-0"));
        // 1920x1080 declared by Hyprland + DRM + EDID DTD (59.997≈59.9967).
        let m1080 = edp.modes.iter().find(|m| m.mode.width == 1920).unwrap();
        assert!(m1080.sources.contains(&ModeSource::Hyprland));
        assert!(m1080.sources.contains(&ModeSource::Drm));
        assert!(m1080.sources.contains(&ModeSource::EdidDtd));
        assert!(edp.warnings.is_empty());
        let hdmi = u.iter().find(|d| d.connector.0 == "HDMI-A-1").unwrap();
        assert!(!hdmi.drm_connected);
        assert!(hdmi.output.is_none() && hdmi.identity.is_none());
        assert!(hdmi.notes.iter().any(|n| n.contains("invisible to compositor")));
    }

    #[test]
    fn corrupt_edid_degrades_to_warning() {
        let (outputs, mut connectors) = live_shaped_inputs();
        connectors[0].edid = vec![0xFFu8; 256]; // bad header
        let u = unify_with(&outputs, &connectors);
        let edp = u.iter().find(|d| d.connector.0 == "eDP-1").unwrap();
        assert!(edp.identity.is_none());
        assert!(edp.warnings.iter().any(|w| w.contains("EDID parse failed")));
        assert!(edp.output.is_some()); // compositor state survives
    }
}
