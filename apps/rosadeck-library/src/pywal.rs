//! Pywal integration: the UI takes its colours from your wallpaper.
//!
//! Convention (no config edits, no templates): pywal writes
//! `$XDG_CACHE_HOME/wal/colors.json`. Both layouts are supported — the modern
//! nested `{"colors": {"color0": …}}` and the legacy flat `{"colors0": …}`.
//!
//! The UI never trusts the palette blindly: pywal keeps `color1`…`color6` dim
//! on purpose (they are meant as a *background*), so text roles pick the
//! brightest usable variant and every role is checked against WCAG contrast
//! against the wallpaper background. Read-only: nothing here writes to the
//! cache or runs pywal.

use crate::theme::{Depth, Palette, Theme};
use rosadeck_tui_frame::{blend, contrast, parse_hex, Rgb};
use std::path::{Path, PathBuf};

/// Where the palette came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Colours generated from the wallpaper (pywal).
    Pywal,
    /// The built-in magenta/cyan/gold scheme.
    Builtin,
}

impl Source {
    /// Short label for the status line.
    pub fn label(self) -> &'static str {
        match self {
            Source::Pywal => "pywal",
            Source::Builtin => "builtin",
        }
    }
}

/// A loaded palette plus the wallpaper it came from.
#[derive(Debug, Clone)]
pub struct Wal {
    /// 16 pywal colours (index 0..15) in RGB.
    pub colors: [Rgb; 16],
    /// `special.background` / `special.foreground` when present.
    pub background: Rgb,
    /// Foreground colour from `special`.
    pub foreground: Rgb,
    /// Wallpaper path recorded by pywal.
    pub wallpaper: String,
    /// mtime (unix seconds) of `colors.json`, used to detect a new wallpaper.
    pub stamp: u64,
}

/// Default cache location (`$XDG_CACHE_HOME/wal/colors.json`).
pub fn colors_path() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())).join(".cache"));
    base.join("wal/colors.json")
}

/// Read `path` into a [`Wal`] (pure; `None` when absent or unusable).
pub fn load_from(path: &Path) -> Option<Wal> {
    let text = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let mut colors = [(0u8, 0u8, 0u8); 16];
    for i in 0..16 {
        // Modern: colors.colorN. Legacy: colorsN.
        let hex = json
            .get("colors")
            .and_then(|c| c.get(format!("color{i}")))
            .or_else(|| json.get(format!("colors{i}")))
            .and_then(serde_json::Value::as_str)?;
        colors[i] = parse_hex(hex)?;
    }
    let special = |k: &str| json.get("special").and_then(|s| s.get(k)).and_then(serde_json::Value::as_str).and_then(parse_hex);
    let stamp = std::fs::metadata(path).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    Some(Wal {
        colors,
        background: special("background").unwrap_or(colors[0]),
        foreground: special("foreground").unwrap_or(colors[7]),
        wallpaper: json.get("wallpaper").and_then(serde_json::Value::as_str).unwrap_or_default().to_owned(),
        stamp,
    })
}

/// Read the default pywal cache location.
pub fn load() -> Option<Wal> {
    load_from(&colors_path())
}

/// Which variant of each pywal hue is bright enough to read on `background`.
///
/// pywal's `color1`…`color6` are deliberately dark (they are window
/// backgrounds), so we walk the bright half (`color8`…`color15`) and fall
/// back to the plain colour if a wallpaper palette has nothing usable.
fn readable(bright: &[Rgb], plain: Rgb, background: Rgb, min: f64) -> Rgb {
    let mut best = plain;
    for &c in bright {
        if contrast(c, background) >= min {
            return c;
        }
        if contrast(c, background) > contrast(best, background) {
            best = c;
        }
    }
    best
}

/// Build a theme from a pywal palette (readable roles, never worse contrast).
pub fn theme_from(wal: &Wal) -> Theme {
    let bg = wal.background;
    let [c0, c1, c2, c3, c4, c5, c6, c7, c8, c9, c10, c11, c12, c13, c14, c15] = wal.colors;
    let accent = readable(&[c5, c13, c12, c3, c11, c14, c15], c15, bg, 3.0);
    let cyan = readable(&[c6, c14, c13, c5, c15], c14, bg, 3.0);
    let gold = readable(&[c3, c11, c4, c12, c2, c15], c15, bg, 3.0);
    let plain = readable(&[c7, c15, c8, c14], wal.foreground, bg, 4.5);
    let dim = blend(bg, plain, 0.62);
    let border = blend(bg, plain, 0.45);
    // Red must stay readable on pywal backgrounds; fall back through the
    // bright/bright-red slots, never through a colour below 4.5:1 when a
    // readable one exists.
    let alert = readable(&[c9, c1, c13, c5, c15], c15, bg, 4.5);
    let _ = (c0, c2, c8, c10, c11);
    Theme {
        depth: Depth::Truecolor,
        art: true,
        palette: Palette { accent, cyan, gold, plain, dim, border, background: bg, alert },
        source: Source::Pywal,
        wallpaper: wal.wallpaper.clone(),
    }
}

/// Theme policy requested on the command line / environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Use pywal when available, otherwise the built-in palette.
    Auto,
    /// Force pywal (fall back to built-in if the cache is missing).
    Pywal,
    /// Ignore pywal.
    System,
}

