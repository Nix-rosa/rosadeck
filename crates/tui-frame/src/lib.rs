//! Raw-mode-safe frame writer shared by every Rosadeck TUI.
//!
//! Why this exists (verified on a pty, 2026-10-02): `crossterm`'s
//! `enable_raw_mode()` calls `cfmakeraw()`, which clears `OPOST`. In raw
//! mode `\n` is **LF only — no carriage return**. Writing a frame with
//! `writeln!` therefore leaves the cursor at the end of the previous line:
//! every row starts further right than the last one (stair-stepping text
//! that overlaps and looks cut). Reproduced byte-for-byte:
//! `b"...query: \n\xe2\x94\x80..."` with `CRLF pairs: 0, bare LF: 16`.
//!
//! Two rules, both enforced here so no TUI can get them wrong:
//! 1. every newline is written as CRLF;
//! 2. no line fills the last column — terminals with deferred wrap skip a
//!    row when the next character arrives after a full-width write.
//!
//! Pure: [`frame_bytes`] does the whole transform with no IO, so tests can
//! assert the byte stream without a terminal.

pub mod vt;

use std::io::Write;

/// Escape sequence introducer (`CSI`, `OSC`, `SS2/SS3`).
const ESC: char = '\x1b';

/// A pywal palette as an RGB triple.
pub type Rgb = (u8, u8, u8);

/// Parse a pywal colour (`#rrggbb`, `rrggbb`, `#rgb`) into RGB.
///
/// Returns `None` for anything else, so a malformed theme never breaks the UI:
/// the caller falls back to the built-in palette.
pub fn parse_hex(hex: &str) -> Option<Rgb> {
    let h = hex.trim().trim_start_matches('#');
    match h.len() {
        3 => {
            let d = |i: usize| u8::from_str_radix(&h[i..i + 1], 16).ok().map(|v| v * 17);
            Some((d(0)?, d(1)?, d(2)?))
        }
        6 => {
            let pair = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
            Some((pair(0)?, pair(2)?, pair(4)?))
        }
        _ => None,
    }
}

/// Linear blend between two colours (`t` = 0 keeps `a`).
pub fn blend(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    (
        (a.0 as f64 + (b.0 as f64 - a.0 as f64) * t).round() as u8,
        (a.1 as f64 + (b.1 as f64 - a.1 as f64) * t).round() as u8,
        (a.2 as f64 + (b.2 as f64 - a.2 as f64) * t).round() as u8,
    )
}

/// Relative luminance (WCAG) — used to keep text readable on any wallpaper.
pub fn luminance(c: Rgb) -> f64 {
    let f = |v: u8| {
        let v = v as f64 / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * f(c.0) + 0.7152 * f(c.1) + 0.0722 * f(c.2)
}

/// Contrast ratio between two colours (1.0 … 21.0).
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    if la > lb {
        (la + 0.05) / (lb + 0.05)
    } else {
        (lb + 0.05) / (la + 0.05)
    }
}

/// True when `c` starts an escape sequence.
fn starts_escape(c: char) -> bool {
    c == ESC || c == '\u{9b}'
}

/// Length of the escape sequence at the start of `s`, or `0` when `s` does
/// not start with one. Handles CSI (`ESC [ … final`), OSC (`ESC ] … BEL/ST`)
/// and two-character escapes (`ESC 7`, SS3 `ESC O A`).
fn escape_len(s: &str) -> usize {
    let mut it = s.char_indices();
    let Some((_, first)) = it.next() else { return 0 };
    if !starts_escape(first) {
        return 0;
    }
    if first == '\u{9b}' {
        // CSI without ESC: parameter/intermediate bytes then a final byte.
        for (i, c) in s.char_indices().skip(1) {
            if ('\x40'..='\x7e').contains(&c) {
                return i + c.len_utf8();
            }
        }
        return s.len(); // incomplete: swallow the rest
    }
    let rest = &s[first.len_utf8()..];
    let Some(kind) = rest.chars().next() else { return s.len() };
    match kind {
        '[' => {
            for (i, c) in rest.char_indices().skip(1) {
                if ('\x40'..='\x7e').contains(&c) {
                    return first.len_utf8() + i + c.len_utf8();
                }
            }
            s.len()
        }
        ']' => {
            for (i, c) in rest.char_indices().skip(1) {
                if c == '\x07' {
                    return first.len_utf8() + i + 1;
                }
                if c == ESC && rest[i + 1..].starts_with('\\') {
                    return first.len_utf8() + i + 2;
                }
            }
            s.len()
        }
        _ => first.len_utf8() + kind.len_utf8(),
    }
}

