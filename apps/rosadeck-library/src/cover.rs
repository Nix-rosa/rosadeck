//! Cover art: real images rendered as half-block cells, plus generated
//! placeholder art so the library never shows empty holes.
//!
//! A terminal cell is about twice as tall as it is wide, so one `▀` glyph
//! carries **two** vertical pixels: a `w x h` character box shows a `w x 2h`
//! pixel image. Consecutive pixels sharing a colour are merged into a single
//! SGR pair, which keeps a full-screen grid around 6 KB instead of 60 KB.
//!
//! Missing/broken art is not an error: `Cover::placeholder` synthesises a
//! deterministic retro label from the title hash.

use crate::theme::{Depth, Rgb, Theme, glyph};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Which part of a card the artwork keeps: the left edge, the right edge, or
/// all of it (the focused game).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Crop {
    /// Columns kept from the left edge.
    pub keep_left: usize,
    /// Columns kept from the right edge.
    pub keep_right: usize,
}

impl Crop {
    /// Nothing cropped.
    pub const FULL: Crop = Crop { keep_left: 0, keep_right: 0 };

    /// The visible slice of a left-hand neighbour: its outer (left) edge.
    pub fn left(visible: usize) -> Self {
        Self { keep_left: visible, keep_right: 0 }
    }

    /// The visible slice of a right-hand neighbour: its outer (right) edge.
    pub fn right(visible: usize) -> Self {
        Self { keep_left: 0, keep_right: visible }
    }

    /// Total columns kept (0 means "all", see [`Crop::FULL`]).
    ///
    /// There is deliberately no `hidden()`: it would need the card's full width
    /// as an argument, and when it was written without one it returned the
    /// *kept* columns — so a neighbour's image was placed with its visible and
    /// hidden counts swapped.
    pub fn kept(&self) -> usize {
        self.keep_left + self.keep_right
    }
}

/// RGB pixel buffer packed as `0xRRGGBB`.
pub type Pixels = Vec<u32>;

/// How a cover is drawn, before any pixel reaches the terminal.
///
/// The selected game is the top layer: [`Treatment::Crisp`], full colour and
/// sharp. Its neighbours are underneath, so they use [`Treatment::Recessed`] —
/// desaturated, blurred and faded towards the backdrop, which is what the eye
/// reads as "further back". The same treatment drives both renderers, so a
/// cover recedes the same way drawn with half blocks or transmitted as an
/// image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Treatment {
    /// On top: the selected game, untouched.
    Crisp,
    /// Behind: blurred, desaturated and faded (see [`recede`]). The payload is
    /// how far back the card sits, 1-100, so the fan fades with distance
    /// instead of every neighbour looking equally far away.
    Recessed(u8),
}

/// Blur a cover so it reads as "behind", independent of the theme.
///
/// Order matters: blur first (mix of neighbours), then desaturate (chroma is
/// what the eye reads as "in focus"), then blend towards the backdrop so the
/// cover sits on the same plane as the page — dimmer on a dark theme, lighter
/// on a light one, instead of always being darker.
pub fn recede(img: &mut image::RgbImage, backdrop: Rgb, depth: u8) {
    let w = img.width() as i32;
    let h = img.height() as i32;
    // A *small* absolute radius, on purpose. A radius proportional to the image
    // (width/40) meant 15 pixels on a 600x900 file: a 31x31 box over half a
    // million pixels took 2.4 seconds per cover, and the fan has six of them.
    // Blur is done at cover size, where 1-3 pixels is already a visible soften,
    // and it grows with the distance from the focus: that gradient is what makes
    // the far cards read as further away instead of merely smaller.
    let depth = depth.clamp(1, 100) as i32;
    let radius = (1 + depth / 40).clamp(1, 3);
    let src = img.clone();
    for y in 0..h {
        for x in 0..w {
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let sx = (x + dx).clamp(0, w - 1) as u32;
                    let sy = (y + dy).clamp(0, h - 1) as u32;
                    let p = src.get_pixel(sx, sy).0;
                    r += p[0] as u32;
                    g += p[1] as u32;
                    b += p[2] as u32;
                    n += 1;
                }
            }
            let (mut r, mut g, mut b) = (r / n, g / n, b / n);
            // Chroma is what the eye reads as "in focus", so it fades with
            // distance too: the nearest neighbour still shows its colours, the
            // furthest is nearly grey.
            let chroma = (40 - depth / 3).clamp(8, 40) as i32;
            let luma = ((r * 299 + g * 587 + b * 114) / 1000) as i32;
            let desaturate = |c: u32| {
                let v = luma + (c as i32 - luma) * chroma / 100;
                v.clamp(0, 255) as u32
            };
            r = desaturate(r);
            g = desaturate(g);
            b = desaturate(b);
            // And it blends into the backdrop, so the far cards sink into the
            // page instead of floating on it.
            let mix = |c: u32, bg: u8| (c * (100 - depth) as u32 + bg as u32 * depth as u32) / 100;
            img.put_pixel(x as u32, y as u32, image::Rgb([mix(r, backdrop.0).min(255) as u8, mix(g, backdrop.1).min(255) as u8, mix(b, backdrop.2).min(255) as u8]));
        }
    }
}