/// Resolve a [`Theme`] for the given mode and terminal capabilities.
pub fn resolve(mode: Mode, depth: Depth, art: bool) -> (Theme, Option<String>) {
    let builtin = Theme::builtin(depth, art);
    if mode == Mode::System {
        return (builtin, None);
    }
    match load() {
        Some(wal) => {
            let mut t = theme_from(&wal);
            if t.depth == Depth::Plain {
                t = Theme::builtin(depth, art);
            }
            let note = Some(format!("pywal · {}", wal.wallpaper.rsplit('/').next().unwrap_or("wallpaper")));
            (t, note)
        }
        None => {
            let note = (mode == Mode::Pywal).then(|| "pywal not found (colors.json) — built-in palette".to_owned());
            (builtin, note)
        }
    }
}

/// True when the cached palette is newer than `previous` (wallpaper changed).
pub fn changed_since(previous: u64) -> bool {
    load().map(|w| w.stamp > previous).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A realistic dark pywal output (nested layout, like the real file).
    const NESTED: &str = r##"{
        "wallpaper": "/home/u/wall.jpg",
        "alpha": "100",
        "special": {"background": "#0b1b0b", "foreground": "#c2c6c2", "cursor": "#c2c6c2"},
        "colors": {
            "color0": "#0b1b0b", "color1": "#2A5148", "color2": "#2F6A61", "color3": "#4B5141",
            "color4": "#AD773C", "color5": "#57A19F", "color6": "#5ED8D8", "color7": "#c2c6c2",
            "color8": "#5a6f5a", "color9": "#2A5148", "color10": "#2F6A61", "color11": "#4B5141",
            "color12": "#AD773C", "color13": "#57A19F", "color14": "#5ED8D8", "color15": "#c2c6c2"
        }
    }"##;

    /// Legacy flat layout (pywal <= 3.3).
    const FLAT: &str = r##"{"colors0":"#101010","colors1":"#202020","colors2":"#303030","colors3":"#ff0055",
        "colors4":"#00aaff","colors5":"#00ffaa","colors6":"#ffcc00","colors7":"#dddddd",
        "colors8":"#505050","colors9":"#ff5577","colors10":"#55bbff","colors11":"#55ffbb",
        "colors12":"#ffdd55","colors13":"#ff5577","colors14":"#55ffff","colors15":"#ffffff"}"##;

    fn write_temp(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("rosadeck-wal-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn parses_both_pywal_layouts() {
        let nested = load_from(&write_temp("colors.json", NESTED)).unwrap();
        assert_eq!(nested.background, (11, 27, 11));
        assert_eq!(nested.foreground, (194, 198, 194));
        assert_eq!(nested.colors[6], (94, 216, 216));
        assert_eq!(nested.wallpaper, "/home/u/wall.jpg");
        assert!(nested.stamp > 0);

        let flat = load_from(&write_temp("flat.json", FLAT)).unwrap();
        assert_eq!(flat.colors[3], (255, 0, 85), "legacy flat keys");
        assert_eq!(flat.colors[15], (255, 255, 255));
        assert_eq!(flat.background, (16, 16, 16), "falls back to color0 without special");
    }

    #[test]
    fn unreadable_or_broken_files_are_none() {
        assert!(load_from(Path::new("/nonexistent/colors.json")).is_none());
        assert!(load_from(&write_temp("bad.json", "{not json")).is_none());
        assert!(load_from(&write_temp("short.json", r##"{"colors":{"color0":"#fff"}}"##)).is_none());
        assert!(load_from(&write_temp("invalid.json", r##"{"colors0":"nope"}"##)).is_none());
    }

    #[test]
    fn theme_roles_are_readable_against_a_dark_wallpaper() {
        let wal = load_from(&write_temp("theme.json", NESTED)).unwrap();
        let t = theme_from(&wal);
        let bg = wal.background;
        // Text roles must be readable; decorative ones only need to be visible.
        for (name, c, min) in [
            ("accent", t.palette.accent, 3.0),
            ("cyan", t.palette.cyan, 3.0),
            ("gold", t.palette.gold, 3.0),
            ("plain", t.palette.plain, 4.5),
            ("dim", t.palette.dim, 2.5),
            ("border", t.palette.border, 1.5),
        ] {
            assert!(contrast(c, bg) >= min, "{name} {c:?} contrast {} < {min}", contrast(c, bg));
        }
        // Text must be clearly brighter than the background (pywal keeps
        // colors1..6 dark on purpose).
        assert!(t.palette.plain != bg);
    }

    #[test]
    fn a_washed_out_palette_still_yields_readable_text() {
        // Every colour nearly the same grey: only `special.foreground` can save
        // legibility, so the fallback path has to exist.
        let mut colors = String::new();
        for i in 0..16 {
            if i > 0 {
                colors.push(',');
            }
            colors.push_str(&format!("\"color{i}\":\"#808080\""));
        }
        let json = format!("{{\"special\":{{\"background\":\"#808080\",\"foreground\":\"#ffffff\"}},\"colors\":{{{colors}}}}}");
        let wal = load_from(&write_temp("grey.json", &json)).unwrap();
        let t = theme_from(&wal);
        assert!(contrast(t.palette.plain, wal.background) > 3.0, "plain text must stay readable");
    }

    #[test]
    fn resolve_modes_behave() {
        let (t, _) = resolve(Mode::System, Depth::Truecolor, true);
        assert_eq!(t.source, Source::Builtin);
        // Auto with no cache in the test env still returns a usable theme.
        let (t, _) = resolve(Mode::Auto, Depth::Truecolor, true);
        assert!(t.depth == Depth::Truecolor);
        assert!(t.art);
    }

    #[test]
    fn changed_since_detects_a_new_wallpaper() {
        assert!(!changed_since(u64::MAX), "a future stamp cannot be exceeded");
        if let Some(wal) = load() {
            assert!(!changed_since(wal.stamp), "the same palette is not a change");
            assert!(changed_since(0), "an older stamp means pywal re-ran");
        }
    }
}