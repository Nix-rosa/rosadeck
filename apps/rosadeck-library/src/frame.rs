//! Frame composition: header box, platform chips, cover grid, detail pane and
//! key legend — all pure, so the exact pixels are unit-testable headless.
//!
//! Invariants enforced by tests (not by hope):
//! * every line is exactly `Layout::drawn_width` visible columns, so the
//!   grid never shears and the box edges line up;
//! * styled output never exceeds the terminal width (ANSI-aware measurement);
//! * grid cells are exactly `cover_h + 4` lines tall and `cover_w + 2` wide.

use crate::browser::Browser;
use crate::cover::{CoverCache, Crop, Treatment};
use crate::emulators::{self, EmulatorStatus};
use crate::images::{plan_for_cropped, Plan};
use crate::layout::{Card, Layout, CARD_GAP, CHROME_ROWS, GRID_TOP};
use crate::icons::Icons;
use crate::theme::{glyph, Style, Theme};
use rosadeck_game_library::{GameEntry, Platform};
use rosadeck_tui_frame::visible_len;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Everything the frame needs (borrowed; no game list is copied).
pub struct FrameInput<'a> {
    /// Filter/navigation state.
    pub browser: &'a Browser,
    /// Every game in the library (unfiltered).
    pub games: &'a [GameEntry],
    /// Favorited game ids.
    pub favorites: &'a [String],
    /// Seconds of emulator time by game id, as recorded by the CLI.
    pub played: &'a HashMap<String, u64>,
    /// Emulator profiles with their binary resolution (detail pane).
    pub emulators: &'a [EmulatorStatus],
    /// ROM roots in scan order (the `d` dialog lists them with their counts).
    pub roots: &'a [PathBuf],
    /// One-line feedback from the last action (played, rescanned, error).
    pub status: &'a str,
    /// How the covers are reaching the terminal, shown in the legend.
    ///
    /// "Se siente lento" is impossible to argue about when the UI says whether
    /// the covers travel as paths (`rutas`), as base64 through the pty (`bytes`)
    /// or as half blocks (`bloques`), and whether this is a debug build — which
    /// is ten times slower than the same code optimised.
    pub art_mode: &'a str,
}

impl<'a> FrameInput<'a> {
    /// Frame input without emulator knowledge: the tests have no profile
    /// list, so every emulator honestly reads as "not configured" instead of
    /// pretending to be installed.
    #[cfg(test)]
    fn base(
        browser: &'a Browser,
        games: &'a [GameEntry],
        favorites: &'a [String],
        played: &'a HashMap<String, u64>,
    ) -> Self {
        Self { browser, games, favorites, played, emulators: &[], roots: &[], status: "", art_mode: "" }
    }
}

/// One rendered frame: the text layer plus the covers that must be sent as
/// images (kitty graphics), with 1-based cell coordinates.
#[derive(Debug, Default, PartialEq)]
pub struct Frame {
    /// Lines joined with `\n`, ready for `tui-frame`.
    pub text: String,
    /// Covers to display as real images instead of half blocks.
    pub images: Vec<Plan>,
}

/// Render one full frame.
///
/// `use_images` comes from terminal capability detection: when the terminal
/// supports the graphics protocol the covers are transmitted as images (native
/// resolution) and their cell area is left empty here; otherwise the frame
/// draws half-block art for every game.
pub fn build(input: &FrameInput<'_>, layout: Layout, theme: &Theme, cache: &mut CoverCache, use_images: bool) -> Frame {
    let games = input.browser.view(input.games, input.favorites);
    // Header, rules, chips, detail and legend span the whole terminal; the
    // cover band is centred inside it (see `Layout::band_indent`).
    let frame_w = layout.width;
    let inner = frame_w.saturating_sub(layout.margin).max(10);
    // Vertical budget: header+chips+rule, the cover band, then the footer.
    debug_assert!(
        GRID_TOP + layout.band_height() + (CHROME_ROWS - GRID_TOP) <= layout.height,
        "cover band overflows the frame: {layout:?}"
    );
    let mut out: Vec<String> = Vec::new();
    let mut images: Vec<Plan> = Vec::new();
    out.push(pad(layout, header(inner, theme, input, games.len())));
    out.push(pad(layout, chips(input, theme, inner)));
    out.push(theme.paint(Style::Border, &rule(layout)));
    out.extend(carousel(input, layout, theme, cache, &games, use_images, &mut images));
    // Spare rows become breathing room between the band and the footer, so the
    // detail pane and the key legend stay anchored to the bottom of the screen.
    // Status line + rule + 3 detail rows + legend. It used to be 5, and the
    // status was never painted at all: every "played X" / "not installed"
    // message went nowhere.
    let footer = 4; // status + rule + detail + legend
    while out.len() + footer < layout.height {
        out.push(pad(layout, String::new()));
    }
    out.push(status_line(input, theme, layout, inner));
    out.push(theme.paint(Style::Border, &rule(layout)));
    out.extend(detail(input, layout, theme, games.get(input.browser.cursor), inner, cache));
    out.push(theme.paint(Style::Border, &pad(layout, legend(theme, inner, input))));
    // Pad to the computed frame height so the box stays square.
    while out.len() < layout.height {
        out.push(pad(layout, String::new()));
    }
    out.truncate(layout.height.max(1));
    // A modal window on top of the shelf: it replaces whole rows, so the band
    // behind it is erased, and the covers go with it. Kitty cannot put an image
    // under text (`z<0` is erased by the very text that would frame it), so a
    // dialog drawn over transmitted covers would be sliced by them.
    if input.browser.roots_input {
        let (top, rows) = roots_dialog(input, theme, layout);
        if !rows.is_empty() {
            images.clear();
            for (i, row) in rows.into_iter().enumerate() {
                if let Some(slot) = out.get_mut(top + i) {
                    *slot = row;
                }
            }
        }
    }
    Frame { text: out.join("\n"), images }
}

/// Rows the window spends on its own frame: top and bottom border.
const DIALOG_BORDER_ROWS: usize = 2;
/// Body rows that are always there: the blank, the prompt and the hint.
const DIALOG_FIXED_ROWS: usize = 3;
/// Widest the dialog gets: past this the paths stop being readable anyway.
const DIALOG_MAX_INNER: usize = 74;
/// Narrowest inner area that can hold a path plus its game count.
const DIALOG_MIN_INNER: usize = 26;

/// The `d` editor as a centred modal window: the first row index to overwrite
/// and the full-width rows that replace them.
///
/// Returns `(0, vec![])` when the terminal is too small for an honest dialog —
/// the status line still explains the keys, which is better than a box that
/// hides the very list it is editing.
fn roots_dialog(input: &FrameInput<'_>, theme: &Theme, layout: Layout) -> (usize, Vec<String>) {
    let inner_w = layout.width.saturating_sub(2 + 2 * layout.margin).min(DIALOG_MAX_INNER);
    let max_h = layout.height.saturating_sub(4);
    if inner_w < DIALOG_MIN_INNER || max_h <= DIALOG_BORDER_ROWS + DIALOG_FIXED_ROWS {
        return (0, Vec::new());
    }
    let title = format!(
        "{} {} {}",
        theme.paint(Style::Accent, "directorios de ROM"),
        theme.paint(Style::Dim, "·"),
        theme.paint(Style::Dim, &format!("{} añadidas", input.roots.len())),
    );
    // The list of roots, with what each one actually contributes.
    let rows_left = max_h - DIALOG_BORDER_ROWS - DIALOG_FIXED_ROWS - 1; // one spare for `+N más`
    let mut body: Vec<String> = Vec::new();
    if input.roots.is_empty() {
        body.push(dialog_content(&theme.paint(Style::Dim, "   ninguna: se usará ~/roms"), None, inner_w));
    }
    let shown = input.roots.len().min(rows_left.saturating_sub(1).max(0));
    for (i, root) in input.roots.iter().take(shown).enumerate() {
        let count = games_under(input.games, root);
        let (note, style) = if !root.is_dir() {
            ("no existe".to_owned(), Style::Alert)
        } else if count == 0 {
            ("0 juegos".to_owned(), Style::Dim)
        } else {
            (format!("{count} {}", if count == 1 { "juego" } else { "juegos" }), Style::Dim)
        };
        let note = theme.paint(style, &note);
        // `NN` + two spaces + path, then the note flush right.
        let path_w = inner_w.saturating_sub(visible_len(&note) + 5);
        let path = theme.paint(Style::Plain, &root.display().to_string());
        let left = format!("{}  {}", theme.paint(Style::Dim, &format!("{:<2}", i + 1)), fit(&path, path_w));
        body.push(dialog_content(&left, Some(&note), inner_w));
    }
    if input.roots.len() > shown {
        body.push(dialog_content(
            &theme.paint(Style::Dim, &format!("   +{} más en ~/.config/rosadeck/roms", input.roots.len() - shown)),
            None,
            inner_w,
        ));
    }
    body.push(dialog_content("", None, inner_w));
    let cursor = theme.paint(Style::Gold, "_");
    body.push(dialog_content(
        &if input.browser.roots_text.is_empty() {
            format!("{} {cursor}", theme.paint(Style::Dim, "ruta ▸"))
        } else {
            format!(
                "{} {}",
                theme.paint(Style::Dim, "ruta ▸"),
                format!("{}{cursor}", theme.paint(Style::Gold, &fit(&input.browser.roots_text, inner_w.saturating_sub(9))))
            )
        },
        None,
        inner_w,
    ));
    body.push(dialog_content(
        &theme.paint(Style::Dim, "   Enter añade · repetirla la quita · Esc cancela"),
        None,
        inner_w,
    ));

    let h = DIALOG_BORDER_ROWS + body.len();
    if h > layout.height {
        return (0, Vec::new());
    }
    let rows: Vec<String> = std::iter::once(dialog_top(theme, inner_w, &title))
        .chain(body.into_iter().map(|line| dialog_row(theme, &line)))
        .chain(std::iter::once(dialog_bottom(theme, inner_w)))
        .collect();
    let left = " ".repeat(layout.width.saturating_sub(inner_w + 2) / 2);
    let right = " ".repeat(layout.width.saturating_sub(left.len() + inner_w + 2));
    let rows: Vec<String> = rows.into_iter().map(|row| format!("{left}{row}{right}")).collect();
    ((layout.height - h) / 2, rows)
}