/// Apply the treatment for `treatment` to `img` (no-op when crisp).
pub fn treat(img: &mut image::RgbImage, treatment: Treatment, backdrop: Rgb) {
    if let Treatment::Recessed(depth) = treatment {
        recede(img, backdrop, depth);
    }
}

/// Longest side of the image a *transmitted* recessed cover is reduced to.
///
/// A neighbour is blurred and scaled by the terminal anyway, so sending it at
/// 600x900 costs ~500 KiB and seconds of CPU for pixels nobody can see. Capping
/// it here keeps both the payload and the work small.
pub const TRANSMIT_MAX_SIDE: u32 = 260;

/// Longest side a recessed cover is sent at, from how far back it sits.
///
/// This is the "low resolution" of the neighbours: the focused card keeps its
/// native pixels, and every step further back gets fewer, so the blur has less
/// and less to work with and the far cards cannot look sharper than the near
/// ones.
pub fn transmit_max_side(depth: u8) -> u32 {
    (TRANSMIT_MAX_SIDE as i32 - depth as i32 * 2).clamp(96, TRANSMIT_MAX_SIDE as i32) as u32
}

/// Size `img` would have after [`fit_for_transmit`]. Pure arithmetic, so the
/// caller can compute the crop rectangle of a *reduced* payload without
/// decoding it.
pub fn fit_size(img: (u32, u32), max_side: u32) -> (u32, u32) {
    let (w, h) = (img.0.max(1), img.1.max(1));
    let longest = w.max(h);
    if longest <= max_side {
        return (w, h);
    }
    let k = max_side as f32 / longest as f32;
    (
        ((w as f32 * k).round() as u32).max(1),
        ((h as f32 * k).round() as u32).max(1),
    )
}

/// Scale `img` so its longest side is at most `max_side`.
///
/// A proper filter, not nearest: the cover is about to be blurred, but with
/// nearest a 3x downscale simply throws two pixels out of every three, and the
/// blur then smooths that aliasing into wavy edges instead of into softness.
pub fn fit_for_transmit(img: &image::RgbImage, max_side: u32) -> image::RgbImage {
    let (w, h) = fit_size((img.width(), img.height()), max_side);
    if (w, h) == (img.width(), img.height()) {
        return img.clone();
    }
    image::imageops::resize(img, w, h, image::imageops::FilterType::Triangle)
}

/// A decoded cover: `w` x `h` pixel samples (h is always even).
#[derive(Debug, Clone)]
pub struct Cover {
    /// Width in pixels (= cover columns).
    pub w: usize,
    /// Height in pixels (= 2 * cover rows).
    pub h: usize,
    /// Packed RGB samples, row-major.
    pub px: Pixels,
}

impl Cover {
    /// Pixel at `(x, y)`, black outside bounds.
    fn at(&self, x: usize, y: usize) -> (u8, u8, u8) {
        if x >= self.w || y >= self.h {
            return (0, 0, 0);
        }
        let p = self.px[y * self.w + x];
        ((p >> 16) as u8, ((p >> 8) & 0xff) as u8, (p & 0xff) as u8)
    }

