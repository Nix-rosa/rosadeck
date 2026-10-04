//! `rosadeck-edid`: minimal EDID parsing, identity and timings.
//!
//! Responsibilities are split: [`parse_edid`] (structure), [`compute_stable_id`]
//! (identity over the first 128 bytes) and [`Edid::detailed_timings`] (mode
//! source). Raw bytes are always preserved. CTA-861/HDR blocks are reported
//! as present (extension count) but not interpreted yet — explicitly out of
//! scope for F1.

use display_core::Mode;
use sha2::{Digest, Sha256};
use std::fmt;

pub mod cta;
pub mod vic;

pub use cta::{CtaDataBlock, CtaExtension};
pub use vic::{vic_info, vic_to_mode, VicInfo};

/// Parse/identity errors.
#[derive(Debug, Clone, PartialEq)]
pub enum EdidError {
    /// Fewer than 128 bytes.
    TooShort(usize),
    /// First 8 bytes are not `00 FF FF FF FF FF FF 00`.
    BadHeader,
    /// Extension block is not exactly 128 bytes.
    BadBlockSize(usize),
    /// A data block claims more bytes than the collection holds.
    TruncatedCta {
        /// Block tag being parsed.
        tag: u8,
        /// Claimed payload length.
        len: usize,
        /// Bytes actually available.
        available: usize,
    },
    /// Malformed CTA header/content.
    InvalidCta(String),
}

impl fmt::Display for EdidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort(n) => write!(f, "EDID too short: {n} bytes (need >= 128)"),
            Self::BadHeader => write!(f, "bad EDID header"),
            Self::BadBlockSize(n) => write!(f, "extension block size {n} != 128"),
            Self::TruncatedCta { tag, len, available } => {
                write!(f, "truncated CTA block: tag {tag} claims {len}B, {available}B available")
            }
            Self::InvalidCta(m) => write!(f, "invalid CTA: {m}"),
        }
    }
}

impl std::error::Error for EdidError {}

/// Decode the 3-letter PNP manufacturer from bytes 8–9.
pub fn decode_pnp(bytes: &[u8]) -> String {
    let raw = ((bytes[8] as u16) << 8) | bytes[9] as u16;
    [(raw >> 10) & 0x1f, (raw >> 5) & 0x1f, raw & 0x1f]
        .iter()
        .map(|v| ((*v as u8) + b'A' - 1) as char)
        .collect()
}

/// Extract the monitor name from a `0xFC`/`0xFE` text descriptor, if any.
fn text_descriptor(data: &[u8]) -> String {
    for i in 0..4 {
        let off = 54 + i * 18;
        if data[off] == 0 && data[off + 1] == 0 && (data[off + 3] == 0xFC || data[off + 3] == 0xFE) {
            let s: String =
                data[off + 5..off + 18].iter().take_while(|&&b| b != 0x0A && b != 0).map(|&b| b as char).collect();
            let s = s.trim().to_owned();
            if !s.is_empty() {
                return s;
            }
        }
    }
    String::new()
}

/// An EDID extension block: interpreted when known, preserved when not.
#[derive(Debug, Clone, PartialEq)]
pub enum Extension {
    /// Parsed CTA-861 extension.
    Cta861(CtaExtension),
    /// Any other tag: raw bytes preserved, never interpreted.
    Unknown {
        /// Extension tag (block byte 0).
        tag: u8,
        /// Original 128 bytes.
        raw: Vec<u8>,
    },
}

/// Parsed EDID base block (+ raw bytes for everything not yet interpreted).
#[derive(Debug, Clone)]
pub struct Edid {
    /// 3-letter manufacturer (e.g. `SDC`).
    pub pnp: String,
    /// 16-bit product code.
    pub product: u16,
    /// 32-bit serial number field (often 0 on laptop panels).
    pub serial: u32,
    /// Manufacture week (1–53, 0/255 = unspecified).
    pub week: u8,
    /// Full manufacture year (1990 + byte 17).
    pub year: u16,
    /// Monitor name from text descriptor, if present.
    pub name: String,
    /// Extension-block count announced by byte 126.
    pub extension_blocks: u8,
    /// Parsed/preserved extension blocks, in order (CTA-861 or unknown).
    pub extensions: Vec<Extension>,
    /// Complete original bytes (all blocks).
    pub raw: Vec<u8>,
}

