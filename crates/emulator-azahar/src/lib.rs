//! Azahar adapter: verified flags only (`-f`, positional path).
//!
//! Observed live (`azahar --help`): `azahar [options] <file path>` with
//! `-f/--fullscreen` and `-w/--windowed`. Note `-c` means *compress*, not
//! config — never pass it a config path.

use rosadeck_emulator_core::{Emulator, EmulatorError, LaunchContext, PreparedLaunch};

/// Azahar adapter configuration.
#[derive(Debug, Clone)]
pub struct AzaharConfig {
    /// Binary name for PATH lookup (`azahar`).
    pub binary: String,
    /// Explicit absolute binary (wins over PATH).
    pub binary_path: Option<String>,
    /// Start fullscreen (`-f`; default true — library launches fullscreen).
    pub fullscreen: bool,
}

/// Azahar adapter (Nintendo 3DS).
#[derive(Debug, Clone)]
pub struct Azahar {
    /// Adapter configuration.
    pub config: AzaharConfig,
}

impl Azahar {
    /// New adapter from explicit configuration.
    pub fn new(config: AzaharConfig) -> Self {
        Self { config }
    }
}

impl Emulator for Azahar {
    fn id(&self) -> &'static str {
        "azahar"
    }

    fn prepare(&self, ctx: &LaunchContext) -> Result<PreparedLaunch, EmulatorError> {
        ctx.game.validate().map_err(EmulatorError::InvalidProfile)?;
        if ctx.shader_path.is_some() {
            return Err(EmulatorError::Unsupported("Azahar does not take shader presets".into()));
        }
        // Missing emulator = hard error with an honest message, not a note
        // followed by a failing exec.
        let program = self
            .resolve_binary(&self.config.binary, self.config.binary_path.as_deref())?
            .to_string_lossy()
            .into_owned();
        let mut args = Vec::new();
        let mut notes = Vec::new();
        args.push(if self.config.fullscreen { "-f".to_owned() } else { "-w".to_owned() });
        args.push(ctx.game.path.clone());
        args.extend(ctx.extra_args.clone());
        if let Some(label) = &ctx.display_mode_label {
            notes.push(format!("display prepared by subsystem: {label}"));
        }
        Ok(PreparedLaunch { program, args, env: ctx.extra_env.clone(), workdir: ctx.workdir.clone(), notes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rosadeck_emulator_core::GameTarget;

    #[test]
    fn fullscreen_positional_args() {
        let a = Azahar::new(AzaharConfig { binary: "azahar".into(), binary_path: None, fullscreen: true });
        let ctx = LaunchContext {
            game: GameTarget { platform: "3ds".into(), path: "/games/mk7.cci".into(), title: None },
            emulator_id: "azahar".into(),
            display_mode_label: None,
            display_mode_requested: None,
            shader_path: None,
            extra_env: vec![],
            extra_args: vec![],
            workdir: None,
        };
        let p = a.prepare(&ctx).unwrap();
        assert!(p.program.ends_with("azahar")); // installed: resolved or fallback name
        assert_eq!(&p.args[0..2], ["-f", "/games/mk7.cci"]);
    }
}
