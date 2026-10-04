//! Icons for the focused card's name.
//!
//! Two sets, because a Nerd Font is a *choice* and cannot be assumed:
//!
//! * **default**: symbols that exist in any terminal font. Verified against the
//!   font this machine's kitty actually resolves to (Noto Sans Mono, since
//!   `kitty.conf` sets no `font_family`): `★`/`☆` are **not** in it, so the star
//!   was rendering as a tofu box before this existed.
//! * **Nerd Font**: private-use glyphs, only used when the user says their
//!   terminal has one (`--nerd-fonts` / `ROSADECK_NERD_FONTS=1`). Getting it
//!   wrong is not cosmetic: an absent glyph is a box, not a missing decoration.
//!
//! The Nerd Font codepoints come from Font Awesome and Material Design Icons
//! ranges, the two families virtually every Nerd Font ships.

/// A set of glyphs, chosen once per run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Icons {
    /// Favourite.
    pub fav_on: &'static str,
    /// Not a favourite.
    pub fav_off: &'static str,
    /// Prefix of the "played" figure (`▶ 3x`, `▶ nunca`).
    pub played: &'static str,
}

/// Plain Unicode: present in Noto Sans Mono and most terminal fonts (checked
/// against its cmap, not assumed).
const PLAIN: Icons = Icons {
    // U+25C9 / U+25CB: fisheye-ish ring, the closest thing to a star that any
    // font carries. `★` (U+2605) is *not* in Noto Sans Mono.
    fav_on: "◉",
    fav_off: "○",
    played: "▶", // U+25B6
};

/// Font Awesome (`nf-fa-*`) range, which every Nerd Font ships.
const NERD: Icons = Icons {
    fav_on: "\u{f005}",  // star
    fav_off: "\u{f006}", // star-o
    played: "\u{f04b}",  // play
};

impl Icons {
    /// Which set to use: the Nerd Font one only when the user says so.
    pub fn detect() -> Self {
        let asked = std::env::var("ROSADECK_NERD_FONTS").map(|v| v != "0").unwrap_or(false);
        if asked {
            NERD
        } else {
            PLAIN
        }
    }

    /// For tests: force a set.
    #[cfg(test)]
    pub fn nerd() -> Self {
        NERD
    }

    /// Every glyph in the set, so a test can assert none is empty.
    #[cfg(test)]
    pub fn all(&self) -> [&'static str; 3] {
        [self.fav_on, self.fav_off, self.played]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_set_stays_out_of_private_use() {
        // Private use is exactly where a Nerd Font lives, so a default glyph
        // there is a tofu box on any font without the patch. Everything in the
        // default set is instead Latin-1 or a Geometric Shape/Arrow/Misc symbol
        // block, which is what was verified present in this machine's kitty font
        // (Noto Sans Mono: it has ◉ ○ ▶ ▸ ◆ · ● and, notably, **not** ★ U+2605 —
        // the star the UI used before was already a box).
        for g in PLAIN.all() {
            assert!(!g.is_empty(), "glifo vacío");
            for c in g.chars() {
                let cp = c as u32;
                assert!(
                    !(0xe000..=0xf8ff).contains(&cp) && cp <= 0x26ff,
                    "{g:?} (U+{cp:04X}) no es un símbolo universal"
                );
            }
        }
    }

    #[test]
    fn the_nerd_set_is_private_use_and_the_default_is_not() {
        for g in NERD.all() {
            assert!(g.chars().next().is_some_and(|c| (0xe000..=0xf8ff).contains(&(c as u32))), "{g:?} no es Nerd Font");
        }
        assert_ne!(Icons::detect(), Icons::nerd(), "sin la bandera no se usan los Nerd Font");
    }
}