    /// Cover-fit an already resized image and apply the treatment.
    ///
    /// The heavy part (decode + triangle resize) happens once per file in
    /// [`CoverCache::master`]; this only crops and fades, which is cheap.
    pub fn from_samples(src: &image::RgbImage, w: usize, h: usize, treatment: Treatment, backdrop: Rgb) -> Self {
        let mut px = Pixels::with_capacity(w * h);
        let sw = src.width().max(1);
        let sh = src.height().max(1);
        for y in 0..h {
            for x in 0..w {
                let p = src.get_pixel((x as u32).min(sw - 1), (y as u32).min(sh - 1)).0;
                px.push((p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32);
            }
        }
        let mut cover = Self { w, h, px };
        if let Treatment::Recessed(depth) = treatment {
            let mut small = image::RgbImage::from_fn(w as u32, h as u32, |x, y| {
                let (r, g, b) = cover.at(x as usize, y as usize);
                image::Rgb([r, g, b])
            });
            recede(&mut small, backdrop, depth);
            for y in 0..h {
                for x in 0..w {
                    let p = small.get_pixel(x as u32, y as u32).0;
                    cover.px[y * w + x] = (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32;
                }
            }
        }
        cover
    }

    /// Load and cover-fit an image to `w` x `h` samples (centre crop).
    ///
    /// Uses the full `image` pipeline (PNG/JPEG/GIF/BMP/WEBP...), resized with
    /// a triangle filter for a clean downsample. `treatment` is applied *after*
    /// the downscale, so the blur costs nothing.
    ///
    /// `w` is the card's **full** width; a neighbour is cropped afterwards with
    /// [`Cover::crop`], so its artwork keeps the scale of the whole card instead
    /// of shrinking twice.
    #[cfg(test)]
    pub fn from_file(path: &Path, w: usize, h: usize, treatment: Treatment, backdrop: Rgb) -> Result<Self, String> {
        let img = image::open(path).map_err(|e| e.to_string())?;
        let mut rgb = img.to_rgb8();
        let (iw, ih) = (rgb.width() as usize, rgb.height() as usize);
        if iw == 0 || ih == 0 {
            return Err("empty image".into());
        }
        // Scale so the image covers the box, then centre-crop.
        let scale = (w as f64 / iw as f64).max(h as f64 / ih as f64);
        let (tw, th) = ((iw as f64 * scale).round() as u32, (ih as f64 * scale).round() as u32);
        rgb = image::imageops::resize(&rgb, tw.max(1), th.max(1), image::imageops::FilterType::Triangle);
        let x0 = tw.saturating_sub(w as u32) / 2;
        let y0 = th.saturating_sub(h as u32) / 2;
        let mut px = Pixels::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let s = rgb.get_pixel((x0 + x as u32).min(tw - 1), (y0 + y as u32).min(th - 1));
                px.push((s[0] as u32) << 16 | (s[1] as u32) << 8 | s[2] as u32);
            }
        }
        let mut cover = Self { w, h, px };
        if let Treatment::Recessed(depth) = treatment {
            // Recede on the already-downscaled buffer: a box blur on `w x h`
            // samples is what the player actually sees.
            let mut small = image::RgbImage::from_fn(w as u32, h as u32, |x, y| {
                let (r, g, b) = cover.at(x as usize, y as usize);
                image::Rgb([r, g, b])
            });
            recede(&mut small, backdrop, depth);
            for y in 0..h {
                for x in 0..w {
                    let p = small.get_pixel(x as u32, y as u32).0;
                    cover.px[y * w + x] = (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32;
                }
            }
        }
        Ok(cover)
    }

    /// Deterministic placeholder art: a "box art" card with the game's
    /// initials, generated from the title hash so the same game always looks
    /// the same. Three background styles keep a shelf from looking uniform.
    #[cfg(test)]
    pub fn placeholder(
        w: usize,
        h: usize,
        seed: u64,
        label: &str,
        tint: (u8, u8, u8),
        treatment: Treatment,
        backdrop: Rgb,
    ) -> Self {
        Self::placeholder_cropped(w, h, seed, label, tint, treatment, backdrop, Crop::FULL)
    }

    /// A generated card with only its visible slice drawn.
    ///
    /// The card is generated at full width and then cropped, so the artwork of
    /// the shelf lines up with the real covers instead of shrinking twice.
    pub fn placeholder_cropped(
        w: usize,
        h: usize,
        seed: u64,
        label: &str,
        tint: (u8, u8, u8),
        treatment: Treatment,
        backdrop: Rgb,
        crop: Crop,
    ) -> Self {
        let mut px = Pixels::with_capacity(w * h);
        let style = seed % 3;
        let (bg1, bg2) = match style {
            0 => ((16, 16, 30), (44, 14, 56)),
            1 => ((8, 24, 32), (10, 56, 60)),
            _ => ((30, 12, 16), (66, 26, 18)),
        };
        // A bright "label band" behind the initials, like a cartridge sticker.
        let band_h = (h / 5).max(3);
        let band_top = h / 2 - band_h / 2;
        for y in 0..h {
            for x in 0..w {
                let t = y as f64 / h.max(1) as f64;
                let mut c = (lerp(bg1.0, bg2.0, t), lerp(bg1.1, bg2.1, t), lerp(bg1.2, bg2.2, t));
                if y >= band_top && y < band_top + band_h {
                    // Darken the band so the initials pop.
                    c = ((c.0 as f64 * 0.55) as u8, (c.1 as f64 * 0.55) as u8, (c.2 as f64 * 0.55) as u8);
                }
                if (x + y * 2 + seed as usize) % 23 == 0 {
                    c = ((c.0 as f64 * 0.65) as u8, (c.1 as f64 * 0.65) as u8, (c.2 as f64 * 0.65) as u8);
                }
                px.push((c.0 as u32) << 16 | (c.1 as u32) << 8 | c.2 as u32);
            }
        }
        // Initials in a 5x7 bitmap, scaled to fit the card, centred.
        let chars: Vec<char> = label.chars().filter(|c| c.is_ascii_alphanumeric()).take(3).collect();
        let scale = ((w / 6).min(h / 8)).max(1);
        let glyphs: Vec<[u8; 7]> = chars.iter().map(|c| bitmap(*c)).collect();
        let text_w = chars.len() * 6 * scale;
        let start_x = w.saturating_sub(text_w) / 2;
        let start_y = h.saturating_sub(7 * scale) / 2;
        for (ci, g) in glyphs.iter().enumerate() {
            for row in 0..7usize {
                for col in 0..5usize {
                    if g[row] & (1 << (4 - col)) == 0 {
                        continue;
                    }
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let x = start_x + (ci * 6 + col) * scale + dx;
                            let y = start_y + row * scale + dy;
                            if x < w && y < h {
                                px[y * w + x] = (tint.0 as u32) << 16 | (tint.1 as u32) << 8 | tint.2 as u32;
                            }
                        }
                    }
                }
            }
        }
        // 1px frame in the platform tint, brighter on top (cartridge lip).
        for x in 0..w {
            px[x] = tint_packed(tint, 1.0);
            px[(h - 1) * w + x] = tint_packed(tint, 0.6);
        }
        for y in 0..h {
            px[y * w] = tint_packed(tint, 0.8);
            px[y * w + w - 1] = tint_packed(tint, 0.8);
        }
        let mut cover = Self { w, h, px };
        if let Treatment::Recessed(depth) = treatment {
            // A generated cover recedes too, or the shelf would look broken on
            // the games that have no artwork.
            let mut small = image::RgbImage::from_fn(w as u32, h as u32, |x, y| {
                let (r, g, b) = cover.at(x as usize, y as usize);
                image::Rgb([r, g, b])
            });
            recede(&mut small, backdrop, depth);
            for y in 0..h {
                for x in 0..w {
                    let p = small.get_pixel(x as u32, y as u32).0;
                    cover.px[y * w + x] = (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32;
                }
            }
        }
        cover.crop(crop)
    }