/// Validate the header and parse the base-block identity fields.
/// Accepts 128 or 256+ bytes; shorter inputs are rejected.
pub fn parse_edid(data: &[u8]) -> Result<Edid, EdidError> {
    if data.len() < 128 {
        return Err(EdidError::TooShort(data.len()));
    }
    if data[0..8] != [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00] {
        return Err(EdidError::BadHeader);
    }
    if data.len() % 128 != 0 {
        return Err(EdidError::BadBlockSize(data.len()));
    }
    let mut extensions = Vec::new();
    for block in data[128..].chunks_exact(128) {
        extensions.push(if block[0] == 0x02 {
            Extension::Cta861(cta::parse_cta(block)?)
        } else {
            Extension::Unknown { tag: block[0], raw: block.to_vec() }
        });
    }
    Ok(Edid {
        pnp: decode_pnp(data),
        product: u16::from_le_bytes([data[10], data[11]]),
        serial: u32::from_le_bytes([data[12], data[13], data[14], data[15]]),
        week: data[16],
        year: 1990 + data[17] as u16,
        name: text_descriptor(data),
        extension_blocks: data[126],
        extensions,
        raw: data.to_vec(),
    })
}

/// Stable id `<pnp-lower>-<product-04x>-<hash8>` where `hash8` is the first
/// 8 lowercase hex chars of SHA-256 over the **first 128 bytes**.
///
/// NOTE: F0 used FNV-1a here; F1 switches to SHA-256 (decision §9 of F1 doc),
/// so F0 ids like `sdc-4161-3b28cae1` are not comparable to F1 ids.
pub fn compute_stable_id(data: &[u8]) -> Result<String, EdidError> {
    let edid = parse_edid(data)?;
    let digest = Sha256::digest(&edid.raw[..128]);
    let hash8 = format!("{digest:x}")[..8].to_owned();
    Ok(format!("{}-{:04x}-{hash8}", edid.pnp.to_ascii_lowercase(), edid.product))
}

/// One Detailed Timing Descriptor: full timing + derived refresh.
#[derive(Debug, Clone, PartialEq)]
pub struct DetailedTiming {
    /// Original 18 bytes.
    pub raw: [u8; 18],
    /// Pixel clock in Hz.
    pub pixel_clock_hz: u64,
    /// Horizontal active pixels.
    pub h_active: u32,
    /// Horizontal blanking pixels.
    pub h_blank: u32,
    /// Vertical active lines.
    pub v_active: u32,
    /// Vertical blanking lines.
    pub v_blank: u32,
    /// Derived refresh: `pixel_clock / (h_total * v_total)`, full precision.
    pub refresh_hz: f64,
}

impl DetailedTiming {
    /// Corresponding [`Mode`] (resolution + derived refresh, unrounded).
    /// Returns `None` for degenerate descriptors (zero totals).
    pub fn as_mode(&self) -> Option<Mode> {
        Mode::new(self.h_active, self.v_active, self.refresh_hz)
    }
}

fn parse_dtd(raw: &[u8]) -> Option<DetailedTiming> {
    if raw.len() < 18 || (raw[0] == 0 && raw[1] == 0) {
        return None; // text / non-timing descriptor
    }
    let clock10khz = u16::from_le_bytes([raw[0], raw[1]]) as u64;
    let h_active = raw[2] as u32 | ((raw[4] as u32 >> 4) << 8);
    let h_blank = raw[3] as u32 | ((raw[4] as u32 & 0x0f) << 8);
    let v_active = raw[5] as u32 | ((raw[7] as u32 >> 4) << 8);
    let v_blank = raw[6] as u32 | ((raw[7] as u32 & 0x0f) << 8);
    let h_total = (h_active + h_blank) as u64;
    let v_total = (v_active + v_blank) as u64;
    if h_total == 0 || v_total == 0 {
        return None;
    }
    let pixel_clock_hz = clock10khz * 10_000;
    Some(DetailedTiming {
        raw: raw[..18].try_into().unwrap(),
        pixel_clock_hz,
        h_active,
        h_blank,
        v_active,
        v_blank,
        refresh_hz: pixel_clock_hz as f64 / (h_total * v_total) as f64,
    })
}

impl Edid {
    /// All Detailed Timing Descriptors of the base block (order preserved).
    pub fn detailed_timings(&self) -> Vec<DetailedTiming> {
        (0..4).filter_map(|i| parse_dtd(&self.raw[54 + i * 18..54 + i * 18 + 18])).collect()
    }

