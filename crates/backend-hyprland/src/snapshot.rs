//! Real snapshots: capture, persist (tmp+fsync+rename) and reload.
//!
//! A snapshot holds everything needed to restore outputs: per-output state
//! (mode, position, scale, disabled, mirror, transform, vrr, focus, active
//! workspace), the workspace layout, and the raw IPC replies for forensics.
//! NOT captured (limitation, documented): `bitdepth`/color-management are not
//! exposed by `monitors -j`, so restore cannot reinstate them.

use crate::backend::{fetch_monitors, fetch_workspaces, BackendResult};
use crate::model::normalize;
use display_core::{DisplaySnapshot, OutputState};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Workspace placement reference.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceRef {
    /// Workspace id.
    pub id: i32,
    /// Workspace name.
    pub name: String,
    /// Hosting monitor.
    pub monitor: String,
}

/// Per-output snapshot entry: restorable state + observed extras.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotOutput {
    /// Core restorable state (mode/position/scale/disabled/mirror).
    pub state: OutputState,
    /// Rotation transform (restored when non-zero).
    pub transform: i32,
    /// VRR flag (recorded only: no keyword path in F3).
    pub vrr: bool,
    /// Focused at capture.
    pub focused: bool,
    /// Active workspace at capture.
    pub active_workspace: Option<WorkspaceRef>,
}

/// Restorable point-in-time capture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// Unix seconds at capture.
    pub taken_at: u64,
    /// One entry per monitor known to the compositor.
    pub outputs: Vec<SnapshotOutput>,
    /// Workspace layout at capture.
    pub workspaces: Vec<WorkspaceRef>,
    /// Focused output name at capture, when any.
    pub focused_output: Option<String>,
    /// Raw `monitors all -j` reply (forensics / forward-compat).
    pub raw_monitors: serde_json::Value,
    /// Raw `workspaces -j` reply.
    pub raw_workspaces: serde_json::Value,
}

impl Snapshot {
    /// Core view for the planner (`DisplaySnapshot`).
    pub fn to_core(&self) -> DisplaySnapshot {
        DisplaySnapshot { taken_at_unix: self.taken_at, outputs: self.outputs.iter().map(|o| o.state.clone()).collect() }
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Capture a snapshot from live IPC (read-only).
pub fn take_snapshot() -> BackendResult<Snapshot> {
    let raw_m = request_monitors_raw()?;
    let raw_w = request_workspaces_raw()?;
    let mons = fetch_monitors()?;
    let workspaces = fetch_workspaces()?;
    let ws_refs: Vec<WorkspaceRef> =
        workspaces.iter().map(|w| WorkspaceRef { id: w.id, name: w.name.clone(), monitor: w.monitor.clone() }).collect();
    let mut outputs = Vec::new();
    let mut focused_output = None;
    for m in &mons {
        let (state, _) = normalize(m);
        if m.focused {
            focused_output = Some(m.name.clone());
        }
        let active_workspace =
            ws_refs.iter().find(|w| w.monitor == m.name && (w.id == m.activeWorkspace.id || m.activeWorkspace.id == 0)).cloned();
        outputs.push(SnapshotOutput {
            state,
            transform: m.transform,
            vrr: m.vrr,
            focused: m.focused,
            active_workspace,
        });
    }
    Ok(Snapshot { taken_at: now_unix(), outputs, workspaces: ws_refs, focused_output, raw_monitors: raw_m, raw_workspaces: raw_w })
}

fn request_monitors_raw() -> BackendResult<serde_json::Value> {
    let buf = super::backend::request(b"j/monitors all")?;
    serde_json::from_slice(&buf).map_err(|e| crate::backend::BackendError::Json(format!("monitors raw: {e}")))
}

fn request_workspaces_raw() -> BackendResult<serde_json::Value> {
    let buf = super::backend::request(b"j/workspaces")?;
    serde_json::from_slice(&buf).map_err(|e| crate::backend::BackendError::Json(format!("workspaces raw: {e}")))
}

/// State directory: `$XDG_STATE_HOME/rosadeck` else `~/.local/state/rosadeck`.
pub fn state_dir() -> PathBuf {
    let base = std::env::var("XDG_STATE_HOME")
        .ok()
        .filter(|x| !x.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())).join(".local/state"));
    // El nombre viejo sigue valiendo hasta que el usuario mueva sus datos.
    if base.join("rosadeck").exists() || !base.join("hyprgame").exists() {
        base.join("rosadeck")
    } else {
        base.join("hyprgame")
    }
}

