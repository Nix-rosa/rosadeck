//! Dolphin adapter: argument generation only.
//!
//! Documented `dolphin-emu` CLI surface used here (`-e/--exec`, `-b/--batch`,
//! `-u/--user`). VSync lives in Dolphin's INI (`[Core] VSync` / graphics
//! settings), not on the CLI: recorded in notes, never written (limitation,
//! same as RetroArch). slangp shaders are rejected as incompatible.

use rosadeck_emulator_core::{Emulator, EmulatorError, LaunchContext, PreparedLaunch};

/// Dolphin adapter configuration (projected from the emulator profile).
#[derive(Debug, Clone)]
pub struct DolphinConfig {
    /// Binary name for PATH lookup (`dolphin-emu`).
    pub binary: String,
    /// Explicit absolute binary (wins over PATH).
    pub binary_path: Option<String>,
    /// `-u/--user` dir (referenced, never written).
    pub user_dir: Option<String>,
    /// `-b/--batch`: exit Dolphin when emulation stops.
    pub batch: bool,
    /// Start fullscreen via `-C Dolphin.Display.Fullscreen=True`
    /// (documented upstream; default true — library launches fullscreen).
    pub fullscreen: bool,
    /// Desired VSync via `-C GFX.Settings.VSync=True/False`
    /// (key observed in real `GFX.ini` `[Settings]`).
    pub vsync: Option<bool>,
}

/// Dolphin adapter.
#[derive(Debug, Clone)]
pub struct Dolphin {
    /// Adapter configuration.
    pub config: DolphinConfig,
}

impl Dolphin {
    /// New adapter from explicit configuration.
    pub fn new(config: DolphinConfig) -> Self {
        Self { config }
    }
}

impl Emulator for Dolphin {
    fn id(&self) -> &'static str {
        "dolphin"
    }

    fn prepare(&self, ctx: &LaunchContext) -> Result<PreparedLaunch, EmulatorError> {
        ctx.game.validate().map_err(EmulatorError::InvalidProfile)?;
        if ctx.shader_path.is_some() {
            return Err(EmulatorError::Unsupported("Dolphin does not take slangp shader presets".into()));
        }
        // Missing emulator = hard error with an honest message, not a note
        // followed by a failing exec.
        let program = self
            .resolve_binary(&self.config.binary, self.config.binary_path.as_deref())?
            .to_string_lossy()
            .into_owned();
        let mut args = Vec::new();
        let mut notes = Vec::new();
        if self.config.batch {
            args.push("-b".to_owned());
        }
        if let Some(user) = &self.config.user_dir {
            args.extend(["-u".into(), user.clone()]);
        }
        if self.config.fullscreen {
            args.extend(["-C".into(), "Dolphin.Display.Fullscreen=True".into()]);
        }
        if let Some(vsync) = self.config.vsync {
            args.extend(["-C".into(), format!("GFX.Settings.VSync={}", if vsync { "True" } else { "False" })]);
            notes.push(format!(
                "VSync={} applied via -C (verified key from real GFX.ini)",
                if vsync { "ON" } else { "OFF" }
            ));
        }
        args.extend(["-e".into(), ctx.game.path.clone()]);
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

    fn ctx() -> LaunchContext {
        LaunchContext {
            game: GameTarget { platform: "wii".into(), path: "/games/mk.wbfs".into(), title: Some("Mario Kart Wii".into()) },
            emulator_id: "dolphin".into(),
            display_mode_label: Some("external-only".into()),
            display_mode_requested: None,
            shader_path: None,
            extra_env: vec![],
            extra_args: vec![],
            workdir: None,
        }
    }

    #[test]
    fn generates_batch_exec_args() {
        let d = Dolphin::new(DolphinConfig {
            binary: "dolphin-emu".into(),
            binary_path: None,
            user_dir: None,
            batch: true,
            fullscreen: true,
            vsync: Some(true),
        });
        let p = d.prepare(&ctx()).unwrap();
        assert!(p.program.ends_with("dolphin-emu")); // installed here: resolved
        assert_eq!(&p.args[0..3], ["-b", "-C", "Dolphin.Display.Fullscreen=True"]);
        assert!(p.args.windows(2).any(|w| w == ["-C", "GFX.Settings.VSync=True"]));
        assert_eq!(&p.args[p.args.len() - 2..], ["-e", "/games/mk.wbfs"]);
    }

    #[test]
    fn rejects_shaders() {
        let d = Dolphin::new(DolphinConfig { binary: "dolphin-emu".into(), binary_path: None, user_dir: None, batch: true, fullscreen: false, vsync: None });
        let mut c = ctx();
        c.shader_path = Some("/shaders/crt.slangp".into());
        assert!(d.prepare(&c).is_err());
    }
}