    /// Keep only the visible slice of the card.
    pub fn crop(&self, crop: Crop) -> Self {
        if crop.kept() == 0 || crop.kept() >= self.w {
            return self.clone();
        }
        // `keep_left` columns from the left edge, then `keep_right` from the
        // right one. The two arms used to disagree about which side they were
        // keeping, and `Crop::right(k)` returned `w - k` columns: a neighbour
        // 19 columns wide was drawn with 16 and the leftover cells showed up as
        // blank gaps inside the band.
        let left = crop.keep_left.min(self.w);
        let right = crop.keep_right.min(self.w.saturating_sub(left));
        let out_w = left + right;
        let mut px = Pixels::with_capacity(out_w * self.h);
        for y in 0..self.h {
            for x in 0..left {
                px.push(self.px[y * self.w + x]);
            }
            for x in 0..right {
                px.push(self.px[y * self.w + self.w - right + x]);
            }
        }
        Self { w: out_w, h: self.h, px }
    }

    /// Render to `cover_h` character rows of half blocks.
    pub fn rows(&self, theme: &Theme) -> Vec<String> {
        if theme.art {
            self.rows_color(theme)
        } else {
            self.rows_plain()
        }
    }

    /// Half blocks with merged SGR runs.
    fn rows_color(&self, theme: &Theme) -> Vec<String> {
        let mut out = Vec::with_capacity(self.h / 2);
        for cy in 0..self.h / 2 {
            let mut line = String::new();
            let mut last_fg: Option<(u8, u8, u8)> = None;
            let mut last_bg: Option<(u8, u8, u8)> = None;
            for x in 0..self.w {
                let fg = self.at(x, cy * 2);
                let bg = self.at(x, cy * 2 + 1);
                if last_fg != Some(fg) {
                    line.push_str(&theme.rgb(fg.0, fg.1, fg.2));
                    last_fg = Some(fg);
                }
                if last_bg != Some(bg) {
                    line.push_str(&theme.bg(bg.0, bg.1, bg.2));
                    last_bg = Some(bg);
                }
                line.push_str(glyph::HALF);
            }
            if theme.depth != Depth::Plain {
                line.push_str("\x1b[0m");
            }
            out.push(line);
        }
        out
    }

    /// ASCII fallback (no color): luminance ramp with half/full blocks.
    fn rows_plain(&self) -> Vec<String> {
        const RAMP: [&str; 4] = [" ", "░", "▒", crate::theme::glyph::FULL];
        let mut out = Vec::with_capacity(self.h / 2);
        for cy in 0..self.h / 2 {
            let mut line = String::new();
            for x in 0..self.w {
                let (r1, g1, b1) = self.at(x, cy * 2);
                let (r2, g2, b2) = self.at(x, cy * 2 + 1);
                let l = (0.299 * r1 as f64 + 0.587 * g1 as f64 + 0.114 * b1 as f64
                    + 0.299 * r2 as f64
                    + 0.587 * g2 as f64
                    + 0.114 * b2 as f64)
                    / 2.0;
                line.push_str(RAMP[(l / 64.0).clamp(0.0, 3.0) as usize]);
            }
            out.push(line);
        }
        out
    }
}

fn lerp(a: u8, b: u8, t: f64) -> u8 {
    (a as f64 + (b as f64 - a as f64) * t.clamp(0.0, 1.0)).round().clamp(0.0, 255.0) as u8
}

/// Platform tint scaled by `k` (dimmer variants for the frame edges).
fn tint_packed(tint: (u8, u8, u8), k: f64) -> u32 {
    ((tint.0 as f64 * k) as u32) << 16 | ((tint.1 as f64 * k) as u32) << 8 | ((tint.2 as f64 * k) as u32)
}

#[allow(dead_code)]
fn tint_of(style: u64, x: usize) -> u32 {
    let base = match style {
        0 => (255u8, 105u8, 180u8),
        1 => (90, 220, 255),
        _ => (255, 205, 90),
    };
    let k = 0.6 + 0.4 * ((x / 3) % 2) as f64;
    ((base.0 as f64 * k) as u32) << 16 | ((base.1 as f64 * k) as u32) << 8 | (base.2 as f64 * k) as u32
}

/// 5x7 bitmap for A-Z, 0-9 and a few marks (bit 4 = leftmost column).

