//! A minimal terminal model: what a real terminal would end up showing.
//!
//! Used by the tests (and available to the apps) to check that the diff painter
//! is *semantically* correct: feeding its byte stream into this model must
//! produce exactly the last frame, characters **and** styling. Comparing bytes
//! proves nothing — the same wrong output can look plausible.
//!
//! It understands only what `tui-frame` emits: absolute cursor moves, SGR
//! (reset, bold, reverse, 24-bit and 256-colour foreground), ED, EL and CRLF.

/// Style attributes tracked per cell.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Style {
    /// Bold (`SGR 1`).
    pub bold: bool,
    /// Reverse video (`SGR 7`).
    pub reverse: bool,
    /// Foreground colour as written, e.g. `38;2;10;20;30` or `38;5;99`.
    pub fg: Option<String>,
}

impl Style {
    /// Reset style (what `SGR 0` gives).
    pub const RESET: Self = Self { bold: false, reverse: false, fg: None };
}

/// A character plus the style it was written with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// The character.
    pub ch: char,
    /// Its styling.
    pub style: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Self { ch: ' ', style: Style::RESET }
    }
}

/// Screen model.
#[derive(Debug, Clone)]
pub struct Terminal {
    /// Cell grid, `rows` x `cols`.
    pub cells: Vec<Vec<Cell>>,
    /// Cursor column.
    pub cx: usize,
    /// Cursor row.
    pub cy: usize,
    /// Cursor column width (always 1 here).
    cols: usize,
    rows: usize,
    /// Style in effect for the next character written.
    style: Style,
}