    /// Convenience: stable id of this EDID.
    pub fn stable_id(&self) -> String {
        compute_stable_id(&self.raw).expect("already validated")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &[u8] = include_bytes!("../tests/fixtures/edp1-edid.bin");
    const SYNTH4K: &[u8] = include_bytes!("../tests/fixtures/synth-4k120-edid.bin");

    /// Minimal synthetic 128-byte EDID (builder mirrors F0's; kept for unit tests).
    fn synthetic() -> Vec<u8> {
        let mut d = vec![0u8; 128];
        d[0..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        d[8] = 0x4C;
        d[9] = 0x2D; // SAM
        d[10] = 0x22;
        d[11] = 0x07; // 0x0722
        d[16] = 10;
        d[17] = 32; // 2022
        d[54] = 0;
        d[55] = 0;
        d[57] = 0xFE;
        d[59..68].copy_from_slice(b"SYNTH-TV ");
        d
    }

    #[test]
    fn parses_real_256b_fixture() {
        assert_eq!(REAL.len(), 256); // bytes untouched since F0
        let e = parse_edid(REAL).unwrap();
        assert_eq!(e.pnp, "SDC");
        assert_eq!(e.product, 0x4161);
        assert_eq!((e.week, e.year), (46, 2020));
        assert_eq!(e.name, "ATNA56YX03-0");
        assert_eq!(e.raw.len(), 256);
    }

    #[test]
    fn real_dtd_matches_edid_decode() {
        // edid-decode: 1920x1080 @ 59.996714 Hz, 138.77 MHz.
        let e = parse_edid(REAL).unwrap();
        let dtds = e.detailed_timings();
        assert!(!dtds.is_empty());
        let d = &dtds[0];
        assert_eq!((d.h_active, d.v_active), (1920, 1080));
        assert_eq!(d.pixel_clock_hz, 138_770_000);
        assert!((d.refresh_hz - 59.996_714).abs() < 0.001, "got {}", d.refresh_hz);
        let m = d.as_mode().unwrap();
        assert_eq!((m.width, m.height), (1920, 1080));
        assert!((m.hz - 59.996_714).abs() < 0.001);
    }

    #[test]
    fn synth_4k120_fixture_dtds_are_exact() {
        // Documented synthetic panel: base DTDs carry 4K@60 + 1440p@120
        // (a base-block 4K120 DTD is impossible: 2-byte clock max 655.35 MHz).
        // Real 4K120 panels advertise VIC 118 in the CTA extension, which this
        // fixture includes (tag 0x02) but F1 does not interpret yet.
        let e = parse_edid(SYNTH4K).unwrap();
        assert_eq!(e.pnp, "SYN");
        assert_eq!(e.name, "SYNTH-4K120");
        assert_eq!(e.extension_blocks, 1);
        assert_eq!(e.raw.len(), 256);
        assert_eq!(SYNTH4K[128], 0x02); // CTA extension present, preserved raw
        let dtds = e.detailed_timings();
        assert_eq!(dtds.len(), 2);
        assert_eq!((dtds[0].h_active, dtds[0].v_active), (3840, 2160));
        assert!((dtds[0].refresh_hz - 60.0).abs() < 1e-9, "got {}", dtds[0].refresh_hz);
        assert_eq!((dtds[1].h_active, dtds[1].v_active), (2560, 1440));
        assert!((dtds[1].refresh_hz - 120.0).abs() < 1e-9, "got {}", dtds[1].refresh_hz);
    }

    #[test]
    fn synth_fixture_cta_carries_4k120_vic() {
        // The F1 synthetic panel: base DTDs cannot express 4K120, but the CTA
        // extension declares VIC 118 -> 3840x2160@120.000 nominal (Estimated).
        let e = parse_edid(SYNTH4K).unwrap();
        assert_eq!(e.extensions.len(), 1);
        let cta = match &e.extensions[0] {
            Extension::Cta861(c) => c,
            other => panic!("expected CTA, got {other:?}"),
        };
        assert!(cta.checksum_ok);
        assert!(cta.vics().contains(&(118, true)));
        let m = vic_to_mode(118).unwrap();
        assert_eq!((m.width, m.height), (3840, 2160));
        assert!((m.hz - 120.0).abs() < 1e-9);
    }

    #[test]
    fn unknown_extension_tag_is_preserved() {
        let mut d = parse_edid(REAL).unwrap().raw;
        // Append a fake DisplayID-tagged block (0x70) with valid checksum.
        let mut blk = vec![0x70u8; 128];
        blk[127] = ((256 - (blk[..127].iter().map(|x| *x as u32).sum::<u32>() % 256)) % 256) as u8;
        d.extend_from_slice(&blk);
        let e = parse_edid(&d).unwrap();
        assert!(matches!(&e.extensions[1], Extension::Unknown { tag: 0x70, .. }));
    }

    #[test]
    fn stable_id_format_and_sha256_basis() {
        let id = compute_stable_id(REAL).unwrap();
        assert!(id.starts_with("sdc-4161-"), "{id}");
        assert_eq!(id.len(), "sdc-4161-".len() + 8);
        // first-128-bytes basis: appending a block must not change the id
        assert_eq!(id, compute_stable_id(&REAL[..128]).unwrap());
        // differs from the F0 FNV placeholder by design
        assert_ne!(id, "sdc-4161-3b28cae1");
    }

    #[test]
    fn invalid_inputs_rejected() {
        assert!(matches!(parse_edid(&[0u8; 64]), Err(EdidError::TooShort(64))));
        let mut bad = synthetic();
        bad[1] = 0x00;
        assert!(matches!(parse_edid(&bad), Err(EdidError::BadHeader)));
    }

    #[test]
    fn synthetic_name_and_identity() {
        let e = parse_edid(&synthetic()).unwrap();
        assert_eq!(e.name, "SYNTH-TV");
        assert_eq!(e.stable_id().len(), "sam-0722-".len() + 8);
    }
}
