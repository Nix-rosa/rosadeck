//! F0 prototype 3: minimal manual EDID parser, zero dependencies.
use std::fs;

#[derive(Debug)]
struct EdidInfo {
    pnp: String,
    product: u16,
    serial: u32,
    week: u8,
    year: u16,
    name: String,
    blocks: usize,
    bytes: usize,
    stable_id: String,
}

fn pnp(bytes: &[u8]) -> String {
    let raw = ((bytes[8] as u16) << 8) | bytes[9] as u16;
    [(raw >> 10) & 0x1f, (raw >> 5) & 0x1f, raw & 0x1f]
        .iter()
        .map(|v| ((*v as u8) + b'A' - 1) as char)
        .collect()
}

fn fnv1a_hex(data: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")[..8].to_owned()
}

fn monitor_name(data: &[u8]) -> String {
    for i in 0..4 {
        let off = 54 + i * 18;
        // descriptor de texto: primeros 3 bytes cero + tag 0xFE (nombre) o 0xFF (serie)
        if data[off] == 0 && data[off + 1] == 0 && (data[off + 3] == 0xFE || data[off + 3] == 0xFC) {
            let raw = &data[off + 5..off + 18];
            let s: String = raw.iter().take_while(|&&b| b != 0x0A && b != 0).map(|&b| b as char).collect();
            let s = s.trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    String::new()
}

fn parse_edid(data: &[u8]) -> Result<EdidInfo, String> {
    if data.len() < 128 {
        return Err(format!("too short: {} bytes", data.len()));
    }
    if data[0..8] != [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00] {
        return Err("bad EDID header".into());
    }
    // Aceptamos >=128 y reportamos bloques reales sin fallar.
    let blocks = data.len() / 128;
    let name = monitor_name(data);
    let stable = format!("{}-{:04x}-{}", pnp(data).to_lowercase(), u16::from_le_bytes([data[10], data[11]]), fnv1a_hex(&data[..128]));
    Ok(EdidInfo {
        pnp: pnp(data),
        product: u16::from_le_bytes([data[10], data[11]]),
        serial: u32::from_le_bytes([data[12], data[13], data[14], data[15]]),
        week: data[16],
        year: 1990 + data[17] as u16,
        name,
        blocks,
        bytes: data.len(),
        stable_id: stable,
    })
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or("/sys/class/drm/card1-eDP-1/edid".into());
    let data = fs::read(&path).unwrap_or_else(|e| {
        eprintln!("ERROR reading {path}: {e}");
        std::process::exit(1);
    });
    if data.is_empty() {
        println!("Connector EDID: absent (0 bytes, disconnected)");
        return;
    }
    match parse_edid(&data) {
        Ok(i) => {
            println!("EDID ({path})");
            println!("  Manufacturer : {}", i.pnp);
            println!("  Product      : 0x{:04x}", i.product);
            println!("  Serial       : {}", i.serial);
            println!("  Name         : {}", if i.name.is_empty() { "(none)" } else { &i.name });
            println!("  Week         : {}", i.week);
            println!("  Year         : {}", i.year);
            println!("  Blocks       : {} (byte126 anuncia ext adicionales)", i.blocks);
            println!("  Bytes        : {}", i.bytes);
            println!("  Stable ID    : {}", i.stable_id);
        }
        Err(e) => {
            eprintln!("ERROR: {e}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const REAL: &[u8] = include_bytes!("../../edid-probe/tests/fixtures/edp1-edid.bin");

    fn synthetic() -> Vec<u8> {
        let mut d = vec![0u8; 128];
        d[0..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        d[8] = 0x4C;
        d[9] = 0x2D; // "SAM"
        d[10] = 0x22;
        d[11] = 0x07; // producto 0x0722
        d[16] = 10;
        d[17] = 32; // 2022
        d[126] = 0;
        d[54] = 0;
        d[55] = 0;
        d[57] = 0xFE; // tag nombre en offset+3 -> data[57]
        d[57..58].copy_from_slice(&[0xFE]);
        d[59..68].copy_from_slice(b"SYNTH-TV ");
        d
    }

    #[test]
    fn parses_real_edid_fixture() {
        assert_eq!(REAL.len(), 256);
        let i = parse_edid(REAL).unwrap();
        assert_eq!(i.pnp, "SDC");
        assert_eq!(i.product, 0x4161);
        assert_eq!((i.week, i.year), (46, 2020));
        assert_eq!(i.name, "ATNA56YX03-0");
        assert_eq!(i.bytes, 256);
    }

    #[test]
    fn rejects_bad_header() {
        let mut d = synthetic();
        d[1] = 0x00;
        assert!(parse_edid(&d).is_err());
    }

    #[test]
    fn rejects_truncated() {
        assert!(parse_edid(&[0u8; 64]).is_err());
    }

    #[test]
    fn stable_id_is_deterministic() {
        let a = parse_edid(&synthetic()).unwrap().stable_id;
        let b = parse_edid(&synthetic()).unwrap().stable_id;
        assert_eq!(a, b);
        assert!(a.starts_with("sam-0722-"));
    }

    #[test]
    fn synthetic_name_extraction() {
        assert_eq!(parse_edid(&synthetic()).unwrap().name, "SYNTH-TV");
    }
}
