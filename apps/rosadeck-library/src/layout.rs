//! Carousel geometry: one row of large covers, paned horizontally.
//!
//! A cover gets every row the frame can spare, because at half-block
//! resolution each character column is a single pixel: a 30-row cover is a
//! 30x60-pixel image, which is exactly what the terminal can show. (When the
//! terminal supports the kitty graphics protocol the artwork is sent as a real
//! image instead — see `images.rs` — but the geometry stays the same.)
//!
//! Pure arithmetic, brute-forced over candidate sizes and always deterministic.

/// Per-cell rows: cover + title + meta (+1 spare). The cards have no frame.
pub const CELL_ROWS_OVERHEAD: usize = 4;
/// Fixed rows outside the band (header, chips, rule, status, rule, detail,
/// legend). It was 11 with a three-row header box and a three-line detail pane;
/// every row those cost goes to the cover, which is what the eye looks at.
pub const CHROME_ROWS: usize = 7;
/// Terminal row where the cover band starts (header 1 + chips 1 + rule 1).
/// It was 5 with a three-row header box; the two rows go to the cover now.
pub const GRID_TOP: usize = 3;
/// Smallest acceptable cover height in character rows.
pub const MIN_COVER_H: usize = 5;
/// Largest acceptable cover height in character rows.
pub const MAX_COVER_H: usize = 40;
/// Smallest acceptable cover width in columns.
pub const MIN_COVER_W: usize = 8;
/// Largest acceptable cover width in columns.
pub const MAX_COVER_W: usize = 60;
/// Most cards the fan may show (focus plus neighbours on both sides).
pub const MAX_FAN: usize = 7;
/// Portrait 3:4 target. A cell is ~2x taller than wide, so one `▀` glyph
/// carries two pixels and `cover_w / cover_h == 1.5` renders a 3:4 image.
pub const ASPECT: f64 = 1.5;

/// Computed geometry for one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// Usable columns (terminal width minus the last-column safety column).
    pub width: usize,
    /// Usable rows.
    pub height: usize,
    /// Left/right indent of the frame.
    pub margin: usize,
    /// Width of the focused card, in columns.
    pub cover_w: usize,
    /// Height of the focused card, in character rows.
    pub cover_h: usize,
    /// Cards the fan shows at this size (focus included).
    pub cols: usize,
    /// Columns the band actually draws.
    pub drawn_width: usize,
}

impl Layout {
    /// Geometry for a `width` x `height` frame showing `count` games.
    ///
    /// The focused card takes every row it can and keeps the portrait aspect;
    /// the fan fills whatever is left on both sides.
    pub fn compute(width: usize, height: usize, count: usize) -> Self {
        let width = width.max(MIN_COVER_W * 3 + 8);
        let height = height.max(CHROME_ROWS + MIN_COVER_H + CELL_ROWS_OVERHEAD);
        let margin = if width > 100 { 2 } else { 1 };
        let available = width.saturating_sub(2 * margin).max(MIN_COVER_W + 4);
        let band_h = height.saturating_sub(CHROME_ROWS).max(MIN_COVER_H + CELL_ROWS_OVERHEAD);
        let need = count.max(1);

        let cover_h = clamp(band_h - CELL_ROWS_OVERHEAD, MIN_COVER_H, MAX_COVER_H);
        // A single game may be as wide as the terminal; with neighbours the
        // focus keeps its aspect and the fan fills the rest.
        let max_w = if need > 1 { clamp(available / 2, MIN_COVER_W, MAX_COVER_W) } else { MAX_COVER_W };
        let cover_w = clamp((cover_h as f64 * ASPECT).round() as usize, MIN_COVER_W, max_w);
        // The plan for *this* library size, so `cols` is the number of cards the
        // band really uses (the ends of the library draw one or two fewer, which
        // the frame handles when it centres).
        let cards = fan(available, cover_w, cover_h, need);
        Self {
            width,
            height,
            margin,
            cover_w,
            cover_h,
            cols: cards.len().min(need),
            drawn_width: fan_width(&cards).min(available),
        }
    }

    /// Rows the cover band occupies (cover + labels + spare row).
    pub fn band_height(&self) -> usize {
        self.cover_h + CELL_ROWS_OVERHEAD
    }

}

