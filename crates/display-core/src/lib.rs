//! `display-core`: pure display-domain types and logic.
//!
//! This crate never talks to Hyprland, sysfs, udev or external commands.
//! It only defines how a [`Mode`] is represented, normalized, intersected
//! and selected. All IO lives in F2 (`backend-hyprland`).

pub mod identity;
pub mod layout;
pub mod mode;
pub mod policy;

pub use identity::DisplayIdentity;
pub use layout::{ConnectorId, DisplayLayout, DisplaySnapshot, Mirror, OutputId, OutputState, Position, Scale};
pub use mode::{intersect_modes, merge_modes, parse_mode, Mode, ModeSource, RefreshKind, SourcedMode, HZ_TOLERANCE, HZ_UNKNOWN};
pub use policy::{select_mode, ModePolicy, PolicyContext};
