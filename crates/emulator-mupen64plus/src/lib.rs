//! Mupen64Plus adapter (Nintendo 64), flags verified on this machine.
//!
//! Observed 2026-10-02 with `mupen64plus --help` (2.6.0, Arch `extra`):
//! the console UI takes `--fullscreen`, `--nospeedlimit`, `--resolution`,
//! `--gfx`, `--audio`, `--rsp`, `--configdir` and a **positional ROM file**.
//! This is the full windowed UI, not the bare core, so it launches a game.
//!
//! No slangp shaders on this emulator either.

use rosadeck_emulator_core::{Emulator, EmulatorError, LaunchContext, PreparedLaunch};

/// Adapter configuration.
#[derive(Debug, Clone)]
pub struct Mupen64PlusConfig {
    /// Binary name for PATH lookup (`mupen64plus`).
    pub binary: String,
    /// Explicit absolute binary (wins over PATH).
    pub binary_path: Option<String>,
    /// Start fullscreen (`--fullscreen`; default true).
    pub fullscreen: bool,
    /// Disable the speed limiter (`--nospeedlimit`).
    pub nospeedlimit: bool,
    /// Explicit render size, e.g. `1024x768` (verified flag).
    pub resolution: Option<String>,
    /// Graphics plugin to use (verified flag), e.g. `gfxdummy`.
    pub gfx_plugin: Option<String>,
}

/// Mupen64Plus adapter.
#[derive(Debug, Clone)]
pub struct Mupen64Plus {
    /// Adapter configuration.
    pub config: Mupen64PlusConfig,
}

impl Mupen64Plus {
    /// New adapter from explicit configuration.
    pub fn new(config: Mupen64PlusConfig) -> Self {
        Self { config }
    }
}

impl Emulator for Mupen64Plus {
    fn id(&self) -> &'static str {
        "mupen64plus"
    }

    fn prepare(&self, ctx: &LaunchContext) -> Result<PreparedLaunch, EmulatorError> {
        ctx.game.validate().map_err(EmulatorError::InvalidProfile)?;
        if ctx.shader_path.is_some() {
            return Err(EmulatorError::Unsupported("Mupen64Plus does not take slangp shader presets".into()));
        }
        let program = self
            .resolve_binary(&self.config.binary, self.config.binary_path.as_deref())?
            .to_string_lossy()
            .into_owned();
        let mut args = Vec::new();
        if self.config.fullscreen {
            args.push("--fullscreen".to_owned());
        } else {
            args.push("--windowed".to_owned());
        }
        if self.config.nospeedlimit {
            args.push("--nospeedlimit".to_owned());
        }
        if let Some(res) = &self.config.resolution {
            args.push("--resolution".to_owned());
            args.push(res.clone());
        }
        if let Some(gfx) = &self.config.gfx_plugin {
            args.push("--gfx".to_owned());
            args.push(gfx.clone());
        }
        args.push(ctx.game.path.clone());
        args.extend(ctx.extra_args.clone());
        let mut notes = Vec::new();
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

    fn ctx() -> LaunchContext {
        LaunchContext {
            game: GameTarget { platform: "n64".into(), path: "/games/mario party.z64".into(), title: None },
            emulator_id: "mupen64plus".into(),
            display_mode_label: None,
            display_mode_requested: None,
            shader_path: None,
            extra_env: vec![],
            extra_args: vec![],
            workdir: None,
        }
    }

    fn adapter() -> Mupen64Plus {
        Mupen64Plus::new(Mupen64PlusConfig {
            binary: "mupen64plus".into(),
            binary_path: None,
            fullscreen: true,
            nospeedlimit: false,
            resolution: None,
            gfx_plugin: None,
        })
    }

    #[test]
    fn fullscreen_positional_args() {
        let p = adapter().prepare(&ctx()).unwrap();
        assert!(p.program.ends_with("mupen64plus"), "installed here: {p:?}");
        assert_eq!(&p.args[0..2], ["--fullscreen", "/games/mario party.z64"], "the ROM keeps its spaces");
    }

    #[test]
    fn optional_flags_are_passed_through() {
        let a = Mupen64Plus::new(Mupen64PlusConfig {
            binary: "mupen64plus".into(),
            binary_path: None,
            fullscreen: false,
            nospeedlimit: true,
            resolution: Some("1024x768".into()),
            gfx_plugin: Some("gfxdummy".into()),
        });
        let p = a.prepare(&ctx()).unwrap();
        assert_eq!(p.args[0], "--windowed");
        assert_eq!(p.args[1], "--nospeedlimit");
        assert!(p.args.windows(2).any(|w| w == ["--resolution", "1024x768"]));
        assert!(p.args.windows(2).any(|w| w == ["--gfx", "gfxdummy"]));
        assert_eq!(p.args.last().unwrap(), "/games/mario party.z64");
    }

    #[test]
    fn missing_binary_is_an_hard_error() {
        let a = Mupen64Plus::new(Mupen64PlusConfig {
            binary: "mupen64plus-not-installed".into(),
            binary_path: None,
            fullscreen: true,
            nospeedlimit: false,
            resolution: None,
            gfx_plugin: None,
        });
        assert_eq!(a.prepare(&ctx()), Err(EmulatorError::BinaryNotFound("mupen64plus-not-installed".into())));
    }

    #[test]
    fn shaders_are_refused() {
        let mut c = ctx();
        c.shader_path = Some("/shaders/crt.slangp".into());
        assert!(matches!(adapter().prepare(&c), Err(EmulatorError::Unsupported(_))));
    }
}