/// One card of the fanned carousel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Card {
    /// `-1` left of the focus, `0` the focused game, `+1` right.
    pub side: i8,
    /// Distance from the focus: 0 = selected, 1 = its neighbour, …
    pub distance: usize,
    /// Columns the card actually draws (the rest hides behind its neighbour).
    pub width: usize,
    /// Columns cropped off the inner edge, i.e. hidden behind the closer card.
    pub hidden: usize,
    /// Art rows.
    pub rows: usize,
    /// Blank rows above and below the art (the card recedes).
    pub inset: usize,
    /// How far back the card sits, 1-100: it drives the blur, the desaturation
    /// and how much it melts into the page.
    pub depth: u8,
}

impl Card {
    /// Columns this card draws in the band.
    ///
    /// No frame: the artwork speaks for itself, and a box around every sliver
    /// turned the shelf into a row of separate pictures. The fan still reads
    /// because the cards shrink, sit lower and fade with distance.
    pub fn total_width(&self) -> usize {
        self.width
    }

    /// Full card width before the inner part is hidden.
    pub fn full_width(&self) -> usize {
        self.width + self.hidden
    }
}

/// Columns of empty background between two cards.
pub const CARD_GAP: usize = 0;
/// Smallest *visible* sliver of a neighbour, so even the furthest card shows
/// something recognisable.
const MIN_CARD_VISIBLE: usize = 5;
/// How much of a card's width the closer neighbour covers.
const HIDDEN_FRACTION: f64 = 0.45;
/// Width of the first neighbour relative to the focus.
const SIDE_RATIO: f64 = 0.78;
/// Each further neighbour shrinks by this factor (perspective).
const SIDE_FALLOFF: f64 = 0.66;
/// Height of the first neighbour relative to the focus.
const ROW_RATIO: f64 = 0.88;
/// Each further neighbour loses this share of height. Steeper than the width
/// falloff on purpose: a card that is only a bit narrower but nearly as tall
/// reads as a smaller copy, not as one that is further away.
const ROW_FALLOFF: f64 = 0.8;
/// Depth (1-100) of the first neighbour, plus this much per extra step.
const DEPTH_BASE: u8 = 46;
const DEPTH_STEP: u8 = 14;

/// The fanned carousel: the focused game in the middle, its neighbours
/// overlapping it and getting smaller, smaller and fainter.
///
/// Pure arithmetic over `available` columns, so the band always fits: cards are
/// added alternately to both sides while there is room, and the plan is stable
/// for a given (width, count).
pub fn fan(available: usize, cover_w: usize, cover_h: usize, games: usize) -> Vec<Card> {
    let focus = Card {
        side: 0,
        distance: 0,
        width: cover_w,
        hidden: 0,
        rows: cover_h,
        inset: 0,
        depth: 0,
    };
    let mut cards = vec![focus];
    let mut width = focus.total_width();
    let mut distance = 0usize;
    // One card per side at a time, so the fan stays symmetric: a wide terminal
    // grows outwards instead of giving one side extra cards.
    loop {
        distance += 1;
        if 2 * distance + 1 > MAX_FAN || 2 * distance >= games {
            break; // capped, or nothing left but the focus
        }
        let ratio = SIDE_RATIO * SIDE_FALLOFF.powi(distance as i32 - 1);
        // No floor that stops the shrink: past a point the card has to keep
        // getting thinner, otherwise the fan turns into a row of equal stubs.
        let full = ((cover_w as f64 * ratio).round() as usize)
            .max(MIN_CARD_VISIBLE + 2)
            .min(cover_w.saturating_sub(2));
        let hidden = ((full as f64 * HIDDEN_FRACTION).round() as usize)
            .min(full.saturating_sub(MIN_CARD_VISIBLE));
        let width_k = full - hidden;
        let rows = ((cover_h as f64 * (ROW_RATIO * ROW_FALLOFF.powi(distance as i32 - 1))).round() as usize)
            .max(MIN_COVER_H / 2)
            .min(cover_h.saturating_sub(2));
        let depth = DEPTH_BASE.saturating_add(DEPTH_STEP * (distance as u8 - 1)).min(100);
        // Exactly the columns the card draws, so the fit check cannot disagree with
        // the assembly.
        let step = width_k + CARD_GAP;
        // Both sides at once: the fan is symmetric, so the pair has to fit.
        if width + 2 * step > available {
            break;
        }
        width += 2 * step;
        cards.push(Card { side: -1, distance, width: width_k, hidden, rows, inset: (cover_h - rows) / 2, depth });
        cards.push(Card { side: 1, distance, width: width_k, hidden, rows, inset: (cover_h - rows) / 2, depth });
    }
    cards
}

