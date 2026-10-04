//! RetroArch adapter: argument generation only.
//!
//! Documented RetroArch CLI surface used here (`--config`, `--appendconfig`,
//! `-L`, `--set-shader`, positional content). VSync is a `video_vsync`
//! *config key*, not a flag: the adapter records the requirement in
//! `PreparedLaunch.notes` and never writes configs in F5 (limitation).

use rosadeck_emulator_core::{Emulator, EmulatorError, LaunchContext, PreparedLaunch};

/// RetroArch adapter configuration (projected from the emulator profile).
#[derive(Debug, Clone)]
pub struct RetroArchConfig {
    /// Binary name for PATH lookup (`retroarch`).
    pub binary: String,
    /// Explicit absolute binary (wins over PATH).
    pub binary_path: Option<String>,
    /// `--config` base config (referenced, never written).
    pub config_path: Option<String>,
    /// `--appendconfig` overlay config (referenced, never written).
    pub append_config: Option<String>,
    /// `-L` libretro core (optional; content-first invocation otherwise).
    pub core_path: Option<String>,
    /// Start fullscreen via `-f` (documented upstream flag; default true —
    /// library launches fullscreen; not live-validated, RetroArch absent).
    pub fullscreen: bool,
    /// Desired VSync (recorded as config requirement, see module docs).
    pub vsync: Option<bool>,
}

/// RetroArch adapter.
#[derive(Debug, Clone)]
pub struct RetroArch {
    /// Adapter configuration.
    pub config: RetroArchConfig,
}

impl RetroArch {
    /// New adapter from explicit configuration.
    pub fn new(config: RetroArchConfig) -> Self {
        Self { config }
    }
}

impl Emulator for RetroArch {
    fn id(&self) -> &'static str {
        "retroarch"
    }

    fn prepare(&self, ctx: &LaunchContext) -> Result<PreparedLaunch, EmulatorError> {
        ctx.game.validate().map_err(EmulatorError::InvalidProfile)?;
        // A missing emulator is a hard, named error. The old fallback (use the
        // name and add a note) made `--dry-run` look valid for a program that
        // does not exist, and the failure only surfaced as a spawn error.
        let program = self
            .resolve_binary(&self.config.binary, self.config.binary_path.as_deref())?
            .to_string_lossy()
            .into_owned();
        let mut args = Vec::new();
        let mut notes = Vec::new();
        if self.config.fullscreen {
            args.push("-f".to_owned());
        }
        if let Some(cfg) = &self.config.config_path {
            args.extend(["--config".into(), cfg.clone()]);
        }
        if let Some(cfg) = &self.config.append_config {
            args.extend(["--appendconfig".into(), cfg.clone()]);
        }
        if let Some(core) = &self.config.core_path {
            args.extend(["-L".into(), core.clone()]);
        }
        if let Some(shader) = &ctx.shader_path {
            if !shader.ends_with(".slangp") && !shader.ends_with(".glslp") {
                return Err(EmulatorError::Unsupported(format!("RetroArch shader must be a preset: {shader}")));
            }
            args.extend(["--set-shader".into(), shader.clone()]);
        }
        args.push(ctx.game.path.clone());
        args.extend(ctx.extra_args.clone());
        if let Some(vsync) = self.config.vsync {
            notes.push(format!(
                "requires video_vsync={} in RetroArch config (no config writes in F5)",
                if vsync { "true" } else { "false" }
            ));
        }
        if let Some(label) = &ctx.display_mode_label {
            notes.push(format!("display prepared by subsystem: {label}"));
        }
        let mut env = ctx.extra_env.clone();
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_some() {
            env.push(("SDL_VIDEODRIVER".into(), "wayland".into()));
        }
        Ok(PreparedLaunch { program, args, env, workdir: ctx.workdir.clone(), notes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rosadeck_emulator_core::GameTarget;

    fn ctx() -> LaunchContext {
        LaunchContext {
            game: GameTarget { platform: "snes".into(), path: "/games/chrono.sfc".into(), title: None },
            emulator_id: "retroarch".into(),
            display_mode_label: Some("external-only".into()),
            display_mode_requested: Some("1920x1080@120".into()),
            shader_path: None,
            extra_env: vec![],
            extra_args: vec![],
            workdir: None,
        }
    }

    /// Argument construction is testable with any installed binary: the adapter
    /// only builds a description, it never runs it. `true` is always present.
    fn adapter() -> RetroArch {
        RetroArch::new(RetroArchConfig {
            binary: "true".into(),
            binary_path: None,
            config_path: Some("/home/u/.config/retroarch/retroarch.cfg".into()),
            append_config: None,
            core_path: Some("/cores/snes9x_libretro.so".into()),
            fullscreen: true,
            vsync: Some(true),
        })
    }

    #[test]
    fn generates_args_without_spawning() {
        let p = adapter().prepare(&ctx()).unwrap();
        assert_eq!(&p.args[0..6], ["-f", "--config", "/home/u/.config/retroarch/retroarch.cfg", "-L", "/cores/snes9x_libretro.so", "/games/chrono.sfc"]);
        assert!(p.notes.iter().any(|n| n.contains("video_vsync=true")));
    }

    #[test]
    fn shader_is_passed_and_missing_binary_is_an_error() {
        let mut c = ctx();
        c.shader_path = Some("/shaders/crt.slangp".into());
        let p = adapter().prepare(&c).unwrap();
        assert!(p.args.windows(2).any(|w| w == ["--set-shader", "/shaders/crt.slangp"]));
        // 'retroarch' is not installed here: that must fail loudly, with the
        // name, instead of a note followed by a failing exec.
        let missing = RetroArch::new(RetroArchConfig {
            binary: "retroarch-not-installed".into(),
            binary_path: None,
            config_path: None,
            append_config: None,
            core_path: None,
            fullscreen: true,
            vsync: None,
        });
        assert_eq!(
            missing.prepare(&ctx()),
            Err(rosadeck_emulator_core::EmulatorError::BinaryNotFound("retroarch-not-installed".into())),
            "honest error naming the binary"
        );
        let mut bad = ctx();
        bad.shader_path = Some("/shaders/x.cg".into());
        assert!(adapter().prepare(&bad).is_err());
    }
}
