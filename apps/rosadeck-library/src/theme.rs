//! Retro terminal theme: palette, glyphs and color-capability detection.
//!
//! Two color depths, decided once at startup: 24-bit (kitty/alacritty/
//! foot/wezterm) for real cover art, or xterm-256 for everything else.
//! `--no-color` (or a non-color TERM) drops to plain ASCII so the layout
//! stays identical and legible in logs, pipes and screenshots.

/// Color capabilities of the current terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// 24-bit SGR (`38;2;r;g;b`).
    Truecolor,
    /// xterm-256 palette (6x6x6 cube + grayscale ramp).
    Ansi256,
    /// No styling at all.
    Plain,
}

/// Named styles used across the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Brand accent (magenta).
    Accent,
    /// Secondary (cyan).
    Cyan,
    /// Highlight (gold).
    Gold,
    /// Muted text.
    Dim,
    /// Normal text.
    Plain,
    /// Selected cell border.
    Border,
    /// Selected cell background (full reverse).
    Selected,
    /// Alerts: a missing emulator, an error that blocks playing.
    Alert,
}

/// Shorthand for an RGB triple.
pub type Rgb = rosadeck_tui_frame::Rgb;

/// The six colours the UI needs, no matter where they came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Brand accent (selection border, key legend).
    pub accent: Rgb,
    /// Secondary (platform chips, names).
    pub cyan: Rgb,
    /// Highlight (selected title, favourites).
    pub gold: Rgb,
    /// Body text.
    pub plain: Rgb,
    /// Muted text.
    pub dim: Rgb,
    /// Inactive borders and rules.
    pub border: Rgb,
    /// Backdrop the palette was built against.
    pub background: Rgb,
    /// Warnings/errors (missing emulator).
    pub alert: Rgb,
}

impl Palette {
    /// The built-in scheme: magenta / cyan / gold on a dark backdrop.
    pub fn builtin() -> Self {
        Self {
            accent: (255, 105, 180),
            cyan: (90, 220, 255),
            gold: (255, 205, 90),
            plain: (232, 234, 240),
            dim: (128, 132, 150),
            border: (90, 95, 115),
            background: (18, 18, 24),
            alert: (255, 92, 92),
        }
    }
}

/// Theme: colour depth plus the palette in use (built-in or pywal).
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// Color depth in use.
    pub depth: Depth,
    /// Draw cover images (needs color; disabled with `--ascii`).
    pub art: bool,
    /// The colours to paint with.
    pub palette: Palette,
    /// Where the palette came from.
    pub source: crate::pywal::Source,
    /// Wallpaper behind the palette (pywal only).
    pub wallpaper: String,
}

impl Theme {
    /// Detect capabilities from the environment (`NO_COLOR`, `COLORTERM`, `TERM`).
    pub fn from_env() -> Self {
        let no_color = std::env::var("NO_COLOR").is_ok();
        let depth = Self::detect_depth(
            no_color,
            std::env::var("COLORTERM").unwrap_or_default(),
            std::env::var("TERM").unwrap_or_default(),
        );
        Self::builtin(depth, depth != Depth::Plain)
    }

    /// Built-in palette with explicit capabilities.
    pub fn builtin(depth: Depth, art: bool) -> Self {
        Self {
            depth,
            art,
            palette: Palette::builtin(),
            source: crate::pywal::Source::Builtin,
            wallpaper: String::new(),
        }
    }

    /// Pure colour-depth detection (testable).
    pub fn detect_depth(no_color: bool, colorterm: String, term: String) -> Depth {
        if no_color {
            Depth::Plain
        } else if colorterm.contains("truecolor") || colorterm.contains("24bit") {
            Depth::Truecolor
        } else if ["256", "kitty", "alacritty", "foot", "wezterm", "truecolor", "xterm", "screen", "tmux", "linux"]
            .iter()
            .any(|k| term.contains(k))
        {
            Depth::Ansi256
        } else {
            Depth::Plain
        }
    }

    /// A theme with no styling and no art (headless validation).
    pub fn plain() -> Self {
        Self::builtin(Depth::Plain, false)
    }

    /// Wrap `text` in the escape for `style` (returned unchanged when plain).
    ///
    /// Styles resolve through the palette, so a pywal wallpaper repaints the
    /// whole UI without any other code changing.
    pub fn paint(&self, style: Style, text: &str) -> String {
        if self.depth == Depth::Plain {
            return text.to_owned();
        }
        let (fg, bold, reverse) = self.resolve(style);
        format!(
            "\x1b[0;{}{}{}m{text}\x1b[0m",
            if bold { "1;" } else { "" },
            if reverse { "7;" } else { "" },
            self.fg_escape(fg)
        )
    }

    /// Palette entry, weight and reversal for a style.
    fn resolve(&self, style: Style) -> (Rgb, bool, bool) {
        match style {
            Style::Accent => (self.palette.accent, true, false),
            Style::Cyan => (self.palette.cyan, false, false),
            Style::Gold => (self.palette.gold, true, false),
            Style::Dim => (self.palette.dim, false, false),
            Style::Plain => (self.palette.plain, false, false),
            Style::Border => (self.palette.border, false, false),
            Style::Selected => (self.palette.accent, true, true),
            Style::Alert => (self.palette.alert, true, false),
        }
    }

    fn fg_escape(&self, c: Rgb) -> String {
        match self.depth {
            Depth::Truecolor => format!("38;2;{};{};{}", c.0, c.1, c.2),
            _ => format!("38;5;{}", rgb_to_256(c.0, c.1, c.2)),
        }
    }

    /// Escape for an RGB foreground (`None` when plain).
    pub fn rgb(&self, r: u8, g: u8, b: u8) -> String {
        match self.depth {
            Depth::Plain => String::new(),
            Depth::Truecolor => format!("\x1b[38;2;{r};{g};{b}m"),
            Depth::Ansi256 => format!("\x1b[38;5;{}m", rgb_to_256(r, g, b)),
        }
    }