/// Column where the band starts for the cards that will really be drawn.
///
/// The fan at the ends of the library has one or two cards fewer, so the band is
/// narrower and has to be centred on what is drawn — not on the plan for the
/// worst case, which put every cover tens of columns away from its cell.
pub fn band_offset(margin: usize, available: usize, drawn: &[Card]) -> usize {
    margin + available.saturating_sub(fan_width(drawn)) / 2
}

/// Total columns a [`fan`] occupies, gaps included.
pub fn fan_width(cards: &[Card]) -> usize {
    cards.iter().map(|c| c.total_width()).sum::<usize>() + CARD_GAP * cards.len().saturating_sub(1)
}

fn clamp(v: usize, lo: usize, hi: usize) -> usize {
    v.max(lo).min(hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn users_terminal_gets_a_big_focus_and_neighbours() {
        let l = Layout::compute(189, 46, 13);
        assert_eq!(l.cover_h, 35, "cada fila que el chrome suelta va a la portada enfocada");
        assert!(l.cover_w >= 30, "{l:?}");
        assert!(l.cols >= 3, "focus plus a neighbour on each side: {l:?}");
        assert!(l.cols <= MAX_FAN, "{l:?}");
        assert!(l.drawn_width <= 189, "{l:?}");
        let available = l.width - 2 * l.margin;
        assert!(fan_width(&fan(available, l.cover_w, l.cover_h, 13)) <= available, "the fan fits: {l:?}");
        let ratio = l.cover_w as f64 / l.cover_h as f64;
        assert!((ratio - ASPECT).abs() < 0.35, "aspect {ratio} in {l:?}");
    }

    #[test]
    fn fits_every_terminal_and_library_size() {
        for (w, h) in [(20usize, 12usize), (40, 16), (60, 20), (80, 24), (100, 30), (132, 43), (189, 46), (300, 80), (500, 200)] {
            for n in [0usize, 1, 4, 13, 5000] {
                let l = Layout::compute(w, h, n);
                let available = l.width - 2 * l.margin;
                assert!(l.cols >= 1, "{w}x{h} n={n} -> {l:?}");
                assert!(l.cover_h >= MIN_COVER_H && l.cover_h <= MAX_COVER_H, "{w}x{h} -> {l:?}");
                assert!(l.cover_w >= MIN_COVER_W && l.cover_w <= MAX_COVER_W, "{w}x{h} -> {l:?}");
                assert!(l.drawn_width <= available, "{w}x{h} n={n} -> {l:?}");
                let band = fan_width(&fan(available, l.cover_w, l.cover_h, n));
                assert!(band <= available, "{w}x{h} n={n}: the real fan fits: {band} > {available}");
                assert!(GRID_TOP + l.band_height() + (CHROME_ROWS - GRID_TOP) <= l.height, "{w}x{h} -> {l:?}");
                assert!(l.cols <= n.max(1), "no empty cards: {w}x{h} n={n} -> {l:?}");
                assert!(l.margin + available.saturating_sub(band) / 2 + band <= l.width, "{w}x{h} n={n} -> {l:?}");
            }
        }
    }

    #[test]
    fn one_game_gets_the_whole_width() {
        let l = Layout::compute(189, 46, 1);
        assert_eq!(l.cols, 1, "no neighbours to draw");
        assert!(l.cover_w >= 40, "one cover fills the band: {l:?}");
        assert!(l.drawn_width <= l.width);
    }

    #[test]
    fn bigger_terminals_get_bigger_covers_and_more_neighbours() {
        let small = Layout::compute(80, 24, 13);
        let big = Layout::compute(300, 80, 13);
        assert!(big.cover_w > small.cover_w);
        assert!(big.cover_h > small.cover_h);
        assert!(big.cols >= small.cols, "a wider terminal shows more of the fan");
    }

    /// The fan itself: symmetric, shrinking, cropped and always inside the
    /// available columns.
    #[test]
    fn the_fan_shrinks_outwards_and_never_leaves_the_band() {
        for (w, h) in [(60usize, 20usize), (80, 24), (120, 40), (189, 46), (300, 60)] {
            for n in [1usize, 3, 13, 99] {
                let l = Layout::compute(w, h, n);
                let available = l.width - 2 * l.margin;
                let cards = fan(available, l.cover_w, l.cover_h, n);
                assert!(fan_width(&cards) <= available, "{w}x{h} n={n}: {cards:?}");
                let focus = cards.iter().find(|c| c.side == 0).unwrap();
                assert_eq!(focus.width, l.cover_w);
                assert_eq!(focus.hidden, 0, "the focus is never cropped");
                assert_eq!(focus.rows, l.cover_h);
                assert_eq!(focus.inset, 0);
                let left: Vec<&Card> = cards.iter().filter(|c| c.side < 0).collect();
                let right: Vec<&Card> = cards.iter().filter(|c| c.side > 0).collect();
                assert_eq!(left.len(), right.len(), "the fan is symmetric: {cards:?}");
                for (k, c) in left.iter().enumerate() {
                    let r = &right[k];
                    assert_eq!((c.width, c.rows, c.hidden), (r.width, r.rows, r.hidden), "mirrored");
                    assert_eq!(c.distance, k + 1);
                    assert!(c.width < focus.width, "distance {k} narrower: {c:?}");
                    if k > 0 {
                        // Never grows going outwards. It may stop shrinking at
                        // the minimum sliver on a very narrow terminal, which is
                        // why this is "not wider" and not "narrower".
                        let inner = left[k - 1];
                        assert!(c.width <= inner.width, "and never wider than the closer one: {c:?}");
                        assert!(c.rows <= inner.rows, "and never taller: {c:?}");
                        assert!(c.rows < focus.rows, "but always shorter than the focus: {c:?}");
                    }
                    assert!(c.hidden > 0, "overlapped by the card in front: {c:?}");
                    assert!(c.full_width() <= focus.width, "{c:?}");
                    assert_eq!(c.inset, (l.cover_h - c.rows) / 2, "centred vertically: {c:?}");
                }
            }
        }
    }

    #[test]
    fn the_fan_stops_either_when_it_is_full_or_when_it_is_capped() {
        let l = Layout::compute(100, 30, 50);
        let available = l.width - 2 * l.margin;
        let cards = fan(available, l.cover_w, l.cover_h, 50);
        assert!(cards.len() <= MAX_FAN, "never more than the cap: {cards:?}");
        assert!(fan_width(&cards) <= available);
        let right = cards.iter().filter(|c| c.side > 0).count();
        let capped = 2 * right + 1 == MAX_FAN;
        if !capped {
            // Not capped: one more card must genuinely not have fitted.
            let step = cards.last().unwrap().total_width() + CARD_GAP;
            assert!(fan_width(&cards) + step > available, "it stopped for lack of room: {cards:?}");
        }
        // A wide terminal shows more of the fan than a narrow one.
        let narrow = fan(40, l.cover_w, l.cover_h, 50).len();
        let wide = fan(300, l.cover_w, l.cover_h, 50).len();
        assert!(wide >= narrow, "wider terminal, more cards: {narrow} vs {wide}");
    }

    #[test]
    fn band_is_centred_and_never_overflows() {
        for (w, h) in [(60usize, 20usize), (80, 24), (120, 40), (189, 46), (300, 60)] {
            for n in [1usize, 3, 13, 99] {
                let l = Layout::compute(w, h, n);
                let available = l.width - 2 * l.margin;
                let cards = fan(available, l.cover_w, l.cover_h, n);
                let band = fan_width(&cards);
                // Centring is the frame's job (it centres the cards it really
                // draws); here we only require that the band fits.
                assert!(band <= available, "{w}x{h}: {l:?}");
                let offset = l.margin + available.saturating_sub(band) / 2;
                assert!(offset + band <= l.width, "{w}x{h}: {l:?}");
            }
        }
    }

    #[test]
    fn layout_is_deterministic() {
        let a = Layout::compute(189, 46, 13);
        let b = Layout::compute(189, 46, 13);
        assert_eq!(a, b);
        assert_eq!(fan(120, 40, 30, 13), fan(120, 40, 30, 13));
    }
}