/// One line of dialog text, padded on visible columns so the box stays square.
///
/// The left part is cut when it does not fit (a hint longer than a narrow
/// window): a dialog that overflows would shear the frame it sits on.
fn dialog_content(left: &str, right: Option<&str>, inner_w: usize) -> String {
    let right = fit(right.unwrap_or(""), inner_w);
    let left = fit(left, inner_w - visible_len(&right));
    format!("{left}{}{right}", " ".repeat(inner_w.saturating_sub(visible_len(&left) + visible_len(&right))))
}

/// A dialog body row: the content between the two verticals.
fn dialog_row(theme: &Theme, content: &str) -> String {
    format!("{}{content}{}", theme.paint(Style::Border, glyph::V), theme.paint(Style::Border, glyph::V))
}

/// `╭─ title ─────╮`, exactly as wide as the body rows (`inner_w + 2`).
fn dialog_top(theme: &Theme, inner_w: usize, title: &str) -> String {
    // `╭`, `─`, the title, the filler and `╮`: the filler is what closes the
    // width, so it absorbs whatever the title did not use.
    let title = fit(title, inner_w.saturating_sub(2));
    let used = 2 + visible_len(&title);
    let filler = glyph::H.repeat((inner_w + 1).saturating_sub(used));
    theme.paint(Style::Border, &format!("{}{}{}{}{}", glyph::TL, glyph::H, title, filler, glyph::TR))
}

/// `╰────────────╯`.
fn dialog_bottom(theme: &Theme, inner_w: usize) -> String {
    theme.paint(Style::Border, &format!("{}{}{}", glyph::BL, glyph::H.repeat(inner_w), glyph::BR))
}

/// Games that live under `root` (a path prefix that ends in a separator, so
/// `/roms` does not claim `/roms-backup`).
fn games_under(games: &[GameEntry], root: &Path) -> usize {
    let prefix = root.display().to_string();
    games
        .iter()
        .filter(|g| {
            let p = g.path.display().to_string();
            p == prefix || p.starts_with(&format!("{prefix}{}", std::path::MAIN_SEPARATOR))
        })
        .count()
}

/// Feedback from the last action, anchored above the footer rule. Empty means
/// nothing happened yet, and the row is still painted so the frame geometry
/// never changes (a row appearing/disappearing would move everything below it).
/// Message on the left (the last thing that happened) and, permanently on the
/// right, how the covers are reaching the terminal: that is a *state*, not a
/// key, and it was buried in the middle of the legend.
fn status_line(input: &FrameInput<'_>, theme: &Theme, layout: Layout, inner: usize) -> String {
    let left = fit(input.status, inner.saturating_sub(visible_len(input.art_mode) + 1));
    let right = if input.art_mode.is_empty() { String::new() } else { theme.paint(Style::Dim, input.art_mode) };
    let gap = inner.saturating_sub(visible_len(&left) + visible_len(&right));
    pad(layout, format!("{left}{}{right}", " ".repeat(gap)))
}

fn rule(layout: Layout) -> String {
    glyph::H.repeat(layout.width)
}

/// Indent by the margin, then pad (never truncate) to the full frame width.
fn pad(layout: Layout, content: String) -> String {
    let indent = " ".repeat(layout.margin);
    let used = layout.margin + visible_len(&content);
    format!("{indent}{content}{}", " ".repeat(layout.width.saturating_sub(used)))
}

/// One line: brand on the left, library counts on the right.
///
/// It used to be a three-row double box (`╔═╗` / bar / `╚═╝`) that spent more
/// screen on decoration than on words and competed with the covers, which are
/// the only thing in this UI worth looking at. The thin rule under the chips is
/// what separates the chrome from the art.
fn header(inner: usize, theme: &Theme, input: &FrameInput<'_>, shown: usize) -> String {
    let brand = format!(
        "{} {} {}",
        theme.paint(Style::Accent, "ROSADECK"),
        theme.paint(Style::Dim, "·"),
        theme.paint(Style::Gold, "RETRO LIBRARY"),
    );
    let stats = format!(
        "{} {} · {} {} · {} {}",
        theme.paint(Style::Plain, &shown.to_string()),
        theme.paint(Style::Dim, "shown"),
        theme.paint(Style::Cyan, &input.games.len().to_string()),
        theme.paint(Style::Dim, "roms"),
        theme.paint(Style::Gold, &input.favorites.len().to_string()),
        theme.paint(Style::Dim, "fav"),
    );
    // En una terminal diminuta los recuentos son lo prescindible: el nombre
    // gana y la línea se corta sin pegarse a los números.
    if visible_len(&brand) + visible_len(&stats) + 1 > inner {
        return fit(&brand, inner);
    }
    let gap = inner - visible_len(&brand) - visible_len(&stats);
    fit(&format!("{brand}{}{stats}", " ".repeat(gap)), inner)
}

/// Platform chips with live counts + the search query on the right.
fn chips(input: &FrameInput<'_>, theme: &Theme, inner: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    let all_label = format!(" ALL {} ", input.games.len());
    parts.push(if input.browser.platform.is_none() {
        theme.paint(Style::Selected, &all_label)
    } else {
        theme.paint(Style::Dim, &all_label)
    });
    for p in Platform::all() {
        let count = input.games.iter().filter(|g| g.platform == *p).count();
        if count == 0 {
            continue;
        }
        let label = format!(" {} {} ", short_label(*p), count);
        parts.push(if input.browser.platform == Some(*p) {
            theme.paint(Style::Selected, &label)
        } else {
            theme.paint(Style::Cyan, &label)
        });
    }
    let left = parts.join(" ");
    // The roots editor has its own centred window (v26): typing a directory in
    // the corner of the screen, next to the counts, was unreadable and easy to
    // miss.
    let search = if input.browser.query.is_empty() {
        theme.paint(Style::Dim, "/ find")
    } else {
        format!("{}{}", theme.paint(Style::Dim, "/ "), theme.paint(Style::Gold, &input.browser.query))
    };
    // Fill the row exactly: chips on the left, the query on the right, the
    // blinking cursor (a reserved column) only while searching.
    let cursor = if input.browser.searching {
        format!("{}", theme.paint(Style::Gold, "_"))
    } else {
        String::new()
    };
    let gap = inner.saturating_sub(visible_len(&left) + visible_len(&search) + visible_len(&cursor));
    fit(&format!("{left}{}{search}{cursor}", " ".repeat(gap)), inner)
}

/// Up to three initials from the title (`Luigi's Mansion` → `LM`), used by the
/// generated placeholder art so a shelf of boxes looks like the games it holds.
pub fn initials(title: &str, platform: Platform) -> String {
    let joined: String = title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2 && !w.chars().all(|c| c.is_ascii_digit()))
        .take(3)
        .filter_map(|w| w.chars().next())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if joined.is_empty() {
        short_label(platform).to_owned()
    } else {
        joined
    }
}

fn short_label(p: Platform) -> &'static str {
    match p {
        Platform::Snes => "SNES",
        Platform::N64 => "N64",
        Platform::GameCube => "GC",
        Platform::Wii => "WII",
        Platform::Nintendo3DS => "3DS",
    }
}