    /// Escape for an RGB background (`None` when plain).
    pub fn bg(&self, r: u8, g: u8, b: u8) -> String {
        match self.depth {
            Depth::Plain => String::new(),
            Depth::Truecolor => format!("\x1b[48;2;{r};{g};{b}m"),
            Depth::Ansi256 => format!("\x1b[48;5;{}m", rgb_to_256(r, g, b)),
        }
    }
}

/// Box-drawing glyphs (single width in every common terminal font).
pub mod glyph {
    /// Top-left / top-right / bottom-left / bottom-right.
    #[allow(dead_code)] // marco de tarjeta retirado (v23)
    pub const TL: &str = "╭";
    /// Top-left / top-right / bottom-left / bottom-right.
    #[allow(dead_code)] // marco de tarjeta retirado (v23)
    pub const TR: &str = "╮";
    /// Top-left / top-right / bottom-left / bottom-right.
    #[allow(dead_code)] // marco de tarjeta retirado (v23)
    pub const BL: &str = "╰";
    /// Top-left / top-right / bottom-left / bottom-right.
    #[allow(dead_code)] // marco de tarjeta retirado (v23)
    pub const BR: &str = "╯";
    /// Vertical / horizontal rules.
    #[allow(dead_code)] // sin marco y con iconos en `icons.rs` (v23)
    pub const V: &str = "│";
    /// Vertical / horizontal rules.
    pub const H: &str = "─";
    /// Upper half block (cover pixel pair).
    pub const HALF: &str = "▀";
    /// Filled block (plain-mode cover).
    pub const FULL: &str = "█";
    /// Unfilled star.
    #[allow(dead_code)] // sin marco y con iconos en `icons.rs` (v23)
    pub const STAR_OFF: &str = "☆";
}

/// Map RGB to the closest xterm-256 index (cube then grayscale ramp).
pub fn rgb_to_256(r: u8, g: u8, b: u8) -> u8 {
    let (r6, g6, b6) = (r as i32 * 5 / 255, g as i32 * 5 / 255, b as i32 * 5 / 255);
    // Distance from the cube corner: inside 12 => the 6x6x6 cube is closer.
    if (r as i32 - r6 * 51).abs() + (g as i32 - g6 * 51).abs() + (b as i32 - b6 * 51).abs() < 24 {
        return (16 + 36 * r6 + 6 * g6 + b6) as u8;
    }
    let gray = ((r as u16 + g as u16 + b as u16) / 3) as u8;
    if r.max(g).max(b) - r.min(g).min(b) < 12 {
        let idx = (gray as u16 * 23 / 255 + 8) as u8;
        return 232u8 + idx.min(23);
    }
    (16 + 36 * r6 + 6 * g6 + b6) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_detection() {
        let d = Theme::detect_depth;
        assert_eq!(d(true, "truecolor".into(), "xterm-kitty".into()), Depth::Plain);
        assert_eq!(d(false, "truecolor".into(), "xterm".into()), Depth::Truecolor);
        assert_eq!(d(false, String::new(), "xterm-256color".into()), Depth::Ansi256);
        assert_eq!(d(false, String::new(), "dumb".into()), Depth::Plain);
        // Art needs colour; the plain theme has neither.
        assert!(!Theme::builtin(d(false, String::new(), "dumb".into()), false).art);
        assert!(Theme::builtin(Depth::Truecolor, true).art);
    }

    #[test]
    fn paint_wraps_and_resets() {
        let t = Theme::builtin(Depth::Truecolor, true);
        let painted = t.paint(Style::Accent, "hi");
        assert!(painted.starts_with("\x1b[0;1;38;2;255;105;180m"), "{painted:?}");
        assert!(painted.ends_with("\x1b[0m"));
        assert_eq!(Theme::plain().paint(Style::Accent, "hi"), "hi");
        assert!(t.paint(Style::Selected, "x").starts_with("\x1b[0;1;7;38;2;"), "selection reverses");
    }

    #[test]
    fn palette_changes_the_escape_bytes() {
        // Same style, different palette => different bytes on the wire.
        let mut t = Theme::builtin(Depth::Truecolor, true);
        let before = t.paint(Style::Cyan, "x");
        t.palette.cyan = (1, 2, 3);
        let after = t.paint(Style::Cyan, "x");
        assert!(after.contains("38;2;1;2;3"), "{after:?}");
        assert_ne!(before, after);
    }

    #[test]
    fn ansi256_depth_quantises_the_palette() {
        let t = Theme::builtin(Depth::Ansi256, false);
        let painted = t.paint(Style::Gold, "x");
        assert!(painted.starts_with("\x1b[0;1;38;5;"), "{painted:?}");
        assert!(!painted.contains("38;2;"), "no truecolor at 256 depth");
    }

    #[test]
    fn rgb_maps_to_cube_or_gray() {
        assert_eq!(rgb_to_256(0, 0, 0), 16);
        assert_eq!(rgb_to_256(255, 255, 255), 231);
        assert!(rgb_to_256(12, 14, 20) > 232, "dark neutral prefers the gray ramp");
        assert_eq!(rgb_to_256(255, 0, 0), 196);
    }

    #[test]
    fn glyphs_are_single_width() {
        // Half blocks and box drawing must be 1 column; a 2-wide glyph would
        // shear every grid cell.
        for g in [glyph::TL, glyph::TR, glyph::BL, glyph::BR, glyph::V, glyph::H, glyph::HALF, glyph::FULL] {
            assert_eq!(rosadeck_tui_frame::visible_len(g), 1, "{g:?} is not single width");
        }
    }
}