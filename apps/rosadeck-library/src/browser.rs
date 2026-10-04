//! Pure library browser model: 2-D grid navigation, filtering and actions.
//!
//! No terminal IO here: the runtime owns the frame, this owns the state, so
//! every behaviour (wrapping, paging, filtering) is unit-tested headless.

use rosadeck_game_library::{GameEntry, Platform};

/// Browser state (pure).
#[derive(Debug, Clone)]
pub struct Browser {
    /// Platform filter (`None` = all).
    pub platform: Option<Platform>,
    /// Search query (substring, case-insensitive).
    pub query: String,
    /// Cursor in the filtered view (flat index).
    pub cursor: usize,
    /// Typing in search mode.
    pub searching: bool,
    /// Editing the ROM roots (`d`).
    pub roots_input: bool,
    /// What has been typed in the roots editor.
    pub roots_text: String,
    /// Show favorites only.
    pub favorites_only: bool,
    /// Current grid geometry (cells per row) for left/right wrapping.
    pub cols: usize,
    /// Rows in the grid viewport (for page-up/down).
    pub rows: usize,
}

impl Browser {
    /// New browser (all platforms, no query, 1x1 grid until told otherwise).
    pub fn new() -> Self {
        Self {
            platform: None,
            query: String::new(),
            cursor: 0,
            searching: false,
            roots_input: false,
            roots_text: String::new(),
            favorites_only: false,
            cols: 1,
            rows: 1,
        }
    }

    /// Tell the browser the current band geometry (called on every resize).
    pub fn set_geometry(&mut self, cols: usize, rows: usize) {
        self.cols = cols.max(1);
        self.rows = rows.max(1);
    }

    /// Covers on screen at once (a viewport step for Up/Down).
    pub fn page(&self) -> usize {
        (self.cols * self.rows).max(1)
    }

    /// Filtered + ordered games (favorites first when no query, then title).
    pub fn view<'a>(&self, games: &'a [GameEntry], favorites: &[String]) -> Vec<&'a GameEntry> {
        let q = self.query.to_lowercase();
        let mut out: Vec<&GameEntry> = games
            .iter()
            .filter(|g| self.platform.is_none_or(|p| g.platform == p))
            .filter(|g| !self.favorites_only || favorites.contains(&g.id))
            .filter(|g| q.is_empty() || g.title.to_lowercase().contains(&q))
            .collect();
        out.sort_by(|a, b| {
            let fa = favorites.contains(&a.id);
            let fb = favorites.contains(&b.id);
            fb.cmp(&fa).then(a.title.cmp(&b.title))
        });
        out
    }

    /// Clamp the cursor into the current view.
    pub fn clamp(&mut self, len: usize) {
        self.cursor = if len == 0 { 0 } else { self.cursor.min(len - 1) };
    }
}

/// Key outcome for the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserKey {
    /// Keep browsing.
    Continue,
    /// Play the cursor game.
    Play(usize),
    /// Toggle favorite on the cursor game.
    ToggleFavorite(usize),
    /// Rescan ROM roots.
    Rescan,
    /// Reload the palette (pywal) and repaint the covers.
    Theme,
    /// The roots editor accepted a path (add it, or remove it if already there).
    SetRoot(String),
    /// Quit.
    Quit,
}

/// Toolkit-agnostic key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Move up one grid row.
    Up,
    /// Move down one grid row.
    Down,
    /// Move left one cell.
    Left,
    /// Move right one cell.
    Right,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Jump to the first game.
    Home,
    /// Jump to the last game.
    End,
    /// Play.
    Enter,
    /// Favorite toggle.
    Favorite,
    /// Toggle search mode.
    Search,
    /// Toggle the ROM-roots editor.
    Dirs,
    /// Rescan.
    Rescan,
    /// Reload the palette (pywal).
    Theme,
    /// Quit (or leave search mode).
    Esc,
    /// Type a char (search mode).
    Type(char),
    /// Backspace (search mode).
    Backspace,
}

/// Move the cursor, clamped to the view.
///
/// The browser is a single-row carousel, so the model is linear: `Left`/`Right`
/// step by one cover (the band pans to follow) and `Up`/`Down` jump a whole
/// viewport, which is what a user expects from them here.
fn move_cursor(b: &mut Browser, view_len: usize, step: i64) {
    if view_len == 0 {
        b.cursor = 0;
        return;
    }
    let next = b.cursor as i64 + step;
    b.cursor = next.clamp(0, view_len as i64 - 1) as usize;
}