/// The cover band: one row of `layout.cols` covers, panned so the selection
/// stays on screen (a carousel).
///
/// A cell whose cover is a real file becomes an image plan — displayed at the
/// terminal's native resolution by the graphics protocol — and its cover area
/// is left empty here; every other cell draws generated half-block art, so the
/// row never has holes.
fn carousel(
    input: &FrameInput<'_>,
    layout: Layout,
    theme: &Theme,
    cache: &mut CoverCache,
    games: &[&GameEntry],
    use_images: bool,
    images: &mut Vec<Plan>,
) -> Vec<String> {
    let height = layout.cover_h + 4; // top border, cover, bottom border, title, meta
    if games.is_empty() {
        let msg = if input.games.is_empty() {
            "no ROMs — put them in ~/roms/<platform>/ or set ROSADECK_ROMS"
        } else {
            "nothing matches this filter or search"
        };
        let text = theme.paint(Style::Dim, msg);
        let text = fit(&text, layout.width.saturating_sub(layout.margin));
        return (0..height)
            .map(|i| if i == height / 2 { pad(layout, text.clone()) } else { pad(layout, String::new()) })
            .collect();
    }
    // The fan: the focused game in the middle, its neighbours overlapping it,
    // narrower and fainter the further out they go.
    let available = layout.width.saturating_sub(2 * layout.margin);
    let plan = crate::layout::fan(available, layout.cover_w, layout.cover_h, games.len());
    let backdrop = theme.palette.background;
    let focus = input.browser.cursor;

    // Cards in drawing order, left to right: outermost left first, then the
    // focus, then the right side from the closest outwards. Painting the focus
    // last is what puts it on top of the neighbours.
    let mut order: Vec<Card> = plan.iter().copied().filter(|c| c.side < 0).collect();
    order.sort_by_key(|c| usize::MAX - c.distance);
    order.extend(plan.iter().copied().filter(|c| c.side == 0));
    let mut right: Vec<Card> = plan.iter().copied().filter(|c| c.side > 0).collect();
    right.sort_by_key(|c| c.distance);
    order.extend(right);

    // Each card knows which game it shows: the focus is the cursor, the rest
    // step away from it on both sides. A slot with no game behind it is simply
    // not drawn: at the ends of the library the shelf shows the games there are,
    // four cards instead of seven, and `band_offset` re-centres what is drawn.
    // Repeating the closest game to keep the shape (what this did before) shows
    // the same cover three times, and drawing an empty frame shows a hole.
    let mut slots: Vec<(Card, Option<&GameEntry>)> = Vec::with_capacity(order.len());
    for card in order {
        let index = match card.side {
            0 => Some(focus),
            s if s < 0 => focus.checked_sub(card.distance),
            _ => Some(focus + card.distance),
        };
        let Some(game) = index.and_then(|i| games.get(i)) else { continue };
        slots.push((card, Some(game)));
    }
    let drawn: Vec<Card> = slots.iter().map(|(c, _)| *c).collect();
    let band_offset = crate::layout::band_offset(layout.margin, available, &drawn);
    let mut rendered: Vec<CardArt> = Vec::with_capacity(slots.len());
    let mut x = band_offset;
    for (card, game) in slots {
        let selected = card.side == 0;
        // Depth drives the treatment: the further back, the more it blurs,
        // greys out and melts into the page.
        let treatment = if selected { Treatment::Crisp } else { Treatment::Recessed(card.depth) };
        // A neighbour shows its outer edge; the rest hides behind the closer card.
        let crop = match card.side {
            0 => Crop::FULL,
            s if s < 0 => Crop::left(card.width),
            _ => Crop::right(card.width),
        };
        let art_w = card.full_width();
        // Unreachable today (slots without a game are skipped above), but a hole
        // must never be drawn as a card.
        let Some(game) = game else { continue };
        let art = cache.art_path(&game.path);
        let mut lines = Vec::with_capacity(card.rows);
        if use_images && art.is_some() {
            // Native resolution: kitty scales and crops the source rectangle, so
            // the card keeps the artwork's scale instead of being squeezed.
            //
            // The cards have no frame, so the artwork starts at the card's own
            // column; `col` is the 1-based cursor cell (kitty renders a
            // placement at the cursor).
            let art_col = x;
            images.push(plan_for_cropped(
                art.as_deref().unwrap(),
                (art_col + 1) as u16,
                // The art starts on the card's first row; +1 to 1-based.
                (GRID_TOP + 1 + card.inset) as u16,
                art_w as u16,
                card.rows as u16,
                crop,
                treatment,
                backdrop,
            ));
        } else {
            // Real artwork when there is one, a generated cover otherwise —
            // cropped the same way, so the fan looks right either way.
            lines = cache
                .cover_cropped(
                    &game.path,
                    art_w,
                    card.rows * 2,
                    game.platform,
                    &game.title,
                    treatment,
                    backdrop,
                    crop,
                )
                .rows(theme);
        }
        let label = if selected {
            let w = card.width;
            let fav = input.favorites.contains(&game.id);
            let played = input.played.get(&game.id).copied().unwrap_or(0);
            let icons = Icons::detect();
            let icon = |off: &str, on: &str| {
                if fav { theme.paint(Style::Gold, on) } else { theme.paint(Style::Dim, off) }
            };
            let star = icon(icons.fav_off, icons.fav_on);
            let played_icon = theme.paint(Style::Dim, icons.played);
            let played_text = match played {
                0 => theme.paint(Style::Dim, "nunca"),
                secs => theme.paint(Style::Accent, &played_time(secs)),
            };
            // One row is exactly the card's columns: the padding `fit` does not
            // add is what keeps the band rectangular.
            let row = |body: String| {
                let body = fit(&body, w);
                let fill = " ".repeat(w.saturating_sub(visible_len(&body)));
                format!("{body}{fill}")
            };
            let room = w.saturating_sub(visible_len(&star) + 1);
            Some((
                row(format!(
                    "{star} {}",
                    theme.paint(Style::Gold, &ellipsize(&game.title, room))
                )),
                row(format!(
                    "{} {}  {played_icon} {played_text}",
                    theme.paint(Style::Cyan, &game.platform.label()),
                    theme.paint(Style::Dim, &ellipsize(&game.region.as_deref().unwrap_or(""), 18)),
                )),
            ))
        } else {
            None
        };
        rendered.push(CardArt { card, lines, label });
        x += card.total_width() + CARD_GAP;
    }

    // Assemble the band row by row: every card answers with exactly its own
    // columns for every row, and blank for the rows its shorter art does not
    // reach — which is what makes the neighbours recede without any overlap
    // arithmetic.
    // The text band starts at the same column as the first image plan: two
    // different offsets here are what makes the covers drift away from the
    // cards (they used to start at the plain margin while the plans centred).
    let mut rows: Vec<String> = (0..height).map(|_| " ".repeat(band_offset)).collect();
    for (i, art) in rendered.iter().enumerate() {
        let card = art.card;
        for (band_row, row) in rows.iter_mut().enumerate() {
            // A receding card is shorter, so it is drawn `inset` rows lower: its
            // borders and art sit inside the band, not against its top. Without
            // this the neighbour's image — which does use the inset — covered
            // its own bottom border, one row below where the text drew it.
            //
            // The rows above the card still get its columns (blank): skipping
            // them would shift every other card left by that card's width on
            // those rows, so the band would be a different shape on each row.
            let y = band_row.wrapping_sub(card.inset);
            // No frame: the art occupies the card's columns and the labels come
            // right under it. A box per card turned the shelf into a row of
            // separate pictures, which is exactly what a carousel is not.
            let piece = if band_row < card.inset {
                " ".repeat(card.total_width())
            } else if y < card.rows {
                let body = fit(&art.lines.get(y).cloned().unwrap_or_default(), card.width);
                // Pad: with the graphics protocol the body is empty (the image
                // covers those cells), and without the padding the band would be
                // short on every row that has one.
                let fill = " ".repeat(card.width.saturating_sub(visible_len(&body)));
                format!("{body}{fill}")
            } else if let Some((title, meta)) = &art.label {
                match y {
                    y if y == card.rows => title.clone(),
                    y if y == card.rows + 1 => meta.clone(),
                    _ => " ".repeat(card.total_width()),
                }
            } else {
                " ".repeat(card.total_width())
            };
            row.push_str(&fit(&piece, card.total_width()));
        }
        if i + 1 < rendered.len() {
            for row in rows.iter_mut() {
                row.push_str(&" ".repeat(CARD_GAP));
            }
        }
    }
    for row in rows.iter_mut() {
        row.push_str(&" ".repeat(layout.width.saturating_sub(visible_len(row))));
    }
    rows
}

/// One rendered card of the fan: its geometry, its art rows and — only for the
/// focused game — its two label rows.
struct CardArt {
    card: Card,
    lines: Vec<String>,
    label: Option<(String, String)>,
}