/// 5x7 bitmap for the characters a title can start with (bit 4 = leftmost
/// column). Only the initials are drawn, so a compact hand-set font is enough;
/// anything unknown falls back to a filled block so a label never disappears.
fn bitmap(c: char) -> [u8; 7] {
    const GLYPHS: &[(char, [u8; 7])] = &[
        ('A', [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11]),
        ('B', [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e]),
        ('C', [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e]),
        ('D', [0x1e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1e]),
        ('E', [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f]),
        ('F', [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10]),
        ('G', [0x0e, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0f]),
        ('H', [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11]),
        ('I', [0x0e, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e]),
        ('J', [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0c]),
        ('K', [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11]),
        ('L', [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f]),
        ('M', [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11]),
        ('N', [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11]),
        ('O', [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e]),
        ('P', [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10]),
        ('Q', [0x0e, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0d]),
        ('R', [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11]),
        ('S', [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e]),
        ('T', [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04]),
        ('U', [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e]),
        ('V', [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04]),
        ('W', [0x11, 0x11, 0x11, 0x15, 0x15, 0x1b, 0x11]),
        ('X', [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11]),
        ('Y', [0x11, 0x11, 0x0a, 0x04, 0x04, 0x04, 0x04]),
        ('Z', [0x1f, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1f]),
        ('0', [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e]),
        ('1', [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e]),
        ('2', [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f]),
        ('3', [0x1f, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0e]),
        ('4', [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02]),
        ('5', [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e]),
        ('6', [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e]),
        ('7', [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08]),
        ('8', [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e]),
        ('9', [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c]),
    ];
    let up = c.to_ascii_uppercase();
    GLYPHS.iter().find(|(g, _)| *g == up).map(|(_, b)| *b).unwrap_or([0x1f; 7])
}

