//! `rosadeck-emulator-core`: emulator-agnostic launch model.
//!
//! Boundary (audited): this crate and its adapters never touch displays —
//! no `hyprctl`, no snapshot/apply/restore, no sockets, no sysfs. The launch
//! pipeline (CLI layer) requests display work through the F3 backend and
//! drives an [`EmulatorSession`]; adapters only describe *how to run* an
//! emulator (`prepare` is fully inspectable without spawning anything).

pub mod emulator;
pub mod game;
pub mod launch;
pub mod process;
pub mod session;
pub mod stub;

pub use emulator::{resolve_in_path, Emulator, EmulatorError};
pub use game::GameTarget;
pub use launch::{LaunchContext, PreparedLaunch};
pub use process::{ChildHandle, QuietSpawner, Spawner, StdSpawner};
pub use session::{DisplayLostOutcome, EmulatorSession, SessionState};
