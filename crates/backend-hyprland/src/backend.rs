//! Read-only [`DisplayBackend`] trait + Hyprland socket implementation.
//!
//! F2 exposes only reads. No mutating method exists here on purpose —
//! `apply`/`snapshot` arrive in F3. The trait lives so F3 can add capability
//! without touching `display-core`.

use crate::model::{normalize, HyprMonitor, HyprMonitorInfo};
use display_core::{Mode, OutputState};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

/// Backend failure (all read-path).
#[derive(Debug)]
pub enum BackendError {
    /// Environment (`XDG_RUNTIME_DIR` / `HYPRLAND_INSTANCE_SIGNATURE`).
    Env(String),
    /// Socket IO.
    Socket(String),
    /// `monitors all -j` reply is not the expected JSON.
    Json(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Env(m) => write!(f, "env: {m}"),
            Self::Socket(m) => write!(f, "socket: {m}"),
            Self::Json(m) => write!(f, "json: {m}"),
        }
    }
}

impl std::error::Error for BackendError {}

/// Shorthand for backend read results.
pub type BackendResult<T> = Result<T, BackendError>;

/// Read-only display backend. F2: queries only.
pub trait DisplayBackend {
    /// Backend identifier for `status` output (e.g. `"Hyprland"`).
    fn backend_name(&self) -> &'static str;
    /// All outputs known to the compositor (normalized).
    fn list_outputs(&self) -> BackendResult<Vec<(OutputState, HyprMonitorInfo)>>;
    /// One output by compositor name.
    fn get_output(&self, name: &str) -> BackendResult<Option<(OutputState, HyprMonitorInfo)>>;
    /// Modes the compositor will accept for `name` (Hz preserved exactly).
    fn available_modes(&self, name: &str) -> BackendResult<Vec<Mode>>;
}

/// Hyprland IPC backend (synchronous socket request per call; the socket is
/// opened, read and closed each time — never held open, per Hyprland docs).
#[derive(Debug, Default, Clone, Copy)]
pub struct HyprlandBackend;

fn socket_path() -> BackendResult<String> {
    let runtime = std::env::var("XDG_RUNTIME_DIR").map_err(|_| BackendError::Env("XDG_RUNTIME_DIR missing".into()))?;
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .map_err(|_| BackendError::Env("HYPRLAND_INSTANCE_SIGNATURE missing".into()))?;
    Ok(format!("{runtime}/hypr/{sig}/.socket.sock"))
}

pub(crate) fn request(payload: &[u8]) -> BackendResult<Vec<u8>> {
    let path = socket_path()?;
    let mut s = UnixStream::connect(&path).map_err(|e| BackendError::Socket(format!("connect {path}: {e}")))?;
    s.write_all(payload).map_err(|e| BackendError::Socket(format!("write: {e}")))?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).map_err(|e| BackendError::Socket(format!("read: {e}")))?;
    Ok(buf)
}

/// Raw `monitors all -j` reply parsed into tolerant [`HyprMonitor`]s.
pub fn fetch_monitors() -> BackendResult<Vec<HyprMonitor>> {
    let buf = request(b"j/monitors all")?;
    serde_json::from_slice(&buf).map_err(|e| BackendError::Json(format!("monitors all: {e}")))
}

/// One entry of `hyprctl clients -j` (tolerant subset for session snapshot).
#[derive(Debug, Clone, serde::Deserialize)]
#[allow(non_snake_case)]
pub struct ClientRaw {
    /// Stable window address.
    #[serde(default)]
    pub address: String,
    /// Hosting workspace.
    #[serde(default)]
    pub workspace: WorkspaceId,
    /// Hosting monitor id.
    #[serde(default)]
    pub monitor: i32,
    /// Floating window.
    #[serde(default)]
    pub floating: bool,
    /// Fullscreen mode (0 = none).
    #[serde(default)]
    pub fullscreen: i32,
    /// Pinned window.
    #[serde(default)]
    pub pinned: bool,
    /// Mapped (visible) window.
    #[serde(default)]
    pub mapped: bool,
    /// Focus history id.
    #[serde(default)]
    pub focusHistoryID: i32,
}

/// Workspace pointer inside client/monitor entries.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct WorkspaceId {
    /// Workspace id.
    #[serde(default)]
    pub id: i32,
    /// Workspace name.
    #[serde(default)]
    pub name: String,
}

/// Raw `clients -j` reply (window records for session snapshot/verify).
pub fn fetch_clients() -> BackendResult<Vec<ClientRaw>> {
    let buf = request(b"j/clients")?;
    serde_json::from_slice(&buf).map_err(|e| BackendError::Json(format!("clients: {e}")))
}

/// One entry of `hyprctl workspaces -j` (tolerant subset for snapshot).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct WorkspaceRaw {
    /// Workspace id.
    #[serde(default)]
    pub id: i32,
    /// Workspace name.
    #[serde(default)]
    pub name: String,
    /// Hosting monitor name.
    #[serde(default)]
    pub monitor: String,
}

/// Raw `workspaces -j` reply (workspace layout for snapshot/verify).
pub fn fetch_workspaces() -> BackendResult<Vec<WorkspaceRaw>> {
    let buf = request(b"j/workspaces")?;
    serde_json::from_slice(&buf).map_err(|e| BackendError::Json(format!("workspaces: {e}")))
}

impl DisplayBackend for HyprlandBackend {
    fn backend_name(&self) -> &'static str {
        "Hyprland"
    }

    fn list_outputs(&self) -> BackendResult<Vec<(OutputState, HyprMonitorInfo)>> {
        Ok(fetch_monitors()?.iter().map(normalize).collect())
    }

    fn get_output(&self, name: &str) -> BackendResult<Option<(OutputState, HyprMonitorInfo)>> {
        Ok(fetch_monitors()?.iter().find(|m| m.name == name).map(normalize))
    }

    fn available_modes(&self, name: &str) -> BackendResult<Vec<Mode>> {
        let mons = fetch_monitors()?;
        let Some(m) = mons.iter().find(|m| m.name == name) else { return Ok(Vec::new()) };
        // Unparseable entries are skipped (reported by unify as warnings),
        // never invented.
        Ok(m.available_modes.iter().filter_map(|s| display_core::parse_mode(s).ok()).collect())
    }
}
