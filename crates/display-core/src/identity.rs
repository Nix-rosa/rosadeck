//! Stable display identity: EDID-based, never connector-based.
//!
//! Connector names (`HDMI-A-1`) move between ports/GPUs, so identity is
//! `pnp + product [+ serial] + edid_hash`, formatted as
//! `<pnp>-<product>-<hash8>` (e.g. `sdc-4161-9f2c…`). The connector is
//! kept only as [`DisplayIdentity::connector_hint`].

use crate::layout::ConnectorId;
use serde::{Deserialize, Serialize};

/// Identity of a physical display panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayIdentity {
    /// 3-letter PNP manufacturer, uppercase (e.g. `SDC`).
    pub pnp: String,
    /// 16-bit EDID product code.
    pub product: u16,
    /// 32-bit EDID serial (may be 0/absent on laptop panels).
    pub serial: Option<u32>,
    /// First 8 lowercase hex chars of SHA-256 over the first 128 EDID bytes.
    pub edid_hash: String,
    /// Last-seen connector; hint only, never identity.
    pub connector_hint: Option<ConnectorId>,
}

impl DisplayIdentity {
    /// Validated constructor (`pnp` must be 3 ASCII letters, `edid_hash` 8 hex).
    pub fn new(pnp: &str, product: u16, serial: Option<u32>, edid_hash: &str) -> Option<Self> {
        if pnp.len() != 3 || !pnp.bytes().all(|b| b.is_ascii_alphabetic()) {
            return None;
        }
        if edid_hash.len() != 8 || !edid_hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        Some(Self {
            pnp: pnp.to_ascii_uppercase(),
            product,
            serial,
            edid_hash: edid_hash.to_ascii_lowercase(),
            connector_hint: None,
        })
    }

    /// Canonical stable id: `<pnp-lower>-<product-04x>-<hash8>`.
    pub fn stable_id(&self) -> String {
        format!("{}-{:04x}-{}", self.pnp.to_ascii_lowercase(), self.product, self.edid_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_id_format() {
        let id = DisplayIdentity::new("SDC", 0x4161, Some(0), "9f2cab11").unwrap();
        assert_eq!(id.stable_id(), "sdc-4161-9f2cab11");
    }

    #[test]
    fn rejects_bad_pnp_and_hash() {
        assert!(DisplayIdentity::new("SD", 1, None, "9f2cab11").is_none());
        assert!(DisplayIdentity::new("SDC", 1, None, "xyz").is_none());
    }

    #[test]
    fn connector_hint_does_not_change_identity() {
        let mut a = DisplayIdentity::new("SAM", 0x0722, None, "aaaaaaaa").unwrap();
        let mut b = a.clone();
        a.connector_hint = Some(ConnectorId("HDMI-A-1".into()));
        b.connector_hint = Some(ConnectorId("HDMI-A-2".into()));
        assert_eq!(a.stable_id(), b.stable_id());
    }
}
