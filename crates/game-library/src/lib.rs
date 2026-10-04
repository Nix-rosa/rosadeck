//! `rosadeck-game-library`: the library is the center, not flags.
//!
//! Scan configured ROM roots → identify platform by directory + extension →
//! parse No-Intro style titles → resolve artwork by convention → track local
//! stats (favorites, plays). No launching, no network, no scraping here.

pub mod art;
pub mod platform;
pub mod scan;
pub mod stats;
pub mod title;

pub use art::{
    candidate_dirs, candidate_names, resolve_artwork, suggested_cover, suggested_paths, Artwork, ART_EXTENSIONS, COVERS_DIR,
};
pub use platform::{platform_for, Platform};
pub use scan::{
    configured_roots, default_roots, dir_with_legacy, expand_home, roots_config_path, save_configured_roots, scan_roots,
    GameEntry,
};
pub use stats::{LibraryStats, StatsDb};
pub use title::{parse_title, ParsedTitle};