/// Persist atomically: tmp file + fsync + rename; refresh the `last` symlink.
pub fn save_snapshot(dir: &std::path::Path, snap: &Snapshot) -> BackendResult<PathBuf> {
    fs::create_dir_all(dir).map_err(|e| crate::backend::BackendError::Env(format!("state dir: {e}")))?;
    let name = format!("snapshot-{}.json", snap.taken_at);
    let dest = dir.join(&name);
    let tmp = dir.join(format!(".{name}.tmp"));
    let text = serde_json::to_string_pretty(snap).map_err(|e| crate::backend::BackendError::Json(e.to_string()))?;
    {
        use std::io::Write;
        let mut f = fs::File::create(&tmp).map_err(|e| crate::backend::BackendError::Env(format!("tmp: {e}")))?;
        f.write_all(text.as_bytes()).map_err(|e| crate::backend::BackendError::Env(format!("write: {e}")))?;
        f.sync_all().map_err(|e| crate::backend::BackendError::Env(format!("fsync: {e}")))?;
    }
    fs::rename(&tmp, &dest).map_err(|e| crate::backend::BackendError::Env(format!("rename: {e}")))?;
    let last = dir.join("last");
    let _ = fs::remove_file(&last);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&name, &last)
        .map_err(|e| crate::backend::BackendError::Env(format!("symlink last: {e}")))?;
    Ok(dest)
}

/// Load the snapshot pointed to by `last` (or newest `snapshot-*.json`).
pub fn load_last_snapshot(dir: &std::path::Path) -> BackendResult<Snapshot> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let last = dir.join("last");
    if last.exists() {
        candidates.push(fs::read_link(&last).map(|t| dir.join(t)).unwrap_or(last));
    }
    if candidates.is_empty() {
        let mut files: Vec<PathBuf> = match fs::read_dir(dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("snapshot-")))
                .collect(),
            Err(_) => return Err(crate::backend::BackendError::Env("no snapshots found".into())),
        };
        files.sort();
        candidates.extend(files.pop());
    }
    let path = candidates.into_iter().next().ok_or_else(|| crate::backend::BackendError::Env("no snapshots found".into()))?;
    let text = fs::read_to_string(&path).map_err(|e| crate::backend::BackendError::Env(format!("read: {e}")))?;
    serde_json::from_str(&text).map_err(|e| crate::backend::BackendError::Json(format!("snapshot parse: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use display_core::{Mirror, OutputId, Position, Scale};

    fn fake_snapshot() -> Snapshot {
        Snapshot {
            taken_at: 1_700_000_000,
            outputs: vec![SnapshotOutput {
                state: OutputState {
                    id: OutputId("eDP-1".into()),
                    connector: None,
                    mode: Some(display_core::Mode { width: 1920, height: 1080, hz: 59.997 }),
                    position: Position { x: 0, y: 0 },
                    scale: Scale(1.5),
                    disabled: false,
                    mirror: Mirror::Disabled,
                },
                transform: 0,
                vrr: false,
                focused: true,
                active_workspace: Some(WorkspaceRef { id: 1, name: "1".into(), monitor: "eDP-1".into() }),
            }],
            workspaces: vec![WorkspaceRef { id: 1, name: "1".into(), monitor: "eDP-1".into() }],
            focused_output: Some("eDP-1".into()),
            raw_monitors: serde_json::json!([]),
            raw_workspaces: serde_json::json!([]),
        }
    }

    #[test]
    fn save_load_roundtrip_in_tempdir() {
        let dir = std::env::temp_dir().join("rosadeck-snap-test");
        std::fs::remove_dir_all(&dir).ok();
        let snap = fake_snapshot();
        let dest = save_snapshot(&dir, &snap).unwrap();
        assert!(dest.exists());
        assert!(dir.join("last").exists());
        let back = load_last_snapshot(&dir).unwrap();
        assert_eq!(back.taken_at, snap.taken_at);
        assert_eq!(back.outputs.len(), 1);
        assert!((back.outputs[0].state.mode.unwrap().hz - 59.997).abs() < 1e-9);
        assert_eq!(back.to_core().outputs.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_missing_is_error() {
        let dir = std::env::temp_dir().join("rosadeck-snap-empty");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        assert!(load_last_snapshot(&dir).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