/// Visible column count of `s` (escape sequences and their payloads are free).
///
/// Half-block glyphs (`▀`) and box drawing count as one column each; CJK or
/// emoji would count as two in most terminals — callers pass ASCII/borders.
pub fn visible_len(s: &str) -> usize {
    let mut n = 0;
    let mut rest = s;
    while !rest.is_empty() {
        let e = escape_len(rest);
        if e > 0 {
            rest = &rest[e..];
        } else {
            let c = rest.chars().next().unwrap();
            n += 1;
            rest = &rest[c.len_utf8()..];
        }
    }
    n
}

/// Cut `s` to `max` visible columns, keeping escapes intact and closing any
/// open styling with a reset so the rest of the frame is not recoloured.
pub fn truncate_visible(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut n = 0;
    let mut rest = s;
    let mut styled = false;
    let mut cut = false;
    while !rest.is_empty() {
        let e = escape_len(rest);
        if e > 0 {
            out.push_str(&rest[..e]);
            if rest.len() > 1 && rest.as_bytes()[1] == b'[' && !rest.starts_with("\x1b[0m") {
                styled = true;
            }
            rest = &rest[e..];
            continue;
        }
        if n >= max {
            cut = true;
            break;
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        n += 1;
        rest = &rest[c.len_utf8()..];
    }
    // A truncated line must not leak style into the next row. Lines that fit
    // are returned untouched (they carry their own reset already).
    if cut && styled {
        out.push_str("\x1b[0m");
    }
    out
}

/// Width assumed when the terminal reports nothing usable (0 columns:
/// pty not sized yet, launched from a non-interactive shell).
pub const FALLBACK_WIDTH: u16 = 80;

/// Maximum printable columns for a frame (one less than the terminal width).
///
/// Below 20 columns the report is treated as bogus — otherwise a 0-column
/// report truncates every line to a single character.
pub fn safe_width(width: u16) -> usize {
    let w = if width < 20 { FALLBACK_WIDTH } else { width };
    (w as usize) - 1
}

/// Encode a `\n`-joined frame as raw-mode-safe terminal bytes.
///
/// Line endings become CRLF, no line exceeds [`safe_width`], and the last
/// line carries no trailing newline (so a full-height frame never scrolls).
pub fn frame_bytes(frame: &str, width: u16) -> Vec<u8> {
    let max = safe_width(width);
    let mut out = Vec::with_capacity(frame.len() + frame.len() / 8);
    let lines: Vec<&str> = frame.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(truncate_visible(line, max).as_bytes());
    }
    out
}

/// Write a frame to `out` in raw mode (CRLF, width-capped).
pub fn write_frame<W: Write>(out: &mut W, frame: &str, width: u16) -> std::io::Result<()> {
    out.write_all(&frame_bytes(frame, width))
}

/// Synchronized-output mode (DEC 2026): the terminal buffers everything and
/// presents it in one go. Supported by kitty, foot and WezTerm; ignored by the
/// rest, but we only emit it when we know it is implemented.
pub fn supports_synchronized_output(term: &str) -> bool {
    term.contains("kitty") || term.contains("foot") || term.contains("wezterm")
}

/// Begin/end a synchronized frame (`CSI ? 2026 h` / `l`).
pub const SYNC_BEGIN: &[u8] = b"\x1b[?2026h";
/// Begin/end a synchronized frame (`CSI ? 2026 h` / `l`).
pub const SYNC_END: &[u8] = b"\x1b[?2026l";

/// One token of a styled line: an escape sequence or a visible character.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Token {
    /// Escape sequence (never occupies a column).
    escape: Option<String>,
    /// Visible character (always exactly one column).
    ch: Option<char>,
    /// Column of the next visible character (for escapes: where they apply).
    col: usize,
}