/// `cover <file> image` / `cover <file> blocks` / `cover generated`.
///
/// The mode matters: an image is transmitted to the terminal at its native
/// resolution, blocks are one pixel per character column.
/// What the detail pane says about the artwork.
///
/// Existence is not enough: the note names the file only when it really
/// decodes. A cover the decoder rejects is reported as unreadable instead of
/// claiming a picture the shelf is not showing — that is the difference between
/// "the JPEG is unsupported" and the truth.
/// The cover verdict, and only when there is one: a file that cannot be decoded
/// has to be named, because then the shelf is drawing a generated placeholder
/// instead of it. A cover that decodes says nothing.
fn cover_alert(theme: &Theme, cache: &mut CoverCache, g: &GameEntry) -> String {
    let Some(path) = cache.art_path(&g.path) else { return String::new() };
    // Memoized per file: resizing artwork here cost 150 ms per keypress.
    if cache.decodes(&path) {
        return String::new();
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    format!(
        "{} {}",
        theme.paint(Style::Dim, "cover"),
        theme.paint(Style::Alert, &format!("{name} unreadable"))
    )
}

/// One line about the selected game: what can be played, and how big it is.
///
/// The title, the platform and the play count are already painted under the
/// cover (see `carousel`), so repeating them here was noise; the ROM path went
/// away with them and only shows up when a launch fails, in the status line.
fn detail(input: &FrameInput<'_>, layout: Layout, theme: &Theme, game: Option<&&GameEntry>, inner: usize, cache: &mut CoverCache) -> Vec<String> {
    let line = match game {
        Some(g) => {
            // Sin etiquetas: con un número y un nombre ya se entiende, y
            // `size ·` / `emulator ·` sólo tenían sentido cuando la línea tenía
            // cuatro campos.
            let mut parts = vec![
                theme.paint(Style::Plain, &human_size(g.size)),
                emulator_note(theme, &input.emulators, g.platform),
            ];
            // Silence unless there is something to say: a cover that decodes is
            // the normal case, and a file name on every row is noise.
            let broken = cover_alert(theme, cache, g);
            if !broken.is_empty() {
                parts.push(broken);
            }
            parts.join(&theme.paint(Style::Dim, " · "))
        }
        None => theme.paint(Style::Dim, "nothing selected"),
    };
    vec![pad(layout, fit(&line, inner))]
}

/// `emulator` field of the detail pane: green tick when the binary exists, a
/// loud "NOT INSTALLED" plus the package hint when it does not. Pressing Enter
/// on a game that cannot launch must not look like a bug.
fn emulator_note(theme: &Theme, list: &[EmulatorStatus], platform: Platform) -> String {
    let avail = emulators::availability(list, platform.default_emulator());
    if avail.playable() {
        theme.paint(Style::Accent, &avail.label())
    } else {
        theme.paint(Style::Alert, &avail.label())
    }
}

fn legend(theme: &Theme, inner: usize, input: &FrameInput<'_>) -> String {
    let selected = input
        .browser
        .view(input.games, input.favorites)
        .get(input.browser.cursor)
        .map(|g| g.platform);
    let play_hint = match selected {
        Some(p) if !emulators::availability(input.emulators, p.default_emulator()).playable() => "needs emulator",
        _ => "play",
    };
    // Only the keys that are not obvious from looking at the screen. Platform
    // filters, rescan, theme reload and the artwork mode live in the docs and in
    // `docs/integrations/rosadeck-keybind.md`; ten pairs of keys in one row was
    // more of a wall than a legend.
    let pairs = [("←→", "move"), ("⏎", play_hint), ("f", "fav"), ("/", "find"), ("d", "roms"), ("q", "quit")];
    let mut s = String::new();
    for (i, (k, label)) in pairs.iter().enumerate() {
        if i > 0 {
            s.push_str("  ");
        }
        s.push_str(&theme.paint(Style::Accent, k));
        s.push(' ');
        s.push_str(&theme.paint(Style::Dim, label));
    }
    fit(&s, inner)
}

fn fit(line: &str, max: usize) -> String {
    if visible_len(line) <= max {
        line.to_owned()
    } else {
        rosadeck_tui_frame::truncate_visible(line, max)
    }
}

fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    if max == 1 {
        return s.chars().take(1).collect();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// Play time as the largest unit that reads well: `45s`, `12m`, `3h 20m`.
///
/// Seconds below a minute, minutes below an hour, hours and minutes above it:
/// one unit keeps the shelf's label short, and hours alone would round a long
/// afternoon down to nothing.
pub fn played_time(secs: u64) -> String {
    const MIN: u64 = 60;
    const HOUR: u64 = 60 * MIN;
    match secs {
        s if s < MIN => format!("{s}s"),
        m if m < HOUR => format!("{}m", secs / MIN),
        h if h < 100 * HOUR => format!("{}h {}m", secs / HOUR, (secs % HOUR) / MIN),
        _ => format!("{}h", secs / HOUR),
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else if v >= 10.0 {
        // Sin decimales de más: «229 MiB» se lee mejor que «229.0 MiB».
        format!("{v:.0} {}", UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn games() -> Vec<GameEntry> {
        vec![
            GameEntry {
                id: "1".into(),
                title: "Luigi's Mansion".into(),
                platform: Platform::Wii,
                path: "/roms/wii/Luigi's Mansion (Europe) (En,Fr,De,Es,It).rvz".into(),
                region: Some("Europe".into()),
                size: 1_500_000_000,
            },
            GameEntry { id: "2".into(), title: "Mario Kart 7".into(), platform: Platform::Nintendo3DS, path: "/roms/3ds/Mario Kart 7 (USA).cci".into(), region: None, size: 1_200_000 },
            GameEntry {
                id: "3".into(),
                title: "A Very Long Game Title That Should Ellipsize Gracefully Instead".into(),
                platform: Platform::Snes,
                path: "/roms/snes/long.sfc".into(),
                region: Some("USA".into()),
                size: 524_288,
            },
            GameEntry { id: "4".into(), title: "Metroid Prime".into(), platform: Platform::Nintendo3DS, path: "/roms/3ds/mp.3ds".into(), region: None, size: 900 },
        ]
    }

    /// Render one frame as text (no image plans), like `--dump` does.
    fn frame_at(w: usize, h: usize, browser: &Browser, favorites: &[String], theme: &Theme, count: usize) -> String {
        let all = games();
        let layout = Layout::compute(w, h, count);
        let played = HashMap::from([("1".to_owned(), 5u64)]);
        let mut cache = CoverCache::new();
        build(&FrameInput::base(&browser, &all, favorites, &played), layout, theme, &mut cache, false).text
    }

    /// The fixture games, living under `dir` (so the window's `is_dir` check
    /// sees a real directory instead of guessing).
    fn games_in(dir: &Path) -> Vec<GameEntry> {
        games()
            .into_iter()
            .map(|mut g| {
                g.path = dir.join(g.path.strip_prefix("/roms").expect("fixture bajo /roms"));
                g
            })
            .collect()
    }

    /// Render a frame with the roots editor open and the given roots.
    fn dialog_frame(w: usize, h: usize, roots: &[PathBuf], text: &str, all: &[GameEntry]) -> Frame {
        let layout = Layout::compute(w, h, all.len());
        let mut b = Browser::new();
        b.roots_input = true;
        b.roots_text = text.to_owned();
        let mut cache = CoverCache::new();
        build(
            &FrameInput { roots, ..FrameInput::base(&b, all, &[], &HashMap::new()) },
            layout,
            &Theme::plain(),
            &mut cache,
            true,
        )
    }

    /// The editor is a window in the middle of the screen, not a line in the
    /// corner: centred horizontally and vertically, square, and the shelf behind
    /// it keeps its geometry (no line ever changes width).
    #[test]
    fn the_roots_editor_is_a_centred_window() {
        // A real cover on disk, so "the covers leave while the window is open"
        // is a claim about plans that exist, not about an empty shelf.
        let dir = std::env::temp_dir().join("rosadeck-dialog-centre");
        std::fs::remove_dir_all(&dir).ok();
        let wii = dir.join("roms/wii");
        let covers = dir.join("roms/covers");
        std::fs::create_dir_all(&wii).unwrap();
        std::fs::create_dir_all(&covers).unwrap();
        let rom = wii.join("Luigi's Mansion (Europe).rvz");
        std::fs::write(&rom, b"rom").unwrap();
        image::RgbImage::from_pixel(32, 48, image::Rgb([9, 9, 9])).save(covers.join("Luigi's Mansion.png")).unwrap();
        let all = vec![GameEntry {
            id: "im".into(),
            title: "Luigi's Mansion".into(),
            platform: Platform::Wii,
            path: rom,
            region: None,
            size: 10,
        }];
        let roots = vec![dir.join("roms")];
        let layout = Layout::compute(100, 30, all.len());
        let theme = Theme::builtin(crate::theme::Depth::Truecolor, true);
        let draw = |browser: &Browser| {
            let mut cache = CoverCache::new();
            build(
                &FrameInput { roots: &roots, ..FrameInput::base(browser, &all, &[], &HashMap::new()) },
                layout,
                &theme,
                &mut cache,
                true,
            )
        };
        let mut b = Browser::new();
        b.roots_input = true;
        let frame = draw(&b);
        let lines: Vec<&str> = frame.text.lines().collect();
        // Every row of the frame is still exactly the drawn width.
        for (i, line) in frame.text.lines().enumerate() {
            assert_eq!(visible_len(line), layout.width, "fila {i}: {line:?}");
        }
        let top = lines.iter().position(|l| l.contains('╭')).expect("la ventana tiene borde superior");
        let bottom = lines.iter().rposition(|l| l.contains('╰')).expect("la ventana tiene borde inferior");
        let h = bottom - top + 1;
        assert_eq!(top, (layout.height - h) / 2, "la ventana no está centrada en vertical");
        let top_row = lines[top];
        let left_pad = top_row.chars().take_while(|c| c.is_whitespace()).count();
        let right_pad = top_row.chars().rev().take_while(|c| c.is_whitespace()).count();
        assert!(layout.width - left_pad - right_pad > 20, "ventana demasiado estrecha: {top_row:?}");
        assert_eq!(left_pad, right_pad, "la ventana no está centrada en horizontal: {top_row:?}");
        for line in &lines[top..=bottom] {
            let indent = line.chars().take_while(|c| c.is_whitespace()).count();
            assert_eq!(indent, left_pad, "fila fuera de caja: {line:?}");
        }
        // The covers leave while the window is open: a text window cannot be
        // painted on top of a transmitted image.
        assert!(draw(&Browser::new()).images.len() == 1, "el estante tiene una portada que enviar");
        assert!(frame.images.is_empty(), "con la ventana abierta no se envían portadas");
        // And they come back when it closes.
        let closed = draw(&Browser::new());
        assert!(!closed.text.contains('╭'), "la ventana se queda al cerrar");
        assert_eq!(closed.images.len(), 1, "al cerrar vuelve la portada");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The window says what each root really contributes, and admits the ones
    /// that are not there.
    #[test]
    fn the_window_lists_every_root_with_its_count() {
        // Real directories, because the window says which roots are missing:
        // a unit test that invented paths would only ever see "no existe".
        let base = std::env::temp_dir().join("rosadeck-dialog-test");
        std::fs::remove_dir_all(&base).ok();
        let real = base.join("roms");
        let empty = base.join("vacia");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(&empty).unwrap();
        // Four games under `real`, plus one that only looks like it belongs.
        let mut all = games_in(&real);
        all.push(GameEntry {
            id: "9".into(),
            title: "Backup".into(),
            platform: Platform::Wii,
            path: base.join("roms-backup/wii/old.rvz"),
            region: None,
            size: 1,
        });
        let roots = vec![real.clone(), empty.clone(), base.join("nunca-montado")];
        let frame = dialog_frame(100, 30, &roots, "/mnt/roms2", &all);
        assert!(frame.text.contains("4 juegos"), "`roms` tiene 4 juegos, no 5: {}", frame.text);
        assert!(frame.text.contains("no existe"), "un disco que no está se dice: {}", frame.text);
        assert!(frame.text.contains("0 juegos"), "una carpeta vacía se dice: {}", frame.text);
        assert!(frame.text.contains("/mnt/roms2"), "lo que se escribe se ve: {}", frame.text);
        assert!(frame.text.contains("Esc cancela"), "las teclas se dicen: {}", frame.text);
        // Nothing invented: a directory that is not a root is not in the window.
        assert!(!frame.text.contains("roms-backup"), "una raíz que no está no aparece: {}", frame.text);
        std::fs::remove_dir_all(&base).ok();
    }

    /// Terminales ridículos: la ventana se niega a salir, pero el frame nunca
    /// se rompe (ni se desborda ni hace panic).
    #[test]
    fn a_tiny_terminal_gets_no_window_but_no_damage() {
        let all = games();
        let roots: Vec<PathBuf> = (0..30).map(|i| PathBuf::from(format!("/mnt/roms{i}"))).collect();
        for (w, h) in [(20usize, 12usize), (30, 10), (40, 9), (60, 8), (80, 24), (189, 46)] {
            let layout = Layout::compute(w, h, all.len());
            let frame = dialog_frame(w, h, &roots, "/mnt/un/ruta/muy/larga/que/no/cabe/ni/de/lejos", &all);
            for (i, line) in frame.text.lines().enumerate() {
                assert_eq!(visible_len(line), layout.width, "{w}x{h} fila {i}: {line:?}");
            }
            assert_eq!(frame.text.lines().count(), layout.height, "{w}x{h}: filas de más o de menos");
        }
        // And when it truly cannot fit, it says nothing at all rather than
        // drawing a box over the list it is meant to show. `Layout::compute`
        // clamps to a usable minimum, so these geometries are built by hand.
        for (w, h) in [(40usize, 6usize), (24, 20), (40, 5), (30, 4)] {
            let layout = Layout { width: w, height: h, margin: 1, cover_w: 8, cover_h: 5, cols: 1, drawn_width: 8 };
            let mut b = Browser::new();
            b.roots_input = true;
            let (top, rows) = roots_dialog(
                &FrameInput { roots: &roots[..1], ..FrameInput::base(&b, &all, &[], &HashMap::new()) },
                &Theme::plain(),
                layout,
            );
            assert!(rows.is_empty() && top == 0, "{w}x{h}: ventana imposible dibujada: {rows:?}");
        }
    }

    #[test]
    fn every_line_is_exactly_the_drawn_width() {
        for (w, h) in [(20usize, 12usize), (40, 16), (60, 20), (80, 24), (100, 30), (189, 46), (300, 80)] {
            let layout = Layout::compute(w, h, 4);
            let browser = Browser::new();
            let frame = frame_at(w, h, &browser, &[], &Theme::plain(), 4);
            for (i, line) in frame.lines().enumerate() {
                assert_eq!(visible_len(line), layout.width, "line {i} at {w}x{h}: {line:?}");
            }
        }
    }

    #[test]
    fn styled_frame_never_exceeds_terminal_width() {
        let theme = Theme::builtin(crate::theme::Depth::Truecolor, true);
        for (w, h) in [(60usize, 20usize), (80, 24), (132, 43), (189, 46)] {
            let browser = Browser::new();
            let frame = frame_at(w, h, &browser, &[], &theme, 4);
            for line in frame.lines() {
                assert!(visible_len(line) <= w, "styled overflow at {w}: {line:?}");
            }
        }
    }

    #[test]
    fn frame_has_header_grid_detail_and_legend() {
        let theme = Theme::builtin(crate::theme::Depth::Truecolor, true);
        let mut browser = Browser::new();
        browser.set_geometry(7, 2);
        browser.cursor = 1; // sorted by title: Luigi's Mansion
        let frame = frame_at(189, 46, &browser, &[], &theme, 4);
        assert!(frame.contains("ROSADECK"));
        assert!(frame.contains("RETRO LIBRARY"));
        assert!(frame.contains("Luigi's Mansion"), "el título se lee bajo la portada, no repetido en mayúsculas en el pie");
        assert!(frame.contains("1.4 GiB"), "size formatted");
        assert!(frame.contains("dolphin"), "emulator shown");
        assert!(frame.contains("move"), "legend present");
        assert!(!frame.contains("/roms/wii/"), "el pie no imprime la ruta del ROM");
        assert!(frame.contains('▀'), "cover half-blocks rendered");
        assert!(!frame.contains('╔'), "la cabecera es una línea, sin caja: el marco era ruido");
        assert!(frame.lines().next().unwrap().contains("ROSADECK"), "y va en la primera fila");
    }

    #[test]
    fn selected_cell_is_highlighted() {
        let theme = Theme::builtin(crate::theme::Depth::Truecolor, true);
        let mut browser = Browser::new();
        browser.set_geometry(3, 2);
        let first = frame_at(189, 46, &browser, &[], &theme, 4);
        browser.cursor = 2;
        let third = frame_at(189, 46, &browser, &[], &theme, 4);
        assert_ne!(first, third, "selection must change the frame");
        assert!(third.contains("Mario Kart 7"), "la etiqueta de la tarjeta enfocada sigue al cursor");
        assert!(!third.contains("Luigi"), "la anterior ya no es la enfocada");
    }

    #[test]
    fn empty_library_explains_itself() {
        let layout = Layout::compute(80, 24, 0);
        let browser = Browser::new();
        let mut cache = CoverCache::new();
        let frame = build(
            &FrameInput::base(&browser, &[], &[], &HashMap::new()),
            layout,
            &Theme::plain(),
            &mut cache,
            false,
        )
        .text;
        assert!(frame.contains("no ROMs"));
        for line in frame.lines() {
            assert_eq!(visible_len(line), layout.width);
        }
    }

    #[test]
    fn filter_with_no_matches_explains_itself() {
        let layout = Layout::compute(120, 30, 4);
        let mut browser = Browser::new();
        browser.query = "zzzz".into();
        let mut cache = CoverCache::new();
        let frame = build(
            &FrameInput::base(&browser, &games(), &[], &HashMap::new()),
            layout,
            &Theme::plain(),
            &mut cache,
            false,
        )
        .text;
        assert!(frame.contains("nothing matches"));
    }

    #[test]
    fn chips_show_counts_and_selection() {
        let theme = Theme::builtin(crate::theme::Depth::Ansi256, false);
        let mut browser = Browser::new();
        let all = frame_at(189, 46, &browser, &[], &theme, 4);
        assert!(all.contains("ALL 4"));
        assert!(all.contains("3DS 2"), "per-platform counts");
        browser.platform = Some(Platform::Wii);
        let wii = frame_at(189, 46, &browser, &[], &theme, 4);
        assert!(wii.contains("WII 1"));
    }

    #[test]
    fn a_cover_that_cannot_be_decoded_is_named_as_unreadable() {
        // The note used to name any file that existed, so a broken or
        // unsupported image claimed to be on screen while the shelf drew a
        // generated placeholder.
        let dir = std::env::temp_dir().join("rosadeck-frame-art-broken");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("wii")).unwrap();
        std::fs::create_dir_all(dir.join("covers")).unwrap();
        // Right name, right extension, not an image.
        std::fs::write(dir.join("covers/Broken Cover.jpg"), b"\xff\xd8\xff\xe0 truncated").unwrap();
        let games = vec![GameEntry {
            id: "b".into(),
            title: "Broken Cover".into(),
            platform: Platform::Wii,
            path: dir.join("wii/Broken Cover.rvz"),
            region: None,
            size: 1,
        }];
        let layout = Layout::compute(120, 40, 1);
        let theme = Theme::plain();
        let mut cache = CoverCache::new();
        let frame = build(&FrameInput::base(&Browser::new(), &games, &[], &HashMap::new()), layout, &theme, &mut cache, false).text;
        assert!(frame.contains("Broken Cover.jpg"), "el fichero se nombra: {frame}");
        assert!(frame.contains("unreadable"), "and not passed off as shown: {frame}");

        // A real PNG in the same spot must NOT be called unreadable.
        std::fs::create_dir_all(dir.join("wii2")).unwrap();
        image::RgbImage::from_pixel(8, 12, image::Rgb([0, 255, 0])).save(dir.join("covers/Good Cover.png")).unwrap();
        let good = vec![GameEntry {
            id: "g".into(),
            title: "Good Cover".into(),
            platform: Platform::Wii,
            path: dir.join("wii2/Good Cover.rvz"),
            region: None,
            size: 1,
        }];
        let frame = build(&FrameInput::base(&Browser::new(), &good, &[], &HashMap::new()), layout, &theme, &mut cache, false).text;
        assert!(!frame.contains("Good Cover.png"), "una portada que decodifica no ocupa sitio: {frame}");
        assert!(!frame.contains("unreadable"), "y no se le llama ilegible: {frame}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The fan: the focused cover sits in the middle at full size, its
    /// neighbours overlap it, are narrower, start lower and are faded.
    #[test]
    fn the_focus_is_the_top_card_and_its_neighbours_fan_out() {
        let all = games();
        let theme = Theme::builtin(crate::theme::Depth::Truecolor, true);
        let layout = Layout::compute(rosadeck_tui_frame::safe_width(120), 40, all.len());
        let available = layout.width.saturating_sub(2 * layout.margin);
        let plan = crate::layout::fan(available, layout.cover_w, layout.cover_h, all.len());
        assert!(plan.len() >= 3, "needs neighbours to fan out: {plan:?}");

        let mut browser = Browser::new();
        browser.set_geometry(layout.cols, 1);
        browser.cursor = 1;
        let mut cache = CoverCache::new();
        let frame = build(&FrameInput::base(&browser, &all, &[], &HashMap::new()), layout, &theme, &mut cache, false);

        // Visible glyphs per row, indexed by *column* (the old test sliced the
        // string by byte, which breaks on multi-byte half blocks).
        let rows: Vec<Vec<char>> = frame
            .text
            .lines()
            .map(|l| {
                rosadeck_tui_frame::tokens_for_tests(l)
                    .into_iter()
                    // Escape tokens carry no column: they are styling.
                    .filter(|t| !t.text.starts_with('\x1b'))
                    .filter_map(|t| t.text.chars().next())
                    .collect()
            })
            .collect();
        let focus = plan.iter().find(|c| c.side == 0).copied().unwrap();
        let neighbour = plan.iter().find(|c| c.side == -1).copied().unwrap();
        assert!(neighbour.width < focus.width, "neighbours are narrower: {neighbour:?}");
        assert!(neighbour.rows < focus.rows, "and shorter: {neighbour:?}");
        assert!(neighbour.hidden > 0, "and partially hidden behind the focus: {neighbour:?}");

        // On the first art row the focus already shows artwork while a
        // neighbour still shows the blank inset above its own art.
        let first_art = GRID_TOP + 1;
        let focus_row: String = rows[first_art][layout.margin + focus.total_width() / 2..].iter().collect();
        let neighbour_art_row = first_art + neighbour.inset;
        // Where the band really starts: the first card's border, found by the
        // same rule the frame uses.
        let available = layout.width - 2 * layout.margin;
        let drawn: Vec<crate::layout::Card> = vec![focus, neighbour];
        let at = crate::layout::band_offset(layout.margin, available, &drawn) + 1;
        let neighbour_row: String = rows[neighbour_art_row][at..at + neighbour.width].iter().collect();
        assert!(focus_row.contains(glyph::HALF), "the focus has artwork on the first row: {focus_row:?}");
        assert!(neighbour_row.contains(glyph::HALF), "and so does its neighbour, lower down: {neighbour_row:?}");

        // 2. Same file, same card: the neighbour's pixels really are faded.
        let rom = &all[1].path;
        let crisp = cache.get_treated(Some(rom), 12, 12, crate::cover::Treatment::Crisp, theme.palette.background);
        let behind = cache.get_treated(Some(rom), 12, 12, crate::cover::Treatment::Recessed(50), theme.palette.background);
        let chroma = |c: &crate::cover::Cover| {
            let mut total = 0u64;
            for p in &c.px {
                let (r, g, b) = (((p >> 16) & 0xff) as u32, ((p >> 8) & 0xff) as u32, (p & 0xff) as u32);
                total += (r.max(g).max(b) - r.min(g).min(b)) as u64;
            }
            total / c.px.len().max(1) as u64
        };
        if let (Some(a), Some(b)) = (&crisp, &behind) {
            assert!(chroma(b) < chroma(a), "neighbours fade: {} -> {}", chroma(a), chroma(b));
        }

        // 3. The whole band still fits the terminal, every row.
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(row.len(), layout.width, "row {i} is exactly the drawn width");
        }
    }

    /// The image plans must tile the band exactly: each card's *visible* cells,
    /// in order, without overlapping its neighbour.
    ///
    /// This is the invariant a visible overlap bug would break: the plan used to
    /// ask kitty for the card's whole width while only the visible slice had
    /// cells, so every neighbour's artwork spilled over the focus.
    #[test]
    fn image_plans_tile_the_band_without_overlapping() {
        // Real cover files on disk, so every card takes the image path.
        let dir = std::env::temp_dir().join("rosadeck-frame-fan-plans");
        std::fs::remove_dir_all(&dir).ok();
        let wii = dir.join("wii");
        let covers = dir.join("covers");
        std::fs::create_dir_all(&wii).unwrap();
        std::fs::create_dir_all(&covers).unwrap();
        let mut all: Vec<GameEntry> = games()
            .iter()
            .map(|g| {
                let title = g.title.replace("'", "");
                let path = wii.join(format!("{title}.rvz"));
                std::fs::write(&path, b"rom").unwrap();
                image::RgbImage::from_pixel(40, 60, image::Rgb([120, 40, 90])).save(covers.join(format!("{title}.png"))).unwrap();
                GameEntry { path, ..g.clone() }
            })
            .collect();
        // More games than the fan shows: the selection must always have its
        // three neighbours on each side, including at the two ends.
        let template = all[0].clone();
        while all.len() < 9 {
            let title = format!("Juego {}", all.len());
            let path = wii.join(format!("{title}.rvz"));
            std::fs::write(&path, b"rom").unwrap();
            image::RgbImage::from_pixel(40, 60, image::Rgb([10, 200, 60])).save(covers.join(format!("{title}.png"))).unwrap();
            all.push(GameEntry { id: format!("extra-{}", all.len()), title, path, ..template.clone() });
        }
        let theme = Theme::builtin(crate::theme::Depth::Truecolor, true);
        for (w, h) in [(120usize, 40usize), (189, 46), (80, 24)] {
            let layout = Layout::compute(rosadeck_tui_frame::safe_width(w as u16), h, all.len());
            for cursor in 0..all.len() {
                let mut browser = Browser::new();
                browser.set_geometry(layout.cols, 1);
                browser.cursor = cursor;
                let mut cache = CoverCache::new();
                let frame = build(
                    &FrameInput::base(&browser, &all, &[], &HashMap::new()),
                    layout,
                    &theme,
                    &mut cache,
                    true,
                );
                let plans = frame.images.clone();
                let margin = layout.margin;
                // The band always has seven slots; at the ends of the library the missing
                // ones are empty frames with no artwork, so there are fewer
                // *plans* there but the same seven cards.
                assert!(
                    plans.len() <= layout.cols,
                    "{w}x{h} cursor {cursor}: never more cards than planned"
                );
                assert!(layout.cols >= 1, "{w}x{h}: siempre hay una tarjeta enfocada");
                if cursor >= 3 && cursor + 3 < all.len() {
                    assert_eq!(
                        plans.len(),
                        layout.cols,
                        "{w}x{h} cursor {cursor}: away from the ends every slot has artwork"
                    );
                }
                for p in &plans {
                    assert!(p.cols > 0 && p.card_w >= p.cols, "the visible slice fits in the card: {p:?}");
                    assert_eq!(
                        p.cols as usize + p.hidden,
                        p.card_w as usize,
                        "the crop accounts for the hidden part: {p:?}"
                    );
                    assert!(p.rows > 0, "{p:?}");
                }
                // Further from the focus the cards are never taller.
                let focus_rows = plans
                    .iter()
                    .find(|p| p.treatment == crate::cover::Treatment::Crisp)
                    .map(|p| p.rows)
                    .expect("the focus is always planned");
                for p in plans.iter().filter(|p| p.treatment == crate::cover::Treatment::Recessed(50)) {
                    assert!(p.rows <= focus_rows, "a neighbour is never taller than the focus: {p:?}");
                }
                // What actually matters: the image plans and the *text* cards
                // agree on the column of every card. They are computed in two
                // different places, and when the band was not centred in both,
                // every cover sat tens of columns away from its cell.
                let row = frame_row_at(&frame, GRID_TOP + 1);
                let cells: Vec<char> = row.chars().collect();
                assert_eq!(cells.len(), layout.width, "the band row is exactly the frame width");
                let mut seen = 0usize;
                for p in &plans {
                    // `col`/`row` are 1-based cursor coordinates (kitty renders
                    // a placement at the cursor), so the artwork starts at
                    // `col - 1`. The cards have no frame: what matters is that
                    // the cells the image covers are blank, so nothing shows
                    // through it, and that every card owns the same columns on
                    // every one of its rows.
                    let art_col = p.col as usize - 1;
                    let first_art = p.row as usize - 1;
                    for y in first_art..first_art + p.rows as usize {
                        let cells: Vec<char> = frame_row_at(&frame, y).chars().collect();
                        assert_eq!(cells.len(), layout.width, "every row is exactly the frame width");
                        let slice = &cells[art_col..art_col + p.cols as usize];
                        assert!(
                            slice.iter().all(|c| *c == ' ' || c.to_string() == glyph::HALF),
                            "{w}x{h} cursor {cursor}: the cells under an image are not blank ({p:?}, row {y})"
                        );
                    }
                    seen += 1;
                }
                assert_eq!(seen, plans.len());
                // Cards tile the band with no gap and no frame between them.
                for pair in plans.windows(2) {
                    assert_eq!(
                        pair[1].col as usize,
                        pair[0].col as usize + pair[0].cols as usize,
                        "cards sit side by side: {:?} then {:?}",
                        pair[0],
                        pair[1]
                    );
                }
                // And it stays inside the frame, roughly centred.
                let first = plans.first().unwrap();
                let last = plans.last().unwrap();
                let band_start = first.col as usize - 1;
                let band_end = last.col as usize + last.cols as usize;
                assert!(band_start >= margin, "{w}x{h}: {first:?}");
                assert!(band_end + 1 <= layout.width - margin, "{w}x{h}: {last:?}");
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Visible glyphs of one row of a frame, indexed by column.
    fn frame_row_at(frame: &Frame, row: usize) -> String {
        frame
            .text
            .lines()
            .nth(row)
            .map(|l| {
                rosadeck_tui_frame::tokens_for_tests(l)
                    .into_iter()
                    .filter(|t| !t.text.starts_with('\x1b'))
                    .filter_map(|t| t.text.chars().next())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The band shows exactly the games there are: seven in the middle, fewer at
    /// the ends, and never the same cover twice.
    ///
    /// Both mistakes are read from the rendered frame — the artwork it sends and
    /// the borders it paints — never recomputed here. Repeating the closest game
    /// to keep the shape showed it three times; drawing an empty frame showed a
    /// hole where nothing exists.
    #[test]
    fn the_band_shows_only_the_games_that_exist() {
        let dir = std::env::temp_dir().join("rosadeck-frame-ends");
        std::fs::remove_dir_all(&dir).ok();
        let wii = dir.join("wii");
        let covers = dir.join("covers");
        std::fs::create_dir_all(&wii).unwrap();
        std::fs::create_dir_all(&covers).unwrap();
        let mut all: Vec<GameEntry> = games()
            .iter()
            .map(|g| {
                let title = g.title.replace("'", "");
                let path = wii.join(format!("{title}.rvz"));
                std::fs::write(&path, b"rom").unwrap();
                image::RgbImage::from_pixel(40, 60, image::Rgb([120, 40, 90]))
                    .save(covers.join(format!("{title}.png")))
                    .unwrap();
                GameEntry { path, ..g.clone() }
            })
            .collect();
        // More games than the fan, or every slot would be empty.
        let template = all[0].clone();
        while all.len() < 9 {
            let title = format!("Juego {}", all.len());
            let path = wii.join(format!("{title}.rvz"));
            std::fs::write(&path, b"rom").unwrap();
            image::RgbImage::from_pixel(40, 60, image::Rgb([10, 200, 60]))
                .save(covers.join(format!("{title}.png")))
                .unwrap();
            all.push(GameEntry { id: format!("x-{}", all.len()), title, path, ..template.clone() });
        }

        let theme = Theme::builtin(crate::theme::Depth::Truecolor, true);
        let layout = Layout::compute(rosadeck_tui_frame::safe_width(189), 46, all.len());
        for cursor in [0usize, 1, 2, 3, all.len() / 2, all.len() - 2, all.len() - 1] {
            let mut browser = Browser::new();
            browser.set_geometry(layout.cols, 1);
            browser.cursor = cursor;
            let mut cache = CoverCache::new();
            let frame = build(
                &FrameInput::base(&browser, &all, &[], &HashMap::new()),
                layout,
                &theme,
                &mut cache,
                true,
            );

            // 1. No artwork twice: read the files the frame really sends.
            let paths: Vec<String> = frame
                .images
                .iter()
                .map(|p| p.path.file_name().unwrap_or_default().to_string_lossy().into_owned())
                .collect();
            let mut unique = paths.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), paths.len(), "cursor {cursor}: a cover is shown twice: {paths:?}");

            // 2. Exactly the cards that exist, and the text has artwork exactly where the
            //    plans will put the images. (Counting zones of lit cells no
            //    longer works: with no frame and no gap the cards touch.)
            let cards = 1 + cursor.min(3) + (all.len() - 1 - cursor).min(3);
            assert_eq!(paths.len(), cards, "cursor {cursor}: una portada por tarjeta: {paths:?}");
            let blocks = build(
                &FrameInput::base(&browser, &all, &[], &HashMap::new()),
                layout,
                &theme,
                &mut cache,
                false,
            );
            let middle = GRID_TOP + layout.cover_h / 2;
            let row: Vec<char> = frame_row_at(&blocks, middle).chars().collect();
            for p in &frame.images {
                let art_col = p.col as usize - 1;
                let slice = &row[art_col..art_col + p.cols as usize];
                assert!(
                    slice.iter().any(|c| *c != ' '),
                    "cursor {cursor}: no hay arte donde va la imagen de {p:?}"
                );
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn status_line_is_actually_painted() {
        // Regression: the status was assigned in ten places and drawn in none,
        // so "played X" and launch errors were invisible. It must be one full
        // row, at the bottom of the band, and must not move the footer.
        let theme = Theme::plain();
        let layout = Layout::compute(120, 40, 2);
        let all = games();
        let played = HashMap::new();
        let mut cache = CoverCache::new();
        let browser = Browser::new();
        let mut input = FrameInput::base(&browser, &all, &[], &played);
        input.status = "rosadeck: PREPARE_FAILED: emulator binary not found: retroarch";
        let with_status = build(&input, layout, &theme, &mut cache, false).text;

        input.status = "";
        let without = build(&input, layout, &theme, &mut cache, false).text;

        assert!(with_status.contains("PREPARE_FAILED"), "{with_status}");
        let rows: Vec<&str> = with_status.lines().collect();
        // Tail layout: status, rule, detail, legend (cuatro filas, las mínimas).
        let idx = rows.iter().position(|l| l.contains("PREPARE_FAILED")).expect("status row painted");
        assert_eq!(rows.len() - idx, 4, "status + rule + detail + legend end the frame: {rows:?}");
        assert!(rows[idx + 1].replace(glyph::H, "").trim().is_empty(), "a rule separates it from the detail: {rows:?}");
        // The legend row and the row count never move: the row is always there.
        assert_eq!(
            with_status.lines().count(),
            without.lines().count(),
            "an empty status must still paint its row"
        );
        assert_eq!(
            rows[rows.len() - 1],
            without.lines().last().unwrap(),
            "the legend row must not shift when the status changes"
        );
        assert!(!rows[rows.len() - 1].contains("PREPARE_FAILED"), "status is not the legend");
        for line in with_status.lines() {
            assert_eq!(visible_len(line), layout.width, "every line is exactly the drawn width");
        }
    }

    #[test]
    fn detail_pane_is_honest_about_the_emulator() {
        // Installed, missing and unconfigured must be distinguishable: Enter
        // starts an emulator, so "looks fine but cannot launch" is a bug.
        let theme = Theme::builtin(crate::theme::Depth::Truecolor, true);
        let layout = Layout::compute(190, 46, 2);
        let all = games();
        let plays = HashMap::new();
        let installed = EmulatorStatus {
            id: "dolphin".into(),
            name: "Dolphin".into(),
            binary: "dolphin-emu".into(),
            resolved: Some("/usr/sbin/dolphin-emu".into()),
        };
        let missing = EmulatorStatus {
            id: "snes9x".into(),
            name: "Snes9x".into(),
            binary: "snes9x-gtk".into(),
            resolved: None,
        };
        let mut cache = CoverCache::new();

        let played = HashMap::new();
        let mut browser = Browser::new();
        browser.platform = Some(Platform::Wii);
        let frame = build(
            &FrameInput { browser: &browser, games: &all, favorites: &[], played: &played, emulators: &[installed.clone()], roots: &[], status: "", art_mode: "" },
            layout,
            &theme,
            &mut cache,
            false,
        )
        .text;
        assert!(frame.contains("dolphin \u{2713}"), "installed emulator shows a tick: {frame}");
        assert!(!frame.contains("NOT INSTALLED"));
        assert!(!frame.contains("needs emulator"), "Enter is still 'play' when playable: {frame}");

        browser.platform = Some(Platform::Snes);
        let frame = build(
            &FrameInput { browser: &browser, games: &all, favorites: &[], played: &played, emulators: &[installed, missing], roots: &[], status: "", art_mode: "" },
            layout,
            &theme,
            &mut cache,
            false,
        )
        .text;
        assert!(frame.contains("NOT INSTALLED"), "{frame}");
        assert!(frame.contains("pacman -S snes9x-gtk"), "the panel offers the package: {frame}");
        assert!(frame.contains("needs emulator"), "the legend admits it: {frame}");

        // No profile at all is a third state: it must not read as installed.
        browser.platform = Some(Platform::Nintendo3DS);
        let frame = build(
            &FrameInput::base(&browser, &all, &[], &plays),
            layout,
            &theme,
            &mut cache,
            false,
        )
        .text;
        assert!(frame.contains("azahar NOT CONFIGURED"), "no profile is not the same as installed: {frame}");
    }

    #[test]
    fn real_cover_art_reaches_the_frame_as_truecolor() {
        // End to end: a solid magenta PNG found next to the ROM must surface as
        // magenta half blocks (no kitty graphics protocol needed).
        let dir = std::env::temp_dir().join("rosadeck-frame-art-real");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("wii")).unwrap();
        image::RgbImage::from_pixel(16, 24, image::Rgb([255, 0, 255])).save(dir.join("wii/G.png")).unwrap();
        let games = vec![GameEntry {
            id: "art1".into(),
            title: "Art Test".into(),
            platform: Platform::Wii,
            path: dir.join("wii/G.rvz"),
            region: None,
            size: 10,
        }];
        let layout = Layout::compute(120, 40, 1);
        let theme = Theme::builtin(crate::theme::Depth::Truecolor, true);
        let mut cache = CoverCache::new();
        let frame = build(
            &FrameInput::base(&Browser::new(), &games, &[], &HashMap::new()),
            layout,
            &theme,
            &mut cache,
            false,
        )
        .text;
        assert!(frame.contains("38;2;255;0;255"), "magenta pixels must reach the frame");
        assert!(frame.contains('▀'));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lossy_cover_formats_still_paint_the_frame() {
        // WebP (lossy) and AVIF (via dav1d) named after the *game*, not the
        // ROM: both must reach the frame with their own pixels.
        let dir = std::env::temp_dir().join("rosadeck-frame-lossy");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("art/wii")).unwrap();
        let mut base = GameEntry {
            id: "lossy".into(),
            title: "Luigi's Mansion".into(),
            platform: Platform::Wii,
            path: dir.join("wii/Luigi's Mansion (Europe) (En,Fr,De,Es,It).rvz"),
            region: None,
            size: 10,
        };
        let layout = Layout::compute(120, 40, 1);
        let theme = Theme::builtin(crate::theme::Depth::Truecolor, true);
        let solid = image::RgbaImage::from_pixel(32, 48, image::Rgba([0, 255, 0, 255]));
        for ext in ["webp", "avif"] {
            let art = dir.join(format!("art/wii/Luigi's Mansion.{ext}"));
            solid
                .save_with_format(&art, image::ImageFormat::from_path(&art).unwrap())
                .expect("encode");
            let mut cache = CoverCache::new();
            base.path = dir.join("wii/Luigi's Mansion (Europe) (En,Fr,De,Es,It).rvz");
            let frame = build(
                &FrameInput::base(&Browser::new(), std::slice::from_ref(&base), &[], &HashMap::new()),
                layout,
                &theme,
                &mut cache,
                false,
            )
            .text;
            assert!(frame.contains("38;2;0;255;0"), "{ext} green pixels missing from the frame");
            assert!(frame.contains('▀'), "{ext} half blocks");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_art_falls_back_to_generated_cover() {
        let games = vec![GameEntry {
            id: "noart".into(),
            title: "No Art Here".into(),
            platform: Platform::Snes,
            path: "/roms/snes/Nope.sfc".into(),
            region: None,
            size: 10,
        }];
        let layout = Layout::compute(120, 40, 1);
        let mut cache = CoverCache::new();
        let frame = build(
            &FrameInput::base(&Browser::new(), &games, &[], &HashMap::new()),
            layout,
            &Theme::plain(),
            &mut cache,
            false,
        )
        .text;
        // Plain mode renders the ramp instead of half blocks; the generated
        // cover must still be visible.
        assert!(
            frame.contains('█') || frame.contains('▒') || frame.contains('░'),
            "placeholder cover is still drawn: {frame:?}"
        );
    }

    #[test]
    fn image_mode_plans_every_cover_and_leaves_the_area_blank() {
        // With the graphics protocol the artwork is sent as an image, so the
        // cell interior must stay empty (no half blocks to paint over it).
        let dir = std::env::temp_dir().join("rosadeck-frame-imgmode");
        std::fs::remove_dir_all(&dir).ok();
        let wii = dir.join("wii");
        let covers = dir.join("covers");
        std::fs::create_dir_all(&wii).unwrap();
        std::fs::create_dir_all(&covers).unwrap();
        let rom = wii.join("Luigi's Mansion (Europe).rvz");
        std::fs::write(&rom, b"rom").unwrap();
        image::RgbImage::from_pixel(32, 48, image::Rgb([9, 9, 9])).save(covers.join("Luigi's Mansion.png")).unwrap();
        let games = vec![GameEntry { id: "im".into(), title: "Luigi's Mansion".into(), platform: Platform::Wii, path: rom, region: None, size: 10 }];
        let layout = Layout::compute(120, 40, 1);
        let theme = Theme::builtin(crate::theme::Depth::Truecolor, true);
        let mut cache = CoverCache::new();
        let frames: Vec<Frame> = [false, true]
            .iter()
            .map(|use_images| {
                build(
                    &FrameInput::base(&Browser::new(), &games, &[], &HashMap::new()),
                    layout,
                    &theme,
                    &mut cache,
                    *use_images,
                )
            })
            .collect();
        let (blocks, images) = (&frames[0], &frames[1]);
        assert_eq!(blocks.images.len(), 0, "no plans without the protocol");
        assert_eq!(images.images.len(), 1, "one plan for the one cover");
        let plan = &images.images[0];
        assert!(plan.path.ends_with("Luigi's Mansion.png"), "{plan:?}");
        assert_eq!((plan.cols as usize, plan.rows as usize), (layout.cover_w, layout.cover_h));
        assert!(plan.col as usize >= layout.margin + 1 && plan.row > 0, "placed inside the band: {plan:?}");
        // Same geometry as the cover's cell area, and the text layer is blank.
        assert!(blocks.text.contains('▀'), "fallback draws half blocks");
        assert!(!images.text.contains('▀'), "image cells must not draw blocks");
        assert!(!images.text.contains("cover"), "el pie ya no nombra la portada");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn image_mode_skips_generated_covers() {
        // A game with no artwork must stay as half blocks even in image mode,
        // otherwise its cell would be an empty hole.
        let games = vec![GameEntry { id: "na".into(), title: "No Art".into(), platform: Platform::Snes, path: "/nope/x.sfc".into(), region: None, size: 10 }];
        let layout = Layout::compute(120, 40, 1);
        let theme = Theme::builtin(crate::theme::Depth::Truecolor, true);
        let mut cache = CoverCache::new();
        let frame = build(
            &FrameInput::base(&Browser::new(), &games, &[], &HashMap::new()),
            layout,
            &theme,
            &mut cache,
            true,
        );
        assert!(frame.images.is_empty(), "nothing to send: {frame:?}");
        assert!(frame.text.contains('▀'), "la portada generada se dibuja");
        assert!(!frame.text.contains("generated"), "y no hace falta anunciarlo");
    }

    #[test]
    fn painting_real_frames_converges_to_the_last_one() {
        // Real carousel frames (selection walking the band, then back) painted
        // through `tui_frame::Screen` must leave the screen exactly as a full
        // repaint of the last frame would. Characters *and* styling.
        let all = games();
        let played = HashMap::from([("1".to_owned(), 3u64)]);
        let theme = Theme::builtin(crate::theme::Depth::Truecolor, true);
        for (w, h) in [(120usize, 40usize), (189, 46), (80, 24)] {
            // Mirror the runtime: the layout works on the safe width.
            let layout = Layout::compute(rosadeck_tui_frame::safe_width(w as u16), h, all.len());
            let cursors: Vec<usize> = (0..all.len()).chain((0..all.len()).rev()).collect();
            let mut frames = Vec::new();
            for cursor in cursors.clone() {
                let mut browser = Browser::new();
                browser.set_geometry(layout.cols, 1);
                browser.cursor = cursor;
                let mut cache = CoverCache::new();
                frames.push(
                    build(
                        &FrameInput::base(&browser, &all, &[], &played),
                        layout,
                        &theme,
                        &mut cache,
                        false,
                    )
                    .text,
                );
            }
            let rows = layout.height;
            let mut screen = rosadeck_tui_frame::Screen::new();
            // The terminal is `w` columns wide; only the geometry uses the safe
            // width. Mixing the two truncates the frame's last column.
            let mut term = rosadeck_tui_frame::vt::Terminal::new(w, rows);
            for frame in &frames {
                let mut bytes = Vec::new();
                screen.draw(&mut bytes, frame, w as u16, rows as u16, false).unwrap();
                term.feed_bytes(&bytes);
            }
            let mut expected = rosadeck_tui_frame::vt::Terminal::new(w, rows);
            expected.feed(&format!("\x1b[2J\x1b[1;1H{}", rosadeck_tui_frame::vt::to_crlf(frames.last().unwrap())));
            if let Some((y, x, got, want)) = term.first_difference(&expected) {
                // Which step introduced it?
                let mut s2 = rosadeck_tui_frame::Screen::new();
                let mut t2 = rosadeck_tui_frame::vt::Terminal::new(w, rows);
                let mut culprit = None;
                for (i, f) in frames.iter().enumerate() {
                    let mut b = Vec::new();
                    s2.draw(&mut b, f, w as u16, rows as u16, false).unwrap();
                    t2.feed_bytes(&b);
                    let mut e2 = rosadeck_tui_frame::vt::Terminal::new(w, rows);
                    e2.feed(&format!("\x1b[2J\x1b[1;1H{}", rosadeck_tui_frame::vt::to_crlf(f)));
                    if t2.first_difference(&e2).is_some() {
                        culprit = Some((i, b.len(), cursors[i]));
                        break;
                    }
                }
                let culprit_frame = culprit.map(|(i, _, _)| i).unwrap_or(frames.len() - 1);
                let culprit_row = frames[culprit_frame].lines().nth(y).unwrap_or("");
                let ctx: Vec<String> = rosadeck_tui_frame::tokens_for_tests(culprit_row)
                    .into_iter()
                    .filter(|t| t.col + 1 >= x.saturating_sub(12) && t.col <= x + 4)
                    .map(|t| format!("[{}]{:?}", t.col, t.text))
                    .take(32)
                    .collect();
                panic!(
                    "{w}x{h}: celda ({y},{x}) pintada {got:?}, repintado daría {want:?}; primer fallo {culprit:?}\n  tokens: {}",
                    ctx.join(" ")
                );
            }
        }
    }

    #[test]
    fn initials_are_short_and_useful() {
        assert_eq!(initials("Luigi's Mansion", Platform::Wii), "LM");
        assert_eq!(initials("Super Mario Sunshine", Platform::Wii), "SMS");
        assert_eq!(initials("The Legend of Zelda - A Link to the Past", Platform::Snes), "TLO");
        assert_eq!(initials("1942", Platform::Snes), "SNES", "digits only fall back");
        assert_eq!(initials("!!!", Platform::Wii), "WII");
    }

    /// The play-time label: seconds, minutes, hours. `4x` counted launches and
    /// said nothing about the time; `3h 20m` is the thing a player remembers.
    #[test]
    fn play_time_is_shown_in_the_unit_that_reads() {
        assert_eq!(played_time(1), "1s");
        assert_eq!(played_time(45), "45s");
        assert_eq!(played_time(59), "59s");
        assert_eq!(played_time(60), "1m", "un minuto ya no son segundos");
        assert_eq!(played_time(119), "1m");
        assert_eq!(played_time(12 * 60), "12m");
        assert_eq!(played_time(3599), "59m");
        assert_eq!(played_time(3600), "1h 0m");
        assert_eq!(played_time(3 * 3600 + 20 * 60), "3h 20m");
        assert_eq!(played_time(3 * 3600 + 59 * 60 + 59), "3h 59m", "los segundos se redondean hacia abajo");
        assert_eq!(played_time(120 * 3600), "120h", "y por encima de cien horas no hay minutos");
        assert!(!played_time(3661).contains('x'), "el contador de lanzamientos se fue");
    }

    /// The focused card shows the time, and `nunca` when there is none: a shelf
    /// that invents a duration is worse than one that admits it does not know.
    #[test]
    fn the_focused_card_shows_played_time() {
        let theme = Theme::builtin(crate::theme::Depth::Truecolor, true);
        let layout = Layout::compute(189, 46, 1);
        let all = games();
        let mut cache = CoverCache::new();
        let frame = build(
            // Todos los juegos con tiempo, porque la enfocada es la primera
            // por título y no es necesariamente el primer id del fixture.
            &FrameInput::base(
                &Browser::new(),
                &all,
                &[],
                &HashMap::from_iter(all.iter().map(|g| (g.id.clone(), 7_560u64))),
            ),
            layout,
            &theme,
            &mut cache,
            false,
        )
        .text;
        assert!(frame.contains("2h 6m"), "7.560 s son 2h 6m: {frame}");
        let frame = build(
            &FrameInput::base(&Browser::new(), &all, &[], &HashMap::new()),
            layout,
            &theme,
            &mut cache,
            false,
        )
        .text;
        assert!(frame.contains("nunca"), "sin partidas: {frame}");
        // El contador antiguo («4x») no puede aparecer en la etiqueta.
        let label = frame.lines().find(|l| l.contains("nunca")).unwrap_or_default();
        assert!(!label.contains('x'), "y sin el contador antiguo: {label:?}");
    }

    #[test]
    fn format_helpers() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KiB", "con un decimal cuando el número es pequeño");
        assert_eq!(human_size(1_500_000_000), "1.4 GiB");
        assert_eq!(human_size(240_000_000), "229 MiB", "y sin decimales cuando no aportan nada");
        assert_eq!(ellipsize("short", 10), "short");
        assert_eq!(ellipsize("abcdefghij", 5), "abcd…");
        assert_eq!(ellipsize("abc", 0), "");
    }
}


