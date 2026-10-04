//! CTA-861 extension parsing (base for F2; HDR/audio blocks preserved, not interpreted).
//!
//! Observed (not assumed): real high-refresh panels (e.g. 4K120) cannot fit
//! their timing in a 16-bit base-block DTD (max 655.35 MHz pixel clock) and
//! advertise it via CTA Video Data Block VICs instead. Unknown blocks are
//! preserved, never failed.

use crate::EdidError;

/// CTA data-block tags (header byte bits 7–5).
pub const TAG_AUDIO: u8 = 1;
/// Video Data Block: Short Video Descriptors (VIC + native flag).
pub const TAG_VIDEO: u8 = 2;
pub const TAG_VENDOR: u8 = 3;
pub const TAG_SPEAKER: u8 = 4;
/// Extended tag block (payload[0] = extended tag).
pub const TAG_EXTENDED: u8 = 7;

/// Extended tag: Video Capability Data Block.
pub const EXT_VIDEO_CAPS: u8 = 0x00;
/// Extended tag: YCbCr 4:2:0 Video Data Block (also carries VICs).
pub const EXT_YCBCR420_VDB: u8 = 0x0E;

/// One parsed CTA data block: tag + payload preserved verbatim.
#[derive(Debug, Clone, PartialEq)]
pub struct CtaDataBlock {
    /// Block tag (1–7).
    pub tag: u8,
    /// Extended tag for [`TAG_EXTENDED`] blocks.
    pub ext_tag: Option<u8>,
    /// Payload bytes (excluding header / extended-tag byte).
    pub payload: Vec<u8>,
}

impl CtaDataBlock {
    /// SVD bytes when this is a Video Data Block or YCbCr420 VDB.
    pub fn video_svds(&self) -> Option<&[u8]> {
        if self.tag == TAG_VIDEO {
            Some(&self.payload)
        } else if self.tag == TAG_EXTENDED && self.ext_tag == Some(EXT_YCBCR420_VDB) {
            Some(&self.payload)
        } else {
            None
        }
    }
}

/// Parsed CTA-861 extension block (raw always preserved).
#[derive(Debug, Clone, PartialEq)]
pub struct CtaExtension {
    /// CTA revision byte.
    pub revision: u8,
    /// Data blocks in order (unknown tags preserved as-is).
    pub blocks: Vec<CtaDataBlock>,
    /// Block checksum valid.
    pub checksum_ok: bool,
    /// Original 128 bytes.
    pub raw: Vec<u8>,
}

impl CtaExtension {
    /// All `(VIC, native)` pairs from Video + YCbCr420 VDBs, in order.
    /// SVD `0x00` is padding and is skipped (documented).
    pub fn vics(&self) -> Vec<(u8, bool)> {
        let mut out = Vec::new();
        for b in &self.blocks {
            if let Some(svds) = b.video_svds() {
                for s in svds {
                    if *s == 0 {
                        continue;
                    }
                    out.push((s & 0x7f, s & 0x80 != 0));
                }
            }
        }
        out
    }
}

/// Parse one 128-byte extension block known to carry tag `0x02` (CTA).
pub fn parse_cta(block: &[u8]) -> Result<CtaExtension, EdidError> {
    if block.len() != 128 {
        return Err(EdidError::BadBlockSize(block.len()));
    }
    let dtd_offset = block[2];
    if dtd_offset < 4 || dtd_offset > 127 {
        return Err(EdidError::InvalidCta(format!("dtd_offset {dtd_offset} out of range 4..=127")));
    }
    let mut blocks = Vec::new();
    let mut pos = 4usize;
    while pos < dtd_offset as usize {
        let header = block[pos];
        let tag = (header >> 5) & 0x07;
        let len = (header & 0x1f) as usize;
        if tag == 0 || tag > 7 {
            return Err(EdidError::InvalidCta(format!("reserved data-block tag {tag}")));
        }
        if pos + 1 + len > dtd_offset as usize {
            return Err(EdidError::TruncatedCta { tag, len, available: dtd_offset as usize - pos - 1 });
        }
        let payload = &block[pos + 1..pos + 1 + len];
        if tag == TAG_EXTENDED {
            if payload.is_empty() {
                return Err(EdidError::TruncatedCta { tag, len, available: 0 });
            }
            blocks.push(CtaDataBlock { tag, ext_tag: Some(payload[0]), payload: payload[1..].to_vec() });
        } else {
            blocks.push(CtaDataBlock { tag, ext_tag: None, payload: payload.to_vec() });
        }
        pos += 1 + len;
    }
    let checksum_ok = block.iter().fold(0u32, |a, b| a + *b as u32) % 256 == 0;
    Ok(CtaExtension { revision: block[1], blocks, checksum_ok, raw: block.to_vec() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal CTA block builder: tag 0x02, rev, trailing checksum fixed.
    fn cta_block(rev: u8, collection: &[u8]) -> Vec<u8> {
        let mut b = vec![0u8; 128];
        b[0] = 0x02;
        b[1] = rev;
        b[2] = (4 + collection.len()) as u8;
        b[4..4 + collection.len()].copy_from_slice(collection);
        let sum: u32 = b[..127].iter().map(|x| *x as u32).sum();
        b[127] = ((256 - (sum % 256)) % 256) as u8;
        b
    }

    #[test]
    fn parses_video_block_vics_with_native_flag() {
        // VDB len 2: SVD 0x90 (VIC 16 native), 0x76 (VIC 118 non-native).
        let b = cta_block(3, &[0x42, 0x90, 0x76]);
        let cta = parse_cta(&b).unwrap();
        assert_eq!(cta.revision, 3);
        assert!(cta.checksum_ok);
        assert_eq!(cta.vics(), vec![(16, true), (118, false)]);
    }

    #[test]
    fn preserves_unknown_blocks_without_failing() {
        // Vendor block (tag 3, len 3) + audio block (tag 1, len 1).
        let b = cta_block(3, &[0x63, 0x03, 0x0C, 0x00, 0x21, 0x09]);
        let cta = parse_cta(&b).unwrap();
        assert_eq!(cta.blocks.len(), 2);
        assert_eq!(cta.blocks[0].tag, TAG_VENDOR);
        assert_eq!(cta.blocks[0].payload, vec![0x03, 0x0C, 0x00]);
        assert!(cta.vics().is_empty());
    }

    #[test]
    fn parses_extended_420_vdb_as_vics() {
        // Extended block, ext tag 0x0E, one SVD.
        let b = cta_block(3, &[0xE2, 0x0E, 0x76]);
        let cta = parse_cta(&b).unwrap();
        assert_eq!(cta.blocks[0].ext_tag, Some(EXT_YCBCR420_VDB));
        assert_eq!(cta.vics(), vec![(118, false)]);
    }

    #[test]
    fn rejects_truncated_collection() {
        let mut b = cta_block(3, &[0x42, 0x90, 0x76]);
        b[2] = 5; // dtd_offset cuts the VDB payload short
        assert!(matches!(parse_cta(&b), Err(EdidError::TruncatedCta { .. })));
    }

    #[test]
    fn rejects_bad_dtd_offset_and_size() {
        let mut b = cta_block(3, &[]);
        b[2] = 2;
        assert!(matches!(parse_cta(&b), Err(EdidError::InvalidCta(_))));
        assert!(matches!(parse_cta(&b[..64]), Err(EdidError::BadBlockSize(64))));
    }
}
