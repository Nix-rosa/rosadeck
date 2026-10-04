//! Deterministic doubles for tests and dry-run tooling (never real processes).

use crate::launch::PreparedLaunch;
use crate::process::{ChildHandle, Spawner};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Scripted child: programmed exit code, records `terminate` calls.
pub struct StubChild {
    /// Exit code `wait` will report.
    pub exit_code: i32,
    /// How many times `terminate` was called.
    pub terminates: Arc<Mutex<u32>>,
    waited: bool,
}

impl ChildHandle for StubChild {
    fn wait(&mut self) -> Result<i32, String> {
        self.waited = true;
        Ok(self.exit_code)
    }

    fn terminate(&mut self) -> Result<(), String> {
        *self.terminates.lock().unwrap() += 1;
        Ok(())
    }

    fn is_running(&mut self) -> bool {
        !self.waited
    }
}

/// Scripted spawner: yields queued exit codes, records every `PreparedLaunch`.
#[derive(Debug, Default)]
pub struct StubSpawner {
    /// Received launches (inspectable).
    pub seen: Arc<Mutex<Vec<PreparedLaunch>>>,
    /// Exit codes to hand out, in order (default 0).
    pub codes: Mutex<VecDeque<i32>>,
    /// Fail `spawn` when true.
    pub fail_spawn: bool,
}

impl StubSpawner {
    /// New stub yielding `codes` in order.
    pub fn with_codes(codes: Vec<i32>) -> Self {
        Self { seen: Arc::new(Mutex::new(vec![])), codes: Mutex::new(codes.into()), fail_spawn: false }
    }
}

impl Spawner for StubSpawner {
    fn spawn(&self, prepared: &PreparedLaunch) -> Result<Box<dyn ChildHandle>, String> {
        if self.fail_spawn {
            return Err("stub spawn refused".into());
        }
        self.seen.lock().unwrap().push(prepared.clone());
        let code = self.codes.lock().unwrap().pop_front().unwrap_or(0);
        Ok(Box::new(StubChild { exit_code: code, terminates: Arc::new(Mutex::new(0)), waited: false }))
    }
}