/// Split a styled line into tokens with their column positions.
fn tokens(s: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut col = 0usize;
    let mut rest = s;
    while !rest.is_empty() {
        let e = escape_len(rest);
        if e > 0 {
            out.push(Token { escape: Some(rest[..e].to_owned()), ch: None, col });
            rest = &rest[e..];
        } else {
            let c = rest.chars().next().unwrap();
            out.push(Token { escape: None, ch: Some(c), col });
            col += 1;
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// Expose the tokenizer for tests and diagnostics.
pub fn tokens_for_tests(line: &str) -> Vec<TokenView> {
    tokens(line).into_iter().map(|t| TokenView { col: t.col, text: t.escape.clone().unwrap_or_else(|| t.ch.map(|c| c.to_string()).unwrap_or_default()) }).collect()
}

/// One token, with its column and text (escape sequences included).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenView {
    /// Column the token applies to.
    pub col: usize,
    /// Escape text or the character.
    pub text: String,
}

/// Minimal patch that turns the rendered row `old` into `new`.
///
/// Returns `(column, styled_slice)`: write `styled_slice` starting at `column`
/// and the row matches `new` again — no need to rewrite the rest of a row that
/// is 190 columns of cover art wide. `None` means "row already identical".
///
/// Safety rules that keep the terminal consistent:
/// * the slice starts with a reset, so stale styling cannot leak in;
/// * if any escape was emitted, a reset closes it (the untouched tail keeps
///   whatever it had);
/// * if the visible width changed, the whole row is returned instead.
pub fn row_patch(old: &str, new: &str) -> Option<(usize, String)> {
    if old == new {
        return None;
    }
    let new_tokens = tokens(new);
    let old_tokens = tokens(old);

    // One pass over the columns, comparing *characters and styling together*.
    //
    // Two cases used to live here: "the text changed" (patch the changed
    // columns) and "same text, different styling". The first case won whenever
    // any glyph differed, and then a style change elsewhere in the same row was
    // left behind — a selected cell that stopped being selected kept its gold
    // border because a few columns further right the artwork had also changed.
    // It showed up the moment a row mixes both, which is exactly what the
    // shelf does when the selection moves: the new cell's artwork appears in
    // the same row whose previous border fades out.
    let (old_styles, old_cells) = cells_of(&old_tokens);
    let (new_styles, new_cells) = cells_of(&new_tokens);
    if old_cells.len() == new_cells.len() {
        let mut first_col: Option<usize> = None;
        let mut last_col = 0usize;
        for (o, n) in old_cells.iter().zip(new_cells.iter()) {
            let _ = n;
            if o.1 != n.1 || old_styles[o.2] != new_styles[n.2] {
                first_col.get_or_insert(o.0);
                last_col = last_col.max(o.0);
            }
        }
        if let Some(first_col) = first_col {
            return Some((first_col, slice_for(&new_tokens, first_col, last_col)));
        }
        // Same characters and same styles: only the escape layout differs.
        return Some((0, format!("\x1b[0m{new}")));
    }

    // Width changed (or the token layout differs): rewrite the whole row. The
    // caller already truncated it to the terminal width.
    Some((0, format!("\x1b[0m{new}")))
}

/// One entry per visible column: the character and the style in force there.
///
/// `cells_of` returns the distinct styles it found plus one entry per column
/// holding the *index* of the style that applies there. Styles are resolved —
/// every escape in force at that column, not just the last one — so two columns
/// compare equal only when they really look the same.
///
/// That distinction was a visible bug on the covers: a half block is two pixels
/// drawn with **two** escapes (`fg` for the top pixel, `bg` for the bottom one).
/// Remembering only the last escape meant a cell whose top pixel changed while
/// its bottom pixel did not compared *equal*, so the painter left the stale top
/// pixel on screen: covers kept ghosts of the previous game.
fn cells_of(line: &[Token]) -> (Vec<String>, Vec<(usize, char, usize)>) {
    let mut styles: Vec<String> = vec![String::new()];
    let mut state = String::new();
    let mut current = 0usize;
    let mut out = Vec::with_capacity(line.len());
    for t in line {
        if let Some(e) = &t.escape {
            // A reset starts a new state; anything else extends it.
            if e.starts_with("\x1b[0") || e.starts_with("\x1b[m") {
                state.clear();
            }
            state.push_str(e);
            current = match styles.iter().position(|s| *s == state) {
                Some(i) => i,
                None => {
                    styles.push(state.clone());
                    styles.len() - 1
                }
            };
        }
        if let Some(c) = t.ch {
            out.push((t.col, c, current));
        }
    }
    (styles, out)
}

/// Bytes that turn columns `first..=last` of `line` into `line` again.
///
/// Three subtleties this handles, all of which used to corrupt the screen:
/// * the slice starts with a reset **and** re-emits the *whole* SGR state in
///   force at `first`, so a patched column inherits its intended colours
///   instead of the default ones. Only re-emitting the last escape (what this
///   did) dropped the foreground of every half block, because a cover cell
///   carries `fg` and `bg` as two separate escapes;
/// * a trailing reset closes the styling so the untouched tail of the row keeps
///   whatever it had.
fn slice_for(line: &[Token], first: usize, last: usize) -> String {
    // Walk the row accumulating the SGR state; the moment the slice starts,
    // emit a reset plus the whole state in force there, then the escapes and
    // characters of the range itself.
    let mut slice = String::new();
    let mut state = String::new();
    let mut started = false;
    let mut had_escape = false;
    for t in line {
        if let Some(e) = &t.escape {
            if e.starts_with("\x1b[0") || e.starts_with("\x1b[m") {
                state.clear();
            }
            state.push_str(e);
            if !started && t.col >= first {
                slice.push_str("\x1b[0m");
                slice.push_str(&state);
                started = true;
                had_escape = true;
            } else if started && t.col <= last {
                slice.push_str(e);
                had_escape = true;
            }
            continue;
        }
        let Some(c) = t.ch else { continue };
        if !started {
            if t.col < first {
                continue;
            }
            slice.push_str("\x1b[0m");
            slice.push_str(&state);
            started = true;
            had_escape = !state.is_empty();
        }
        if t.col <= last {
            slice.push(c);
        }
    }
    if had_escape {
        slice.push_str("\x1b[0m");
    }
    slice
}

/// Flicker-free painter: repaints only the rows that actually changed.
///
/// Clearing and redrawing the whole screen on every keystroke is what makes a
/// TUI flash (the redraw is visible mid-way). Row-level diffing keeps untouched
/// rows on screen and [`row_patch`] keeps untouched *columns* of a dirty row,
/// so a cursor move rewrites a few hundred bytes instead of ~55 KB.
#[derive(Debug, Default)]
pub struct Screen {
    prev: Vec<String>,
    width: u16,
    height: u16,
    painted: bool,
}

impl Screen {
    /// New screen with no state (the next draw is a full paint).
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget what is on screen (after a clear, a resize or a suspend).
    pub fn invalidate(&mut self) {
        self.painted = false;
        self.prev.clear();
    }

    /// Rows that changed since the last draw (`None` = nothing to do).
    ///
    /// Pure and public so the diff can be asserted without a terminal.
    pub fn dirty_rows(&self, frame: &str) -> Option<(usize, usize)> {
        if !self.painted {
            return Some((0, usize::MAX));
        }
        let lines: Vec<&str> = frame.lines().collect();
        let mut first = usize::MAX;
        let mut last = 0;
        for (i, line) in lines.iter().enumerate() {
            if self.prev.get(i).map(String::as_str) != Some(line) {
                first = first.min(i);
                last = i;
            }
        }
        let shrank = lines.len() < self.prev.len();
        if first == usize::MAX {
            // No row changed, but a shorter frame leaves rows behind: the last
            // row still has to be touched so the leftovers can be erased.
            if shrank {
                Some((lines.len().saturating_sub(1), lines.len().saturating_sub(1)))
            } else {
                None
            }
        } else {
            Some((first, last))
        }
    }

    /// Paint `frame`, touching only the dirty rows. `sync` wraps the update in
    /// synchronized-output mode when the terminal supports it.
    pub fn draw<W: Write>(&mut self, out: &mut W, frame: &str, width: u16, height: u16, sync: bool) -> std::io::Result<()> {
        let resize = (width, height) != (self.width, self.height);
        if resize {
            self.invalidate();
            self.width = width;
            self.height = height;
        }
        let lines: Vec<&str> = frame.lines().collect();
        let Some((first, last)) = self.dirty_rows(frame) else { return Ok(()) };
        let first = first.min(lines.len().saturating_sub(1));
        let last = last.min(lines.len().saturating_sub(1));
        if lines.is_empty() {
            self.prev.clear();
            self.painted = true;
            return Ok(());
        }
        if sync {
            out.write_all(SYNC_BEGIN)?;
        }
        let max = safe_width(width);
        // Remember exactly what goes on screen: comparing a truncated frame
        // against an untruncated copy is how a stale column survives a patch.
        let shown: Vec<String> = lines.iter().map(|l| truncate_visible(l, max)).collect();
        let mut body = String::new();
        if !self.painted {
            // First paint (or after a resize): wipe once, then draw in place.
            body.push_str("\x1b[2J\x1b[1;1H");
            body.push_str(&shown.join("\r\n"));
        } else {
            // Row by row, and inside each row only the changed columns: a
            // cursor move costs ~6 bytes, rewriting a 190-column cover row
            // costs kilobytes.
            for (i, line) in shown.iter().enumerate().take(last + 1).skip(first) {
                let previous = self.prev.get(i).map(String::as_str).unwrap_or("");
                let Some((col, slice)) = row_patch(previous, line) else { continue };
                body.push_str(&format!("\x1b[{};{}H", i + 1, col + 1));
                body.push_str(&slice);
            }
        }
        if shown.len() < self.prev.len() {
            // Erase whatever the previous, taller frame left behind.
            body.push_str(&format!("\x1b[{};1H\x1b[J", shown.len() + 1));
        }
        if !body.is_empty() {
            out.write_all(body.as_bytes())?;
        }
        if sync {
            out.write_all(SYNC_END)?;
        }
        self.prev = shown;
        self.painted = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row can change characters *and* styling at once: when the
    /// selection moves, the new cell's artwork appears in the same row whose
    /// previous border fades out. The patch must cover both, or the border
    /// keeps its old colour while the art next to it updates.
    #[test]
    fn a_patch_covers_style_changes_next_to_artwork_changes() {
        let gold = "\x1b[0;1;38;2;255;205;90m";
        let dim = "\x1b[0;38;2;90;95;115m";
        let old = format!("{gold}|{dim}abc {gold}|");
        // Same glyphs on the left (the border loses its gold), different art on
        // the right.
        let new = format!("{dim}|{dim}xyz {gold}|");
        let (col, slice) = row_patch(&old, &new).expect("the row changed");

        // Applying the patch to the old row must produce the new row, checked on
        // a terminal model (characters *and* styles), not on the bytes.
        let mut term = vt::Terminal::new(8, 1);
        term.feed(&format!("\x1b[1;1H{old}"));
        term.feed(&format!("\x1b[1;{}H{slice}", col + 1));
        let mut expected = vt::Terminal::new(8, 1);
        expected.feed(&format!("\x1b[1;1H{new}"));
        if let Some((y, x, got, want)) = term.first_difference(&expected) {
            panic!("at ({y},{x}) patched {got:?} but expected {want:?}");
        }
    }

    #[test]
    fn every_newline_is_crlf() {
        let bytes = frame_bytes("one\ntwo\nthree", 80);
        assert_eq!(bytes, b"one\r\ntwo\r\nthree");
        for (i, b) in bytes.iter().enumerate() {
            if *b == b'\n' {
                assert!(i > 0 && bytes[i - 1] == b'\r', "bare LF at {i}");
            }
        }
    }

    #[test]
    fn never_fills_the_last_column() {
        // 190-col terminal: a 190-char line would trip deferred wrap.
        let long = "─".repeat(300);
        let bytes = frame_bytes(&long, 190);
        assert_eq!(bytes.iter().filter(|b| **b == b'\n').count(), 0);
        // 3 bytes per box char, 189 chars max.
        assert_eq!(bytes.len(), 189 * 3);
    }

    #[test]
    fn tiny_terminals_do_not_panic() {
        for w in [0u16, 1, 2, 3] {
            let bytes = frame_bytes("abcdef\ngh", w);
            let text = String::from_utf8(bytes).unwrap();
            for line in text.split("\r\n") {
                assert!(line.chars().count() <= safe_width(w));
            }
        }
        // 0 columns (unsized pty) must not collapse lines to one char.
        assert_eq!(safe_width(0), 79);
        assert_eq!(safe_width(1), 79);
        assert_eq!(safe_width(20), 19);
        assert_eq!(safe_width(190), 189);
    }

    #[test]
    fn visible_len_ignores_styling() {
        assert_eq!(visible_len("plain"), 5);
        assert_eq!(visible_len("\x1b[1;31mred\x1b[0m"), 3);
        assert_eq!(visible_len("\x1b]0;title\x07x"), 1);
        assert_eq!(visible_len("\x1b[38;2;255;0;0m▀\x1b[0m"), 1);
        assert_eq!(visible_len(""), 0);
        assert_eq!(visible_len("\x1b"), 0);
    }

    #[test]
    fn truncation_keeps_escapes_and_closes_style() {
        let styled = "\x1b[1;35mSUPER MARIO\x1b[0m tail";
        let cut = truncate_visible(styled, 6);
        assert_eq!(visible_len(&cut), 6);
        assert!(cut.starts_with("\x1b[1;35m"));
        assert!(cut.ends_with("\x1b[0m"), "must reset style: {cut:?}");
        // Nothing cut when it already fits.
        assert_eq!(truncate_visible(styled, 100), styled);
    }

    #[test]
    fn styled_lines_reach_the_wire_intact() {
        let frame = "\x1b[1;35mgrid\x1b[0m\n\x1b[38;2;10;20;30m▀▀▀\x1b[0m";
        let text = String::from_utf8(frame_bytes(frame, 80)).unwrap();
        let rows: Vec<&str> = text.split("\r\n").collect();
        assert_eq!(rows[0], "\x1b[1;35mgrid\x1b[0m");
        assert_eq!(visible_len(rows[1]), 3);
        assert!(rows[1].contains("\x1b[38;2;10;20;30m"));
    }

    #[test]
    fn screen_only_repaints_dirty_rows() {
        let a = "header\nrow1\nrow2\nfooter";
        let b = "header\nrow1\nROW2\nfooter";
        let mut s = Screen::new();
        let mut buf: Vec<u8> = Vec::new();
        s.draw(&mut buf, a, 40, 4, false).unwrap();
        let first_paint = buf.len();
        assert!(String::from_utf8_lossy(&buf).contains("\x1b[2J"), "first paint clears once");

        // No change => nothing written at all.
        buf.clear();
        s.draw(&mut buf, a, 40, 4, false).unwrap();
        assert!(buf.is_empty(), "unchanged frame must not touch the terminal");

        // One changed row => a tiny cursor move, not a full repaint.
        buf.clear();
        s.draw(&mut buf, b, 40, 4, false).unwrap();
        let text = String::from_utf8_lossy(&buf);
        assert!(text.starts_with("\x1b[3;1H"), "moves to row 3: {text:?}");
        assert!(text.contains("ROW"), "only the three changed glyphs are resent: {text:?}");
        assert!(!text.contains("row2"));
        assert!(!text.contains("\x1b[2J"), "no full clear on an update");
        assert!(buf.len() < first_paint, "update {} < full paint {first_paint}", buf.len());

        // The diff contract itself.
        s.draw(&mut buf, b, 40, 4, false).unwrap();
        assert_eq!(s.dirty_rows(b), None);
        assert_eq!(s.dirty_rows(a), Some((2, 2)));
        assert_eq!(s.dirty_rows("header\nrow1"), Some((1, 1)), "shrink repaints the tail");
        assert_eq!(s.dirty_rows("header\nrow1\nrow2"), Some((2, 2)), "shrink with identical rows");
    }

    #[test]
    fn screen_erases_rows_left_by_a_taller_frame() {
        let mut s = Screen::new();
        let mut buf = Vec::new();
        s.draw(&mut buf, "a\nb\nc\nd", 40, 4, false).unwrap();
        buf.clear();
        // Same terminal size, shorter frame: only the leftovers need erasing.
        s.draw(&mut buf, "a\nb", 40, 4, false).unwrap();
        let text = String::from_utf8_lossy(&buf);
        assert!(text.contains("\x1b[J"), "must erase the leftovers: {text:?}");
    }

    /// One styled segment of `w` columns (border-like text with a colour).
    fn segment(selected: bool, w: usize, ch: char) -> String {
        let sgr = if selected { "\x1b[0;1;38;2;255;205;90m" } else { "\x1b[0;38;2;90;95;115m" };
        format!("{sgr}{}\x1b[0m", ch.to_string().repeat(w))
    }

    /// A frame of `rows` x safe-width columns built from segments; `selected`
    /// decides which segment is highlighted (the shape of the carousel).
    fn framed_frame(cols: usize, rows: usize, selected: usize, tag: char) -> String {
        let target = safe_width(cols as u16) - 1; // one column for the tag
        let seg_w = (target - 3) / 3;
        (0..rows)
            .map(|row| {
                let ch = if row % 2 == 0 { '\u{2502}' } else { '\u{2500}' };
                let parts: Vec<String> = (0..3).map(|c| segment(c == selected && c % 2 == row % 2, seg_w, ch)).collect();
                let line = format!(" {}", parts.join(" "));
                format!("{line}{}{tag}", " ".repeat(target.saturating_sub(visible_len(&line))))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Paint `frames` one after another and check the resulting screen against a
    /// single full repaint of the last frame: exactly what a terminal must show.
    fn assert_painting_equals_full_repaint(cols: usize, rows: usize, frames: &[String]) {
        let mut screen = Screen::new();
        let mut term = vt::Terminal::new(cols, rows);
        for frame in frames {
            let mut bytes = Vec::new();
            screen.draw(&mut bytes, frame, cols as u16, rows as u16, false).unwrap();
            term.feed_bytes(&bytes);
        }
        let mut expected = vt::Terminal::new(cols, rows);
        expected.feed(&format!("\x1b[2J\x1b[1;1H{}", vt::to_crlf(frames.last().unwrap())));
        if let Some((y, x, got, want)) = term.first_difference(&expected) {
            panic!("celda ({y},{x}) pintada {got:?}, un repintado completo daría {want:?}");
        }
    }

    #[test]
    fn painter_output_equals_the_frame_on_a_real_terminal_model() {
        // The strongest check available: run the painter's bytes through a
        // terminal model and compare characters *and* styling. Comparing bytes
        // proves nothing, but this catches a patch that lands in the wrong
        // column or leaks a style into untouched cells.
        for cols in [40usize, 60, 120] {
            let rows = 3;
            // The selection walks 0 -> 1 -> 2 and back, like the carousel does.
            let frames: Vec<String> = [0usize, 1, 2, 1, 0].iter().map(|s| framed_frame(cols, rows, *s, 'x')).collect();
            assert_eq!(visible_len(frames[0].lines().next().unwrap()), safe_width(cols as u16));
            assert_painting_equals_full_repaint(cols, rows, &frames);
        }
    }

    #[test]
    fn painter_output_equals_the_frame_when_frames_change_shape() {
        // Rows of different widths (a search result, a filter, a shorter list)
        // must still converge to the last frame painted in full.
        let cols = 40usize;
        let rows = 3usize;
        let wide = framed_frame(cols, rows, 0, 'x');
        let narrow = (0..rows)
            .map(|r| format!("{}{}", segment(r == 1, 6 + r, '\u{2502}'), " ".repeat(20)))
            .collect::<Vec<_>>()
            .join("\n");
        assert_painting_equals_full_repaint(cols, rows, &[wide.clone(), narrow, wide.clone()]);
    }

    #[test]
    fn screen_uses_synchronized_output_when_asked() {
        let mut s = Screen::new();
        let mut buf = Vec::new();
        s.draw(&mut buf, "x\ny", 40, 2, true).unwrap();
        assert!(buf.starts_with(SYNC_BEGIN));
        assert!(buf.ends_with(SYNC_END));
        assert!(supports_synchronized_output("xterm-kitty"));
        assert!(supports_synchronized_output("foot-extra"));
        assert!(supports_synchronized_output("xterm-wezterm"));
        assert!(!supports_synchronized_output("xterm-256color"));
    }

    #[test]
    fn row_patch_touches_only_the_changed_columns() {
        let old = "\x1b[38;2;90;95;115m│\x1b[0m  \x1b[38;2;1;2;3m▀\x1b[38;2;4;5;6m▀\x1b[0m\x1b[0m▀  │";
        // Same pixels, but the left border is now the selection colour.
        let new = "\x1b[38;2;255;205;90;1m│\x1b[0m  \x1b[38;2;1;2;3m▀\x1b[38;2;4;5;6m▀\x1b[0m\x1b[0m▀  │";
        let (col, slice) = row_patch(old, new).expect("row changed");
        assert_eq!(col, 0, "first difference is column 0");
        assert!(slice.contains("255;205;90"), "{slice:?}");
        assert!(slice.contains('│'));
        assert!(!slice.contains('▀'), "untouched cover pixels are not resent");
        assert!(slice.starts_with("\x1b[0m") && slice.ends_with("\x1b[0m"));
        assert_eq!(visible_len(&slice), 1);
    }

    #[test]
    fn row_patch_identical_rows_and_plain_changes() {
        assert_eq!(row_patch("abc", "abc"), None, "identical rows need no write");
        let (col, slice) = row_patch("abc", "abX").unwrap();
        assert_eq!(col, 2);
        assert_eq!(slice, "\x1b[0mX", "no trailing reset needed without escapes");
        // Width change falls back to a full-row rewrite.
        let (col, slice) = row_patch("abc", "abcd").unwrap();
        assert_eq!(col, 0);
        assert!(slice.ends_with("abcd"));
    }

    #[test]
    fn row_patch_rewrites_the_whole_styled_run() {
        // Selection moved: run 2 changes colour but its glyphs do not change, so
        // a naive "only the changed glyph" patch leaves the rest of the run in
        // the old colour.
        let gold = "\x1b[0;1;38;2;255;205;90m";
        let grey = "\x1b[0;38;2;90;95;115m";
        let old = format!("{gold}╭{}╮\x1b[0m {grey}╭{}╮\x1b[0m", "─".repeat(20), "─".repeat(20));
        let new = format!("{grey}╭{}╮\x1b[0m {gold}╭{}╮\x1b[0m", "─".repeat(20), "─".repeat(20));
        let (col, slice) = row_patch(&old, &new).expect("row changed");
        let painted: String = slice.chars().filter(|c| !c.is_ascii_hexdigit() && *c != '\x1b' && *c != '[' && *c != ';' && *c != 'm').collect();
        assert_eq!(painted.chars().count(), 45, "must cover both runs: {slice:?}");
        assert!(col <= 1);
        // The new colour must come before the second run's glyphs.
        let gold_at = slice.find(gold).unwrap();
        let second_run = slice.rfind("╭").unwrap();
        assert!(gold_at < second_run, "second run painted with the new colour");
    }

    #[test]
    fn a_half_block_that_only_changes_its_top_pixel_is_repatched() {
        // One cover cell is one half block with *two* escapes: `fg` for the top
        // pixel and `bg` for the bottom one. When only the top pixel changes,
        // the last escape in force (the background) is identical, so a diff
        // that remembered just that one escape considered the cell unchanged and
        // left the old pixel on screen — covers kept ghosts of the last game.
        let cell = |top: &str, bottom: &str| {
            format!("\x1b[0;38;2;{top}m\x1b[48;2;{bottom}m\u{2580}")
        };
        // Two cells side by side; only the top pixel of the first one changes.
        let old = format!("{}{}", cell("204;84;144", "31;12;16"), cell("10;20;30", "40;50;60"));
        let new = format!("{}{}", cell("255;105;180", "31;12;16"), cell("10;20;30", "40;50;60"));
        let (col, slice) = row_patch(&old, &new).expect("the top pixel changed, so the row must be patched");
        assert_eq!(col, 0);
        assert!(slice.contains("255;105;180"), "the new top colour must be sent: {slice:?}");
        assert!(slice.contains("48;2;31;12;16"), "and the bottom colour with it: {slice:?}");
        assert!(
            !slice.contains("40;50;60"),
            "only the cell that changed is rewritten, not the whole row: {slice:?}"
        );
    }

    #[test]
    fn row_patch_keeps_a_multibyte_change_intact() {
        let old = "│  Mario  │";
        let new = "│  Mario …│";
        let (col, slice) = row_patch(old, new).unwrap();
        assert_eq!(col, 9, "the ellipsis replaces the last space");
        assert_eq!(visible_len(&slice), 1);
        assert!(slice.contains('…'));
    }

    #[test]
    fn screen_update_is_bytes_not_kilobytes() {
        // Regression for the flicker: a one-column change in a wide styled row
        // must not resend the whole row.
        let old_line = format!("{}│{}▀{}▀{}│{}", "\x1b[38;2;90;95;115m", "\x1b[0m", "\x1b[38;2;1;2;3m", "\x1b[38;2;4;5;6m", "\x1b[0m");
        let frame: String = (0..40)
            .map(|i| {
                if i == 5 {
                    old_line.replace("90;95;115", "255;205;90")
                } else {
                    format!("plain row {i} ...............................................................")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let original: String = (0..40)
            .map(|i| {
                if i == 5 {
                    old_line.clone()
                } else {
                    format!("plain row {i} ...............................................................")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut s = Screen::new();
        let mut buf = Vec::new();
        s.draw(&mut buf, &original, 100, 40, false).unwrap();
        let full = buf.len();
        buf.clear();
        s.draw(&mut buf, &frame, 100, 40, false).unwrap();
        assert!(buf.len() < full / 8, "update {} vs full {full}", buf.len());
        assert!(buf.len() < 120, "a single styled column costs ~{} bytes", buf.len());
    }

    #[test]
    fn screen_skips_untouched_rows_between_dirty_ones() {
        let base = (0..10).map(|i| format!("row{i}")).collect::<Vec<_>>().join("\n");
        let mut s = Screen::new();
        let mut buf = Vec::new();
        s.draw(&mut buf, &base, 40, 10, false).unwrap();
        // Change row 1 and row 8 only.
        let mut lines: Vec<String> = base.lines().map(str::to_owned).collect();
        lines[1] = "row1!".into();
        lines[8] = "row8!".into();
        let changed = lines.join("\n");
        buf.clear();
        s.draw(&mut buf, &changed, 40, 10, false).unwrap();
        let text = String::from_utf8_lossy(&buf);
        assert!(text.starts_with("\x1b[2;1H"), "starts at the first dirty row: {text:?}");
        assert!(text.contains("\x1b[9;1H"), "jumps to the second dirty row");
        assert!(!text.contains("row3"), "untouched rows are not rewritten");
        assert!(!text.contains("\x1b[3;1H"), "no cursor move to a clean row");
    }

    #[test]
    fn screen_resize_forces_a_full_repaint() {
        let mut s = Screen::new();
        let mut buf = Vec::new();
        s.draw(&mut buf, "a\nb", 40, 2, false).unwrap();
        buf.clear();
        s.draw(&mut buf, "a\nb", 60, 2, false).unwrap();
        assert!(String::from_utf8_lossy(&buf).contains("\x1b[2J"), "resize repaints");
    }

    #[test]
    fn hex_parsing_covers_pywal_forms() {
        assert_eq!(parse_hex("#2A5148"), Some((42, 81, 72)));
        assert_eq!(parse_hex("2A5148"), Some((42, 81, 72)));
        assert_eq!(parse_hex("#fff"), Some((255, 255, 255)));
        assert_eq!(parse_hex("  #000000 "), Some((0, 0, 0)));
        for bad in ["", "#", "zzzzzz", "12345", "rgb(1,2,3)"] {
            assert_eq!(parse_hex(bad), None, "{bad} must not parse");
        }
    }

    #[test]
    fn blend_and_contrast_behave() {
        assert_eq!(blend((0, 0, 0), (255, 255, 255), 0.0), (0, 0, 0));
        assert_eq!(blend((0, 0, 0), (255, 255, 255), 1.0), (255, 255, 255));
        assert_eq!(blend((0, 0, 0), (255, 255, 255), 0.5), (128, 128, 128));
        assert_eq!(blend((10, 20, 30), (200, 0, 0), 2.0), (200, 0, 0), "t is clamped");
        let white_black = contrast((255, 255, 255), (0, 0, 0));
        assert!((white_black - 21.0).abs() < 0.1, "{white_black}");
        assert!((contrast((10, 10, 10), (20, 20, 20)) - 1.07).abs() < 0.05, "near-identical darks are unreadable");
    }

    #[test]
    fn width_matches_a_real_frame() {
        // Regression shape: header + rule + row, as the library renders it.
        let frame = " ROSADECK LIBRARY [ 13] 1-5 plat\n filter: all\n>  Luigi's Mansion\n";
        let bytes = frame_bytes(frame, 190);
        let text = String::from_utf8(bytes).unwrap();
        let rows: Vec<&str> = text.split("\r\n").collect();
        assert_eq!(rows.len(), 3);
        for r in &rows {
            assert!(r.chars().count() <= 189, "{r:?}");
            assert!(!r.contains('\n'));
        }
    }
}