/// Pure key handling over a view length.
pub fn handle_key(b: &mut Browser, view_len: usize, key: Key) -> BrowserKey {
    b.clamp(view_len);
    // A modal window takes the keyboard: navigating the shelf behind it is
    // invisible and confusing (the covers move under a window that covers
    // them). Typing, Enter, Esc, `r` and `d` still work.
    if b.roots_input
        && matches!(
            key,
            Key::Up | Key::Down | Key::Left | Key::Right | Key::PageUp | Key::PageDown | Key::Home | Key::End | Key::Favorite
        )
    {
        return BrowserKey::Continue;
    }
    match key {
        Key::Up | Key::PageUp => {
            move_cursor(b, view_len, -(b.page() as i64));
            BrowserKey::Continue
        }
        Key::Down | Key::PageDown => {
            move_cursor(b, view_len, b.page() as i64);
            BrowserKey::Continue
        }
        Key::Left => {
            move_cursor(b, view_len, -1);
            BrowserKey::Continue
        }
        Key::Right => {
            move_cursor(b, view_len, 1);
            BrowserKey::Continue
        }
        Key::Home => {
            b.cursor = 0;
            BrowserKey::Continue
        }
        Key::End => {
            b.cursor = view_len.saturating_sub(1);
            BrowserKey::Continue
        }
        Key::Enter => {
            if b.roots_input {
                let text = b.roots_text.trim().to_owned();
                if text.is_empty() {
                    // Nothing typed: stay in the editor (the user is about to
                    // type). `Esc` is how you leave without changing anything.
                    return BrowserKey::Continue;
                }
                b.roots_input = false;
                b.roots_text.clear();
                return BrowserKey::SetRoot(text);
            }
            if view_len == 0 {
                BrowserKey::Continue
            } else {
                BrowserKey::Play(b.cursor)
            }
        }
        Key::Favorite => {
            if view_len == 0 {
                BrowserKey::Continue
            } else {
                BrowserKey::ToggleFavorite(b.cursor)
            }
        }
        Key::Rescan => {
            if b.roots_input {
                b.roots_input = false;
                b.roots_text.clear();
            }
            BrowserKey::Rescan
        }
        Key::Theme => BrowserKey::Theme,
        Key::Esc => {
            if b.searching {
                b.searching = false;
                b.query.clear();
            } else if b.roots_input {
                b.roots_input = false;
                b.roots_text.clear();
            } else {
                return BrowserKey::Quit;
            }
            BrowserKey::Continue
        }
        Key::Dirs => {
            // The roots editor takes over the keyboard: while it is open, every
            // character is a path, not a shortcut.
            b.roots_input = !b.roots_input;
            b.roots_text.clear();
            b.searching = false;
            b.query.clear();
            BrowserKey::Continue
        }
        Key::Search => {
            b.searching = !b.searching;
            if !b.searching {
                b.query.clear();
            }
            b.roots_input = false;
            b.roots_text.clear();
            b.cursor = 0;
            BrowserKey::Continue
        }
        Key::Type(c) => {
            if b.roots_input {
                b.roots_text.push(c);
            } else if b.searching {
                b.query.push(c);
                b.cursor = 0;
            }
            BrowserKey::Continue
        }
        Key::Backspace => {
            if b.roots_input {
                b.roots_text.pop();
            } else if b.searching {
                b.query.pop();
                b.cursor = 0;
            }
            BrowserKey::Continue
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn games() -> Vec<GameEntry> {
        vec![
            GameEntry { id: "1".into(), title: "Mario Kart Wii".into(), platform: Platform::Wii, path: "/a".into(), region: None, size: 1 },
            GameEntry { id: "2".into(), title: "Zelda".into(), platform: Platform::Snes, path: "/b".into(), region: None, size: 1 },
            GameEntry { id: "3".into(), title: "Mario Sunshine".into(), platform: Platform::GameCube, path: "/c".into(), region: None, size: 1 },
            GameEntry { id: "4".into(), title: "Metroid".into(), platform: Platform::Nintendo3DS, path: "/d".into(), region: None, size: 1 },
        ]
    }

    #[test]
    fn filters_search_platform_favorites() {
        let mut b = Browser::new();
        let favs = vec!["3".to_owned()];
        assert_eq!(b.view(&games(), &favs).len(), 4);
        assert_eq!(b.view(&games(), &favs)[0].id, "3", "favorites first");
        b.query = "mario".into();
        assert_eq!(b.view(&games(), &favs).len(), 2);
        b.query.clear();
        b.platform = Some(Platform::Wii);
        assert_eq!(b.view(&games(), &favs).len(), 1);
        b.platform = None;
        b.favorites_only = true;
        assert_eq!(b.view(&games(), &favs).len(), 1);
    }

    #[test]
    fn arrows_step_through_the_whole_carousel() {
        let mut b = Browser::new();
        b.set_geometry(3, 1); // three covers on screen
        let len = 11;
        // Right walks the entire list, sliding the band past the visible end.
        for expected in 1..=10 {
            handle_key(&mut b, len, Key::Right);
            assert_eq!(b.cursor, expected, "right reaches cover {expected}");
        }
        handle_key(&mut b, len, Key::Right);
        assert_eq!(b.cursor, 11 - 1, "clamped at the end");
        for expected in (0..10).rev() {
            handle_key(&mut b, len, Key::Left);
            assert_eq!(b.cursor, expected);
        }
        handle_key(&mut b, len, Key::Left);
        assert_eq!(b.cursor, 0, "clamped at the start");
    }

    #[test]
    fn vertical_keys_jump_a_viewport() {
        let mut b = Browser::new();
        b.set_geometry(3, 1);
        assert_eq!(b.page(), 3);
        handle_key(&mut b, 11, Key::Down);
        assert_eq!(b.cursor, 3);
        handle_key(&mut b, 11, Key::PageDown);
        assert_eq!(b.cursor, 6);
        handle_key(&mut b, 11, Key::Up);
        assert_eq!(b.cursor, 3);
        handle_key(&mut b, 11, Key::Home);
        assert_eq!(b.cursor, 0);
        handle_key(&mut b, 11, Key::End);
        assert_eq!(b.cursor, 10);
    }

    #[test]
    fn empty_view_is_inert() {
        let mut b = Browser::new();
        b.set_geometry(4, 3);
        for k in [Key::Down, Key::Right, Key::Up, Key::Left, Key::End] {
            assert_eq!(handle_key(&mut b, 0, k), BrowserKey::Continue);
            assert_eq!(b.cursor, 0);
        }
    }

    #[test]
    fn actions_and_quit() {
        let mut b = Browser::new();
        b.set_geometry(3, 2);
        assert_eq!(handle_key(&mut b, 4, Key::Enter), BrowserKey::Play(0));
        assert_eq!(handle_key(&mut b, 0, Key::Enter), BrowserKey::Continue);
        assert_eq!(handle_key(&mut b, 4, Key::Favorite), BrowserKey::ToggleFavorite(0));
        assert_eq!(handle_key(&mut b, 4, Key::Rescan), BrowserKey::Rescan);
        assert_eq!(handle_key(&mut b, 4, Key::Theme), BrowserKey::Theme);
        assert_eq!(handle_key(&mut b, 4, Key::Esc), BrowserKey::Quit);
    }

    #[test]
    fn search_mode_edits_query_and_escapes() {
        let mut b = Browser::new();
        handle_key(&mut b, 4, Key::Search);
        assert!(b.searching);
        for c in "zel".chars() {
            handle_key(&mut b, 4, Key::Type(c));
        }
        assert_eq!(b.query, "zel");
        handle_key(&mut b, 4, Key::Backspace);
        assert_eq!(b.query, "ze");
        assert_eq!(handle_key(&mut b, 4, Key::Esc), BrowserKey::Continue, "Esc leaves search first");
        assert!(!b.searching);
        assert_eq!(handle_key(&mut b, 4, Key::Esc), BrowserKey::Quit);
    }

    #[test]
    fn clamp_survives_filter_shrinkage() {
        let mut b = Browser::new();
        b.set_geometry(4, 2);
        b.cursor = 7;
        b.clamp(2);
        assert_eq!(b.cursor, 1);
        b.clamp(0);
        assert_eq!(b.cursor, 0);
    }
}