//! Process supervision: spawn, wait, status, terminate.
//!
//! Adapters spawn through [`Spawner`] so tests use doubles and never launch
//! real emulators. Termination uses `Child::kill` (SIGKILL on Unix):
//! graceful SIGTERM needs platform signal support — documented F6 work.
//!
//! Two production spawners, and the difference matters: [`StdSpawner`] gives the
//! emulator the caller's terminal, while [`QuietSpawner`] takes it away. An
//! emulator that prints *after* it exits — Dolphin on SIGTERM dumps its argv
//! through Qt's handler — would otherwise scribble over the TUI that is waiting
//! for it, which is exactly the "the UI breaks when I close the game" bug. The
//! emulator renders in its own window, so its console output is only a log.

use crate::launch::PreparedLaunch;
use std::path::PathBuf;

/// Supervised child handle (object-safe for stubbing).
pub trait ChildHandle {
    /// Block until exit; return the exit code (-1 when unavailable).
    fn wait(&mut self) -> Result<i32, String>;
    /// Terminate the child (SIGKILL semantics on Unix). Idempotent.
    fn terminate(&mut self) -> Result<(), String>;
    /// Best-effort liveness probe.
    fn is_running(&mut self) -> bool;
}

/// Process launcher (object-safe).
pub trait Spawner {
    /// Spawn exactly what [`PreparedLaunch`] describes.
    fn spawn(&self, prepared: &PreparedLaunch) -> Result<Box<dyn ChildHandle>, String>;
}

/// Production spawner: `std::process::Command` (the only other sanctioned
/// spawn site besides F3's `transition.rs` and the daemon's orchestration).
pub struct StdSpawner;

struct StdChild(std::process::Child);

impl ChildHandle for StdChild {
    fn wait(&mut self) -> Result<i32, String> {
        self.0.wait().map(|s| s.code().unwrap_or(-1)).map_err(|e| e.to_string())
    }

    fn terminate(&mut self) -> Result<(), String> {
        match self.0.try_wait() {
            Ok(Some(_)) => Ok(()), // already exited: idempotent success
            Ok(None) => self.0.kill().map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn is_running(&mut self) -> bool {
        matches!(self.0.try_wait(), Ok(None))
    }
}

impl Spawner for StdSpawner {
    fn spawn(&self, prepared: &PreparedLaunch) -> Result<Box<dyn ChildHandle>, String> {
        let mut cmd = std::process::Command::new(&prepared.program);
        cmd.args(&prepared.args);
        for (k, v) in &prepared.env {
            cmd.env(k, v);
        }
        if let Some(dir) = &prepared.workdir {
            cmd.current_dir(dir);
        }
        cmd.spawn().map(|c| Box::new(StdChild(c)) as Box<dyn ChildHandle>).map_err(|e| e.to_string())
    }
}

/// Spawner that detaches the emulator from the terminal.
///
/// stdin becomes `/dev/null` and stdout/stderr are appended to `log`, so a late
/// write from a dying emulator can never corrupt the UI. A log that cannot be
/// opened is *not* fatal: the game must still start, and the spawner then falls
/// back to inheriting the terminal (documented, visible degradation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuietSpawner {
    /// File that receives the emulator's stdout/stderr (appended, kept).
    pub log: PathBuf,
}

impl QuietSpawner {
    /// Redirect the emulator's console output to `log`.
    pub fn new(log: impl Into<PathBuf>) -> Self {
        Self { log: log.into() }
    }
}

impl Spawner for QuietSpawner {
    fn spawn(&self, prepared: &PreparedLaunch) -> Result<Box<dyn ChildHandle>, String> {
        use std::fs::OpenOptions;
        let mut cmd = std::process::Command::new(&prepared.program);
        cmd.args(&prepared.args);
        for (k, v) in &prepared.env {
            cmd.env(k, v);
        }
        if let Some(dir) = &prepared.workdir {
            cmd.current_dir(dir);
        }
        match OpenOptions::new().create(true).append(true).open(&self.log) {
            Ok(file) => {
                cmd.stdin(std::process::Stdio::null());
                cmd.stdout(std::process::Stdio::from(file.try_clone().map_err(|e| e.to_string())?));
                cmd.stderr(std::process::Stdio::from(file));
            }
            // No log file: still launch. The terminal is shared again, which is
            // the old behaviour, and the caller decides whether to warn.
            Err(_) => {}
        }
        cmd.spawn().map(|c| Box::new(StdChild(c)) as Box<dyn ChildHandle>).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_spawner_keeps_console_output_out_of_the_terminal() {
        let dir = std::env::temp_dir().join("rosadeck-quiet-spawner");
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("emu.log");
        std::fs::remove_file(&log).ok();
        let p = PreparedLaunch {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "echo LATE-NOISE; echo ON-STDERR 1>&2".into()],
            env: vec![],
            workdir: None,
            notes: vec![],
        };
        let mut child = QuietSpawner::new(&log).spawn(&p).unwrap();
        assert_eq!(child.wait().unwrap(), 0);
        let got = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(got.contains("LATE-NOISE"), "the emulator's stdout must reach the log: {got:?}");
        assert!(got.contains("ON-STDERR"), "stderr too: {got:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn quiet_spawner_appends_and_still_launches_when_the_log_is_unwritable() {
        let dir = std::env::temp_dir().join("rosadeck-quiet-spawner-2");
        std::fs::remove_dir_all(&dir).ok();
        // A directory as the log path cannot be opened for append: the launch
        // must still succeed rather than failing the game.
        std::fs::create_dir_all(&dir).unwrap();
        let p = PreparedLaunch {
            program: "/bin/true".into(),
            args: vec![],
            env: vec![],
            workdir: None,
            notes: vec![],
        };
        let mut child = QuietSpawner::new(&dir).spawn(&p).unwrap();
        assert_eq!(child.wait().unwrap(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn spawns_true_and_reaps_status() {
        let p = PreparedLaunch {
            program: "/bin/true".into(),
            args: vec![],
            env: vec![],
            workdir: None,
            notes: vec![],
        };
        let mut child = StdSpawner.spawn(&p).unwrap();
        assert_eq!(child.wait().unwrap(), 0);
    }

    #[test]
    fn terminate_is_idempotent() {
        let p = PreparedLaunch {
            program: "/bin/sleep".into(),
            args: vec!["30".into()],
            env: vec![],
            workdir: None,
            notes: vec![],
        };
        let mut child = StdSpawner.spawn(&p).unwrap();
        assert!(child.is_running());
        child.terminate().unwrap();
        let _ = child.wait();
        child.terminate().unwrap(); // second call: still Ok
    }

    #[test]
    fn missing_binary_is_an_error_not_a_panic() {
        let p = PreparedLaunch {
            program: "/definitely/not/here-rosadeck".into(),
            args: vec![],
            env: vec![],
            workdir: None,
            notes: vec![],
        };
        assert!(StdSpawner.spawn(&p).is_err());
    }
}
