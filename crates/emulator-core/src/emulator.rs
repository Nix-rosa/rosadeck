//! Small explicit errors for the launch domain.

use std::fmt;

/// Launch-domain failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmulatorError {
    /// Binary not found (PATH or absolute path).
    BinaryNotFound(String),
    /// Invalid profile/shapePrevent.
    InvalidProfile(String),
    /// `prepare` could not build the launch description.
    PrepareFailed(String),
    /// `spawn` failed (binary missing at exec time, OS error, …).
    LaunchFailed(String),
    /// Requested combination unsupported (e.g. slangp shader on Dolphin).
    Unsupported(String),
}

impl fmt::Display for EmulatorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BinaryNotFound(b) => write!(f, "binary not found: {b}"),
            Self::InvalidProfile(m) => write!(f, "invalid emulator profile: {m}"),
            Self::PrepareFailed(m) => write!(f, "prepare failed: {m}"),
            Self::LaunchFailed(m) => write!(f, "launch failed: {m}"),
            Self::Unsupported(m) => write!(f, "unsupported: {m}"),
        }
    }
}

impl std::error::Error for EmulatorError {}

use crate::launch::{LaunchContext, PreparedLaunch};
use crate::process::{ChildHandle, Spawner};

/// Emulator adapter contract: describe how to run, never touch displays.
///
/// - [`Emulator::prepare`] is pure description (testable without processes).
/// - [`Emulator::spawn`] starts the child through a [`Spawner`] (stubbed in tests).
/// - [`Emulator::cleanup`] must be idempotent (called on exit, crash, signal).
pub trait Emulator {
    /// Stable adapter id (`retroarch`, `dolphin`, …).
    fn id(&self) -> &'static str;

    /// Locate the executable: absolute `binary_path` wins, else PATH lookup
    /// of `binary`. No installation, no downloads, no distro tooling.
    fn resolve_binary(&self, binary: &str, binary_path: Option<&str>) -> Result<std::path::PathBuf, EmulatorError> {
        if let Some(abs) = binary_path {
            let p = std::path::PathBuf::from(abs);
            if p.is_absolute() && p.exists() {
                return Ok(p);
            }
            return Err(EmulatorError::BinaryNotFound(abs.into()));
        }
        resolve_in_path(binary).ok_or_else(|| EmulatorError::BinaryNotFound(binary.into()))
    }

    /// Build the exact `program + args + env + workdir` to run.
    fn prepare(&self, ctx: &LaunchContext) -> Result<PreparedLaunch, EmulatorError>;

    /// Last chance to adjust the launch *right before spawning*.
    ///
    /// `prepare` must stay inspectable, so it never writes; an emulator that
    /// needs a generated config file (Snes9x fullscreen) materialises it here.
    /// The CLI calls this only when it is really going to spawn, which keeps
    /// `--dry-run` free of side effects.
    fn finalize(&self, _prepared: &mut PreparedLaunch) -> Result<(), EmulatorError> {
        Ok(())
    }

    /// Spawn a prepared launch.
    fn spawn(
        &self,
        prepared: &PreparedLaunch,
        spawner: &dyn Spawner,
    ) -> Result<Box<dyn ChildHandle>, EmulatorError> {
        spawner.spawn(prepared).map_err(|e| EmulatorError::LaunchFailed(e.to_string()))
    }

    /// Release adapter resources. Idempotent: second call is a no-op success.
    /// F5 adapters hold no resources, so the default is a no-op.
    fn cleanup(&self, _ctx: &LaunchContext) -> Result<(), EmulatorError> {
        Ok(())
    }
}

/// PATH lookup without external tooling (portable, no `which` binary needed).
pub fn resolve_in_path(binary: &str) -> Option<std::path::PathBuf> {
    if binary.contains('/') {
        let p = std::path::PathBuf::from(binary);
        return p.exists().then_some(p);
    }
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let p = dir.join(binary);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_absolute_and_path_binaries() {
        assert!(resolve_in_path("sh").is_some());
        assert!(resolve_in_path("definitely-not-a-rosadeck-binary").is_none());
        assert!(resolve_in_path("/bin/sh").is_some());
    }
}