impl Terminal {
    /// New empty terminal.
    pub fn new(cols: usize, rows: usize) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Self { cells: vec![vec![Cell::default(); cols]; rows], cx: 0, cy: 0, cols, rows, style: Style::RESET }
    }

    /// Feed a raw byte stream (UTF-8, chunk boundaries anywhere).
    pub fn feed_bytes(&mut self, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes).into_owned();
        self.feed(&text);
    }

    /// Feed a text stream.
    pub fn feed(&mut self, text: &str) {
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '\x1b' {
                i = self.escape(&chars, i);
                continue;
            }
            match chars[i] {
                '\r' => self.cx = 0,
                '\n' => {
                    self.cy = (self.cy + 1).min(self.rows - 1);
                }
                _ => self.put(chars[i]),
            }
            i += 1;
        }
    }

    /// Handle an escape sequence starting at `i`; returns the next index.
    fn escape(&mut self, chars: &[char], i: usize) -> usize {
        let next = chars.get(i + 1).copied();
        match next {
            Some('[') => {
                let mut j = i + 2;
                let mut seq = String::new();
                while let Some(c) = chars.get(j) {
                    if ('\x40'..='\x7e').contains(c) {
                        break;
                    }
                    seq.push(*c);
                    j += 1;
                }
                let final_byte = chars.get(j).copied().unwrap_or('m');
                match final_byte {
                    'H' | 'f' => {
                        let mut parts = seq.split(';');
                        let row = parts.next().and_then(|v| v.parse::<usize>().ok()).unwrap_or(1);
                        let col = parts.next().and_then(|v| v.parse::<usize>().ok()).unwrap_or(1);
                        self.cy = row.saturating_sub(1).min(self.rows - 1);
                        self.cx = col.saturating_sub(1).min(self.cols - 1);
                    }
                    'J' => {
                        let mode: usize = seq.parse().unwrap_or(0);
                        if mode == 2 {
                            self.clear_all();
                        } else {
                            self.erase_down();
                        }
                    }
                    'K' => {
                        let blank = Cell::default();
                        for x in self.cx..self.cols {
                            self.cells[self.cy][x] = blank.clone();
                        }
                    }
                    'm' => self.sgr(&seq),
                    _ => {}
                }
                j + 1
            }
            // APC/OSC (kitty graphics payloads, window titles): skip to ST/BEL.
            Some('_') | Some(']') | Some('P') | Some('^') => {
                let mut j = i + 2;
                while j < chars.len() {
                    if chars[j] == '\x07' {
                        return j + 1;
                    }
                    if chars[j] == '\x1b' && chars.get(j + 1) == Some(&'\\') {
                        return j + 2;
                    }
                    j += 1;
                }
                j
            }
            Some(_) => i + 2,
            None => i + 1,
        }
    }

    /// Apply an SGR parameter list.
    fn sgr(&mut self, seq: &str) {
        if seq.is_empty() {
            self.style = Style::RESET;
            return;
        }
        let parts: Vec<&str> = seq.split(';').collect();
        let mut k = 0;
        while k < parts.len() {
            match parts[k] {
                "0" => self.style = Style::RESET,
                "1" => self.style.bold = true,
                "7" => self.style.reverse = true,
                "22" => self.style.bold = false,
                "27" => self.style.reverse = false,
                "38" => {
                    // 38;2;r;g;b or 38;5;n
                    match parts.get(k + 1) {
                        Some(&"2") => {
                            let get = |n: usize| parts.get(k + n).copied().unwrap_or("0");
                            self.style.fg = Some(format!("38;2;{};{};{}", get(2), get(3), get(4)));
                            k += 4;
                        }
                        Some(&"5") => {
                            let get = |n: usize| parts.get(k + n).copied().unwrap_or("0");
                            self.style.fg = Some(format!("38;5;{}", get(2)));
                            k += 2;
                        }
                        _ => {}
                    }
                }
                "39" => self.style.fg = None,
                _ => {}
            }
            k += 1;
        }
    }

    /// Write one character at the cursor.
    fn put(&mut self, ch: char) {
        if self.cx >= self.cols {
            self.cx = self.cols - 1;
        }
        self.cells[self.cy][self.cx] = Cell { ch, style: self.style.clone() };
        self.cx += 1;
    }

    /// Erase from the cursor to the end of the screen.
    fn erase_down(&mut self) {
        let blank = Cell::default();
        for y in self.cy..self.rows {
            let from = if y == self.cy { self.cx } else { 0 };
            for x in from..self.cols {
                self.cells[y][x] = blank.clone();
            }
        }
    }

    /// Erase everything.
    fn clear_all(&mut self) {
        self.cells = vec![vec![Cell::default(); self.cols]; self.rows];
        self.cx = 0;
        self.cy = 0;
    }

    /// Render as text, one line per row (trailing blanks trimmed).
    pub fn text(&self) -> String {
        self.cells
            .iter()
            .map(|row| row.iter().map(|c| c.ch).collect::<String>().trim_end().to_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Render including styling, for exact comparisons.
    ///
    /// A blank cell ignores its foreground: a space painted in any colour looks
    /// the same, so comparing it would report differences nobody can see.
    pub fn styled(&self) -> String {
        self.cells
            .iter()
            .map(|row| {
                row.iter()
                    .map(|c| {
                        let blank = c.ch == ' ';
                        let f = match (&c.style.fg, blank) {
                            (None, _) | (Some(_), true) => "-".to_owned(),
                            (Some(fg), false) => fg.clone(),
                        };
                        format!("{}|{}{}", c.ch, f, if c.style.bold { "b" } else { "" })
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// First cell where two screens differ (`None` when identical).
    pub fn first_difference(&self, other: &Self) -> Option<(usize, usize, Cell, Cell)> {
        for (y, (a, b)) in self.cells.iter().zip(other.cells.iter()).enumerate() {
            for (x, (ca, cb)) in a.iter().zip(b.iter()).enumerate() {
                if ca == cb {
                    continue;
                }
                // A blank cell only differs if its background does.
                let fg_matters = |c: &Cell| c.ch != ' ' || c.style.reverse;
                if !fg_matters(ca) && !fg_matters(cb) {
                    continue;
                }
                return Some((y, x, ca.clone(), cb.clone()));
            }
        }
        None
    }

    /// Expected `styled()` for a frame the painter was supposed to produce.
    ///
    /// Frames are joined with `\n` (the painter's input format); the terminal
    /// only ever receives CRLF, so normalise before replaying.
    pub fn expected(frame: &str, cols: usize, rows: usize) -> String {
        let mut term = Terminal::new(cols, rows);
        term.feed(&to_crlf(frame));
        term.styled()
    }

}

/// What the painter writes: `\n` line endings become CRLF.
pub fn to_crlf(frame: &str) -> String {
    frame.replace("\r\n", "\n").replace('\n', "\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_characters_and_styles() {
        let mut t = Terminal::new(10, 3);
        t.feed("ab\r\n\x1b[1;38;2;1;2;3mCD\x1b[0mE");
        assert_eq!(t.cells[0][0].ch, 'a');
        assert_eq!(t.cells[1][0].ch, 'C');
        assert!(t.cells[1][0].style.bold, "bold survives until reset");
        assert_eq!(t.cells[1][0].style.fg.as_deref(), Some("38;2;1;2;3"));
        assert_eq!(t.cells[1][1].ch, 'D');
        assert!(t.cells[1][1].style.bold, "both glyphs inherit the style");
        assert_eq!(t.cells[1][2].ch, 'E');
        assert!(!t.cells[1][2].style.bold, "the reset closes it");
        assert_eq!(t.cells[1][2].style.fg, None);
        assert_eq!(t.text().lines().next(), Some("ab"));
        assert_eq!(t.text().lines().nth(1), Some("CDE"));
    }

    #[test]
    fn cursor_moves_and_erases() {
        let mut t = Terminal::new(10, 3);
        t.feed("hello\r\nworld");
        t.feed("\x1b[1;3H");
        assert_eq!((t.cy, t.cx), (0, 2));
        t.feed("XY");
        assert_eq!(t.cells[0][2].ch, 'X');
        t.feed("\x1b[2J");
        assert_eq!(t.text().lines().next().unwrap_or("x").trim(), "");
        t.feed("done\x1b[1;1H\x1b[J");
        assert_eq!(t.text().lines().next().unwrap_or("x").trim(), "");
    }

    #[test]
    fn skips_kitty_graphics_payloads() {
        let mut t = Terminal::new(20, 2);
        t.feed("ab\x1b_Ga=t,f=100,i=1;TWFU\x1b\\\x1b_Ga=p,i=1\x1b\\cd");
        assert_eq!(t.cells[0][2].ch, 'c', "payload must not become text");
        assert_eq!(t.cells[0][3].ch, 'd');
    }

    #[test]
    fn utf8_multibyte_is_one_cell() {
        let mut t = Terminal::new(10, 1);
        t.feed_bytes("╭▀★".as_bytes());
        assert_eq!(t.text(), "╭▀★");
        assert_eq!(t.cx, 3);
    }
}