/// Stable seed from a title, so a generated cover never changes between runs.
fn seed_of(title: &str) -> u64 {
    // FNV-1a: short, stable, no dependencies.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in title.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Platform colour used for the generated card and its frame.
fn platform_tint(dir: &str) -> (u8, u8, u8) {
    match dir {
        "snes" => (255, 205, 90),
        "n64" => (90, 220, 255),
        "gamecube" => (150, 190, 255),
        "wii" => (255, 105, 180),
        _ => (232, 234, 240), // 3ds and anything new
    }
}

/// Decode + artwork cache, keyed by everything that changes the pixels.
///
/// Treatment and backdrop are part of the key on purpose: a recessed cover and
/// a cover recessed towards a different palette are different images, and a
/// pywal reload must not keep serving the old ones.
#[derive(Debug, Default)]
pub struct CoverCache {
    /// Decoded and resized artwork, shared by every treatment and crop of the
    /// same card size: the decode plus the triangle resize is the expensive part
    /// (70 ms for a 600x900 file at 45x60), and the fan has seven cards per
    /// frame. Memoized, so it is paid once per file per session.
    masters: HashMap<(PathBuf, usize, usize), Option<Arc<image::RgbImage>>>,
    /// Biggest box decoded per file: every smaller card (the neighbours of the
    /// fan) is derived from it instead of decoding the 600x900 file again.
    biggest: HashMap<PathBuf, (usize, usize, Arc<image::RgbImage>)>,
    /// Files verified to decode (or not), so the detail pane does not have to
    /// ask the artwork cache and resize the whole file to learn a fact.
    readable: HashMap<PathBuf, bool>,
    map: HashMap<(PathBuf, usize, usize, Treatment, Rgb, Crop), Option<Arc<Cover>>>,
    resolved: HashMap<PathBuf, Option<PathBuf>>,
    placeholders: HashMap<(PathBuf, usize, usize, Treatment, Rgb, Crop), Arc<Cover>>,
}

impl CoverCache {
    /// New empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve (and memoize) the cover file for a ROM.
    pub fn art_path(&mut self, rom: &Path) -> Option<PathBuf> {
        if let Some(hit) = self.resolved.get(rom) {
            return hit.clone();
        }
        let found = rosadeck_game_library::resolve_artwork(rom).map(|a| a.cover);
        self.resolved.insert(rom.to_path_buf(), found.clone());
        found
    }

    /// The same, for one card of the fan: a neighbour is generated at the full
    /// card width and then cropped, so generated and real artwork line up.
    pub fn cover_cropped(
        &mut self,
        rom: &Path,
        w: usize,
        h: usize,
        platform: crate::Platform,
        title: &str,
        treatment: Treatment,
        backdrop: Rgb,
        crop: Crop,
    ) -> Arc<Cover> {
        let art = self.art_path(rom);
        if let Some(c) = self.get_cropped(art.as_deref(), w, h, treatment, backdrop, crop) {
            return c;
        }
        let key = (rom.to_path_buf(), w, h, treatment, backdrop, crop);
        if let Some(c) = self.placeholders.get(&key) {
            return c.clone();
        }
        let generated = Arc::new(Cover::placeholder_cropped(
            w,
            h,
            seed_of(title),
            &crate::frame::initials(title, platform),
            platform_tint(platform.dir_name()),
            treatment,
            backdrop,
            crop,
        ));
        self.placeholders.insert(key, generated.clone());
        generated
    }

    /// [`Self::get_treated`] with the default (crisp) treatment.
    ///
    /// Only the tests use this: the carousel always goes through
    /// [`Self::get_cropped`], because a neighbour is always cropped.
    #[cfg(test)]
    pub fn get(&mut self, path: Option<&Path>, w: usize, h: usize) -> Option<Arc<Cover>> {
        self.get_treated(path, w, h, Treatment::Crisp, (0, 0, 0))
    }

    /// Does this file really decode? Memoized per path.
    ///
    /// The detail pane needs this to tell a cover apart from an unreadable one,
    /// and it needs nothing else from the image. Asking the artwork cache
    /// instead meant decoding *and* resizing the full 600x900 file every time
    /// the cursor reached a game whose cover had not been touched yet: 150 ms
    /// to write one word of text.
    pub fn decodes(&mut self, path: &Path) -> bool {
        if let Some(hit) = self.readable.get(path) {
            return *hit;
        }
        // A full decode, not just the header: `image_dimensions` accepts files
        // that are truncated past the header, and the pane claims "unreadable"
        // only when the decoder really refuses.
        let ok = image::open(path).is_ok();
        self.readable.insert(path.to_path_buf(), ok);
        ok
    }

    /// Get (and memoize) the visible slice of a card.
    ///
    /// `w`/`h` are the *full* card; `crop` keeps the outer edge that is not
    /// hidden behind the closer neighbour.
    pub fn get_cropped(
        &mut self,
        path: Option<&Path>,
        w: usize,
        h: usize,
        treatment: Treatment,
        backdrop: Rgb,
        crop: Crop,
    ) -> Option<Arc<Cover>> {
        let key = (path.map(Path::to_path_buf).unwrap_or_default(), w, h, treatment, backdrop, crop);
        if let Some(hit) = self.map.get(&key) {
            return hit.clone();
        }
        let loaded = match path {
            None => None,
            Some(p) => {
                let master = self.master(p, w, h)?;
                let cover = Cover::from_samples(&master, w, h, treatment, backdrop).crop(crop);
                Some(Arc::new(cover))
            }
        };
        self.map.insert(key, loaded.clone());
        loaded
    }

    /// Decoded artwork at `w` x `h`.
    ///
    /// Decoding the file and the triangle resize are the expensive part, so the
    /// first (biggest) box for a file is decoded once and every smaller card —
    /// each neighbour of the fan — is a cheap downscale of that master.
    fn master(&mut self, path: &Path, w: usize, h: usize) -> Option<Arc<image::RgbImage>> {
        let key = (path.to_path_buf(), w, h);
        if let Some(hit) = self.masters.get(&key) {
            return hit.clone();
        }
        if let Some((bw, bh, big)) = self.biggest.get(path).cloned() {
            if (bw, bh) != (w, h) {
                let small = image::imageops::resize(
                    big.as_ref(),
                    w as u32,
                    h as u32,
                    image::imageops::FilterType::Triangle,
                );
                let arc = Arc::new(small);
                self.masters.insert(key, Some(arc.clone()));
                return Some(arc);
            }
        }
        let img = image::open(path).ok().map(|i| i.to_rgb8());
        let sized = img.map(|src| {
            let (iw, ih) = (src.width() as usize, src.height() as usize);
            if iw == 0 || ih == 0 {
                return src;
            }
            // Cover-fit: scale so the image covers the box, then centre-crop.
            let scale = (w as f64 / iw as f64).max(h as f64 / ih as f64);
            let (tw, th) = ((iw as f64 * scale).round() as u32, (ih as f64 * scale).round() as u32);
            let mut resized = image::imageops::resize(&src, tw.max(1), th.max(1), image::imageops::FilterType::Triangle);
            let (x0, y0) = (tw.saturating_sub(w as u32) / 2, th.saturating_sub(h as u32) / 2);
            let mut out = image::RgbImage::new(w as u32, h as u32);
            for y in 0..h as u32 {
                for x in 0..w as u32 {
                    out.put_pixel(x, y, *resized.get_pixel((x0 + x).min(tw - 1), (y0 + y).min(th - 1)));
                }
            }
            resized = out;
            resized
        });
        let arc = sized.map(Arc::new);
        if let Some(a) = &arc {
            self.biggest.insert(path.to_path_buf(), (w, h, a.clone()));
        }
        self.masters.insert(key, arc.clone());
        arc
    }

    /// Get (and memoize) the cover for `path` with a treatment.
    ///
    /// Only the tests use this: the carousel goes through
    /// [`Self::get_cropped`] so a neighbour keeps its outer edge.
    #[cfg(test)]
    pub fn get_treated(
        &mut self,
        path: Option<&Path>,
        w: usize,
        h: usize,
        treatment: Treatment,
        backdrop: Rgb,
    ) -> Option<Arc<Cover>> {
        let key = (path.map(Path::to_path_buf).unwrap_or_default(), w, h, treatment, backdrop, Crop::FULL);
        if let Some(hit) = self.map.get(&key) {
            return hit.clone();
        }
        let loaded = match path {
            None => None,
            Some(p) => {
                let master = self.master(p, w, h)?;
                Some(Arc::new(Cover::from_samples(&master, w, h, treatment, backdrop)))
            }
        };
        self.map.insert(key, loaded.clone());
        loaded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK: Rgb = (18, 18, 24);

    fn magenta(dir: &str, name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        image::RgbImage::from_pixel(60, 90, image::Rgb([255, 0, 128])).save(&path).unwrap();
        path
    }

    /// A 10-wide cover whose columns are all different, so a crop says which ones
    /// it kept.
    fn striped() -> Cover {
        let mut img = image::RgbImage::new(10, 2);
        for x in 0..10u32 {
            let v = (x * 20) as u8;
            for y in 0..2u32 {
                img.put_pixel(x, y, image::Rgb([v, v, v]));
            }
        }
        Cover::from_samples(&img, 10, 2, Treatment::Crisp, BLACK)
    }

    fn column_values(cover: &Cover) -> Vec<u32> {
        (0..cover.w).map(|x| cover.px[x] & 0xff).collect()
    }

    /// A neighbour shows its *outer* edge: `left` keeps the leftmost columns,
    /// `right` the rightmost, and both return exactly as many as asked.
    #[test]
    fn cropping_keeps_the_outer_edge_of_the_side_it_names() {
        let full = column_values(&striped());
        assert_eq!(full, (0..10).map(|x| x * 20).collect::<Vec<_>>());

        let left = striped().crop(Crop::left(3));
        assert_eq!(left.w, 3, "left keeps exactly the columns asked for");
        assert_eq!(column_values(&left), vec![0, 20, 40]);

        let right = striped().crop(Crop::right(3));
        assert_eq!(right.w, 3, "right keeps exactly the columns asked for");
        assert_eq!(column_values(&right), vec![140, 160, 180]);

        // Asking for more than there is leaves the cover alone instead of
        // producing a short row with holes.
        assert_eq!(striped().crop(Crop::right(40)).w, 10);
        assert_eq!(striped().crop(Crop::FULL).w, 10);
        // Both edges at once: left part then right part, no overlap.
        let both = striped().crop(Crop { keep_left: 2, keep_right: 3 });
        assert_eq!(column_values(&both), vec![0, 20, 140, 160, 180]);
    }

    /// Chroma spread: max channel minus min channel, averaged over pixels.
    fn chroma(px: &Pixels) -> u32 {
        let mut total = 0u64;
        for p in px {
            let r = (p >> 16) as u32;
            let g = ((p >> 8) & 0xff) as u32;
            let b = (p & 0xff) as u32;
            total += (r.max(g).max(b) - r.min(g).min(b)) as u64;
        }
        (total / px.len().max(1) as u64) as u32
    }

    #[test]
    #[ignore = "insights de rendimiento, no assertions"]
    fn perf_probe_of_the_treatment() {
        use std::time::Instant;
        let dir = std::env::temp_dir().join("rosadeck-perf");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cover.png");
        let mut img = image::RgbImage::new(600, 900);
        let mut seed = 12345u32;
        for p in img.pixels_mut() {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            p.0 = [seed as u8, (seed >> 8) as u8, (seed >> 16) as u8];
        }
        img.save(&path).unwrap();
        println!("png en disco: {} KiB", std::fs::metadata(&path).unwrap().len() / 1024);

        let t = Instant::now();
        let full = image::open(&path).unwrap().to_rgb8();
        println!("decode 600x900: {:?}", t.elapsed());

        let t = Instant::now();
        let mut small = image::imageops::resize(&full, 45, 60, image::imageops::FilterType::Triangle);
        println!("resize a 45x60 (celda): {:?}", t.elapsed());
        let t = Instant::now();
        recede(&mut small, (18, 18, 24), 60);
        println!("recede 45x60: {:?}", t.elapsed());

        let t = Instant::now();
        let mut native = full.clone();
        recede(&mut native, (18, 18, 24), 60);
        println!("recede 600x900 (nativo, lo que hacia images.rs): {:?}", t.elapsed());
        let t = Instant::now();
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(native)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        println!("encode png 600x900: {:?} -> {} KiB", t.elapsed(), png.len() / 1024);

        // Lo que hace de verdad la ruta de imágenes para un vecino.
        let t = Instant::now();
        let mut small2 = fit_for_transmit(&full, TRANSMIT_MAX_SIDE);
        treat(&mut small2, Treatment::Recessed(50), (18, 18, 24));
        let (w2, h2) = (small2.width(), small2.height());
        let mut png2 = Vec::new();
        image::DynamicImage::ImageRgb8(small2)
            .write_to(&mut std::io::Cursor::new(&mut png2), image::ImageFormat::Png)
            .unwrap();
        println!("vecino (fit_for_transmit+recede+encode): {:?} -> {} KiB ({w2}x{h2})", t.elapsed(), png2.len() / 1024);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_recessed_cover_is_desaturated_and_faded_but_not_dead() {
        let file = magenta("rosadeck-cover-recede", "magenta.png");
        let crisp = Cover::from_file(&file, 30, 40, Treatment::Crisp, BLACK).unwrap();
        let behind = Cover::from_file(&file, 30, 40, Treatment::Recessed(50), BLACK).unwrap();
        assert_eq!((crisp.w, crisp.h), (30, 40));
        assert!(chroma(&behind.px) < chroma(&crisp.px) / 3, "desaturated: {} -> {}", chroma(&crisp.px), chroma(&behind.px));
        // Faded towards the (dark) backdrop, but never black-on-black.
        let avg = |c: &Cover| c.px.iter().map(|p| ((p >> 16) as u32 + ((p >> 8) & 0xff) as u32 + (p & 0xff) as u32) / 3).sum::<u32>() / c.px.len() as u32;
        assert!(avg(&behind) < avg(&crisp), "faded: {} -> {}", avg(&crisp), avg(&behind));
        assert!(avg(&behind) > 12, "still visible on a dark backdrop: {}", avg(&behind));
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn a_recessed_cover_blurs_its_neighbours() {
        // Two vertical bars: a crisp cover keeps the step, a recessed one
        // cannot.
        let dir = std::env::temp_dir().join("rosadeck-cover-blur");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("bars.png");
        // Left half white, right half black: a hard vertical edge.
        let mut img = image::RgbImage::from_pixel(40, 20, image::Rgb([0, 0, 0]));
        for y in 0..20 {
            for x in 0..20 {
                img.put_pixel(x, y, image::Rgb([255, 255, 255]));
            }
        }
        img.save(&file).unwrap();
        // Sharpness = the steepest step between neighbouring columns. A crisp
        // cover has one hard edge of 255; a blurred one spreads it over the
        // kernel, so no single step is that steep any more.
        let steepest = |c: &Cover| -> u32 {
            let mid = c.h / 2;
            let mut worst = 0u32;
            for x in 1..c.w {
                let a = c.at(x - 1, mid).0 as i32;
                let b = c.at(x, mid).0 as i32;
                worst = worst.max((a - b).unsigned_abs());
            }
            worst
        };
        let crisp = Cover::from_file(&file, 40, 20, Treatment::Crisp, BLACK).unwrap();
        let behind = Cover::from_file(&file, 40, 20, Treatment::Recessed(50), BLACK).unwrap();
        assert!(steepest(&crisp) > 200, "the bar edge is hard when crisp: {}", steepest(&crisp));
        assert!(
            steepest(&behind) < steepest(&crisp) / 2,
            "and softened when recessed: {} -> {}",
            steepest(&crisp),
            steepest(&behind)
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_backdrop_changes_the_result_so_light_themes_also_work() {
        let file = magenta("rosadeck-cover-backdrop", "magenta.png");
        let dark = Cover::from_file(&file, 20, 20, Treatment::Recessed(50), (0, 0, 0)).unwrap();
        let light = Cover::from_file(&file, 20, 20, Treatment::Recessed(50), (240, 240, 240)).unwrap();
        let avg = |c: &Cover| c.px.iter().map(|p| ((p >> 16) as u32 + ((p >> 8) & 0xff) as u32 + (p & 0xff) as u32) / 3).sum::<u32>() / c.px.len() as u32;
        assert!(avg(&light) > avg(&dark), "receding on a light theme gets lighter, not darker: {} -> {}", avg(&dark), avg(&light));
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn generated_covers_also_recede() {
        let crisp = Cover::placeholder(30, 40, 7, "LM", platform_tint("wii"), Treatment::Crisp, BLACK);
        let behind = Cover::placeholder(30, 40, 7, "LM", platform_tint("wii"), Treatment::Recessed(50), BLACK);
        assert!(chroma(&behind.px) < chroma(&crisp.px), "generated covers fade too");
        assert_eq!((behind.w, behind.h), (30, 40));
    }

    #[test]
    fn cache_keys_on_treatment_and_backdrop() {
        let dir = std::env::temp_dir().join("rosadeck-cover-cache");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("c.png");
        image::RgbImage::from_pixel(20, 20, image::Rgb([255, 0, 128])).save(&file).unwrap();
        let mut c = CoverCache::new();
        let crisp = c.get_treated(Some(&file), 20, 20, Treatment::Crisp, BLACK).unwrap();
        let same = c.get_treated(Some(&file), 20, 20, Treatment::Crisp, BLACK).unwrap();
        let behind = c.get_treated(Some(&file), 20, 20, Treatment::Recessed(50), BLACK).unwrap();
        assert!(Arc::ptr_eq(&crisp, &same), "same request is memoized");
        assert!(!Arc::ptr_eq(&crisp, &behind), "a different treatment is a different image");
        assert!(chroma(&behind.px) < chroma(&crisp.px));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_art_is_never_an_error() {
        let mut c = CoverCache::new();
        assert!(c.get(Some(Path::new("/nope/rosadeck/x.png")), 8, 8).is_none());
        assert!(c.get(None, 8, 8).is_none());
        // And a generated cover is always available.
        let cover = c.cover_cropped(Path::new("/nope/rosadeck/x.rom"), 8, 8, crate::Platform::Wii, "Missing", Treatment::Crisp, BLACK, Crop::FULL);
        assert_eq!((cover.w, cover.h), (8, 8));
    }

    #[test]
    fn bitmap_font_covers_the_characters_a_title_can_start_with() {
        for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".chars() {
            let g = bitmap(c);
            assert!(g.iter().any(|row| *row != 0), "{c} is not blank");
            assert!(g.iter().all(|row| row & !0b1110_0000 == *row), "{c} stays in 5 columns");
        }
        assert_eq!(bitmap('a'), bitmap('A'), "case insensitive");
        assert!(bitmap('~').iter().all(|r| *r == 0x1f), "unknown characters are a block, never nothing");
    }

    #[test]
    fn rows_use_half_blocks_and_keep_the_requested_height() {
        let cover = Cover::placeholder(12, 8, 3, "MP", platform_tint("n64"), Treatment::Crisp, BLACK);
        let theme = crate::theme::Theme::builtin(crate::theme::Depth::Truecolor, true);
        let rows = cover.rows(&theme);
        assert_eq!(rows.len(), 4, "8 samples = 4 character rows");
        assert!(rows[0].contains(crate::theme::glyph::HALF));
    }
}
