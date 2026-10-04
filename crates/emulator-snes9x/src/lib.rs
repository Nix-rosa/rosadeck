//! Snes9x GTK adapter (SNES).
//!
//! How fullscreen was actually verified on this machine (2026-10-02,
//! Snes9x 1.4 GTK frontend, `snes9x-gtk`):
//!
//! * `--fullscreen` is **accepted and ignored**. The window came up 634x676 with
//!   `fullscreen: 0` in `hyprctl clients -j` — identical to the run without the
//!   flag. (An earlier note in this crate claimed the flag was verified; it was
//!   only verified to be *parsed*, which is not the same thing.)
//! * Snes9x stores fullscreen as a config item: `~/.config/snes9x/snes9x.conf`
//!   has `[Window State] Fullscreen` and `[Display] FullscreenOnOpen`.
//! * `-conf <file>` is parsed and then ignored: the file is never opened.
//! * What *does* work, and touches nothing of the user's: point the child at an
//!   `XDG_CONFIG_HOME` we own, seeded with a copy of the user's own config
//!   patched on those two keys. Verified: window 1280x720 at (0,0),
//!   `fullscreen: 2`.
//!
//! Copying the user's file (instead of writing a minimal one) is deliberate: it
//! keeps their joystick mappings, filters and sound settings, and we only
//! override the two fullscreen keys. The user's `snes9x.conf` is read, never
//! written.
//!
//! Snes9x has no slangp shaders, so a preset is refused rather than ignored.

use rosadeck_emulator_core::{Emulator, EmulatorError, LaunchContext, PreparedLaunch};
use std::path::{Path, PathBuf};

/// Adapter configuration.
#[derive(Debug, Clone)]
pub struct Snes9xConfig {
    /// Binary name for PATH lookup (`snes9x-gtk`).
    pub binary: String,
    /// Explicit absolute binary (wins over PATH).
    pub binary_path: Option<String>,
    /// Start fullscreen (`FullscreenOnOpen`/`Fullscreen` in the config;
    /// default true — library launches are fullscreen).
    pub fullscreen: bool,
    /// Mute audio at startup (`--mute`), when wanted.
    pub mute: bool,
    /// Directory where Rosadeck materialises the patched config
    /// (`<dir>/snes9x/snes9x.conf`) and points `XDG_CONFIG_HOME` at.
    pub config_home: PathBuf,
}

/// Snes9x adapter.
#[derive(Debug, Clone)]
pub struct Snes9x {
    /// Adapter configuration.
    pub config: Snes9xConfig,
}

impl Snes9x {
    /// New adapter from explicit configuration.
    pub fn new(config: Snes9xConfig) -> Self {
        Self { config }
    }

    /// Path of the config this adapter writes for the child to read.
    pub fn overlay_config(&self) -> PathBuf {
        self.config.config_home.join("snes9x").join("snes9x.conf")
    }

    /// Read the user's config (if any) and force the fullscreen keys on.
    ///
    /// Pure: returns the text to write, so the decision is testable without
    /// touching the filesystem.
    pub fn overlay_text(&self, user_config: Option<&str>) -> String {
        let want = if self.config.fullscreen { "true" } else { "false" };
        let mut out = user_config.unwrap_or("").replace('\r', "");
        // Both keys drive the same behaviour in different code paths
        // ("Fullscreen" = last known window state, "FullscreenOnOpen" =
        // the preference checkbox), so both are set.
        for (group, key) in [("Window State", "Fullscreen"), ("Display", "FullscreenOnOpen")] {
            out = set_key(&out, group, key, want);
        }
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out
    }

    /// Where the user's own config lives, honouring `XDG_CONFIG_HOME`.
    fn user_config_path() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(dir).join("snes9x").join("snes9x.conf"));
        }
        let home = std::env::var_os("HOME").filter(|v| !v.is_empty())?;
        Some(Path::new(&home).join(".config").join("snes9x").join("snes9x.conf"))
    }
}

/// Set `key = value` in `[group]`, creating the group if needed (GKeyFile-ish).
///
/// The user's inline `#` comments are preserved, so the overlay they read on
/// disk stays self-documenting instead of becoming a bare list of keys.
fn set_key(text: &str, group: &str, key: &str, value: &str) -> String {
    let header = format!("[{group}]");
    let mut out: Vec<String> = Vec::with_capacity(text.len() / 24 + 8);
    let mut in_group = false;
    let mut group_seen = false;
    let mut key_seen = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_group && !key_seen {
                out.push(format!("{key} = {value}")); // last resort: end of the group
            }
            in_group = trimmed == header;
            group_seen |= in_group;
            out.push(line.to_owned());
            continue;
        }
        if in_group {
            let name = trimmed.split(['=', ';']).next().unwrap_or("").trim();
            if name == key {
                key_seen = true;
                // Reuse the user's own spacing and comment: the overlay should
                // look like their file with one value changed, not like ours.
                let prefix = trimmed.find('=').map_or(key, |i| &trimmed[..i]);
                let comment = trimmed.find('#').map(|i| trimmed[i..].trim_end());
                out.push(match comment {
                    Some(c) => format!("{prefix}= {value}   {c}"),
                    None => format!("{prefix}= {value}"),
                });
                continue;
            }
        }
        out.push(line.to_owned());
    }
    if in_group && !key_seen {
        out.push(format!("{key} = {value}"));
    }
    if !group_seen {
        if !out.is_empty() && !out.last().is_some_and(|l| l.trim().is_empty()) {
            out.push(String::new());
        }
        out.push(header);
        out.push(format!("{key} = {value}"));
    }
    out.join("\n")
}

impl Emulator for Snes9x {
    fn id(&self) -> &'static str {
        "snes9x"
    }

    /// Materialise the patched config. Runs only right before the spawn, never
    /// in `--dry-run`, and only inside Rosadeck's own state directory.
    fn finalize(&self, prepared: &mut PreparedLaunch) -> Result<(), EmulatorError> {
        let target = self.overlay_config();
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                EmulatorError::PrepareFailed(format!("cannot create {}: {e}", parent.display()))
            })?;
        }
        let user = Self::user_config_path().and_then(|p| std::fs::read_to_string(p).ok());
        let text = self.overlay_text(user.as_deref());
        // Idempotent: rewriting identical bytes would churn the file on every
        // launch and make Snes9x's own mtime checks noisy.
        let same = std::fs::read_to_string(&target).is_ok_and(|cur| cur == text);
        if !same {
            std::fs::write(&target, &text)
                .map_err(|e| EmulatorError::PrepareFailed(format!("cannot write {}: {e}", target.display())))?;
        }
        prepared.env.push(("XDG_CONFIG_HOME".to_owned(), self.config.config_home.display().to_string()));
        Ok(())
    }

    fn prepare(&self, ctx: &LaunchContext) -> Result<PreparedLaunch, EmulatorError> {
        ctx.game.validate().map_err(EmulatorError::InvalidProfile)?;
        if ctx.shader_path.is_some() {
            return Err(EmulatorError::Unsupported("Snes9x does not take slangp shader presets".into()));
        }
        // A missing emulator is a hard error with an honest message: no
        // "note:" and a failing exec later on.
        let program = self
            .resolve_binary(&self.config.binary, self.config.binary_path.as_deref())?
            .to_string_lossy()
            .into_owned();
        let mut args = Vec::new();
        if self.config.mute {
            args.push("--mute".to_owned());
        }
        args.push(ctx.game.path.clone());
        args.extend(ctx.extra_args.clone());
        let mut notes = Vec::new();
        notes.push("fullscreen via XDG_CONFIG_HOME overlay (snes9x ignores --fullscreen)".to_owned());
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
            game: GameTarget { platform: "snes".into(), path: "/games/zelda.sfc".into(), title: None },
            emulator_id: "snes9x".into(),
            display_mode_label: None,
            display_mode_requested: None,
            shader_path: None,
            extra_env: vec![],
            extra_args: vec![],
            workdir: None,
        }
    }

    fn adapter(dir: &Path) -> Snes9x {
        Snes9x::new(Snes9xConfig {
            binary: "snes9x-gtk".into(),
            binary_path: None,
            fullscreen: true,
            mute: false,
            config_home: dir.to_path_buf(),
        })
    }

    #[test]
    fn no_fullscreen_flag_is_passed_because_the_emulator_ignores_it() {
        // Verified: `--fullscreen` parses but leaves the window 634x676.
        let dir = std::env::temp_dir().join("rosadeck-snes9x-no-flag");
        let p = adapter(&dir).prepare(&ctx()).unwrap();
        assert!(p.program.ends_with("snes9x-gtk"), "installed here: {p:?}");
        assert_eq!(p.args, vec!["/games/zelda.sfc"], "no flag that does nothing");
        assert!(p.notes.iter().any(|n| n.contains("ignores --fullscreen")), "{p:?}");
    }

    #[test]
    fn mute_is_optional() {
        let dir = std::env::temp_dir().join("rosadeck-snes9x-mute");
        let a = Snes9x::new(Snes9xConfig { binary: "snes9x-gtk".into(), binary_path: None, fullscreen: true, mute: true, config_home: dir });
        let p = a.prepare(&ctx()).unwrap();
        assert_eq!(p.args[0], "--mute");
        assert_eq!(p.args[1], "/games/zelda.sfc");
    }

    #[test]
    fn missing_binary_is_an_hard_error() {
        let a = Snes9x::new(Snes9xConfig {
            binary: "snes9x-not-installed".into(),
            binary_path: None,
            fullscreen: true,
            mute: false,
            config_home: std::env::temp_dir().join("rosadeck-snes9x-missing"),
        });
        assert_eq!(a.prepare(&ctx()), Err(EmulatorError::BinaryNotFound("snes9x-not-installed".into())));
    }

    #[test]
    fn shaders_are_refused() {
        let dir = std::env::temp_dir().join("rosadeck-snes9x-shader");
        let mut c = ctx();
        c.shader_path = Some("/shaders/crt.slangp".into());
        assert!(matches!(adapter(&dir).prepare(&c), Err(EmulatorError::Unsupported(_))));
    }

    #[test]
    fn overlay_sets_both_keys_on_a_config_without_them() {
        let a = adapter(Path::new("/tmp/rosadeck-snes9x-a"));
        let text = a.overlay_text(None);
        assert!(text.contains("[Window State]"), "{text}");
        assert!(text.contains("[Display]"), "{text}");
        assert!(text.contains("Fullscreen = true"), "{text}");
        assert!(text.contains("FullscreenOnOpen = true"), "{text}");
    }

    #[test]
    fn overlay_keeps_the_users_settings_and_their_comments() {
        let user = "\
[Input]
Joypad1Port = 1

[Window State]
CurrentDisplayTab      = 1
Fullscreen             = false   # start fullscreen, please

[Sound]
Volume                 = 5
";
        let text = adapter(Path::new("/tmp/rosadeck-snes9x-b")).overlay_text(Some(user));
        assert!(text.contains("Fullscreen             = true   # start fullscreen, please"), "comment kept: {text}");
        assert!(text.contains("Joypad1Port = 1"), "other groups untouched: {text}");
        assert!(text.contains("Volume                 = 5"), "{text}");
        assert!(!text.contains("[Display]\nFullscreen = true\n\n[Display]"), "no duplicate group: {text}");
    }

    #[test]
    fn overlay_can_also_ask_for_windowed() {
        let a = Snes9x::new(Snes9xConfig {
            binary: "snes9x-gtk".into(),
            binary_path: None,
            fullscreen: false,
            mute: false,
            config_home: PathBuf::from("/tmp/rosadeck-snes9x-c"),
        });
        let text = a.overlay_text(Some("[Window State]\nFullscreen = true\n"));
        assert!(text.contains("Fullscreen = false"), "{text}");
        assert!(text.contains("FullscreenOnOpen = false"), "{text}");
    }

    #[test]
    fn finalize_writes_the_overlay_and_points_xdg_at_it() {
        let dir = std::env::temp_dir().join("rosadeck-snes9x-finalize");
        std::fs::remove_dir_all(&dir).ok();
        let a = adapter(&dir);
        let mut prepared = a.prepare(&ctx()).unwrap();
        assert!(!a.overlay_config().exists(), "prepare must not write anything");
        a.finalize(&mut prepared).unwrap();
        assert!(a.overlay_config().exists(), "finalize materialises the config");
        assert!(
            prepared.env.iter().any(|(k, v)| k == "XDG_CONFIG_HOME" && v == &dir.display().to_string()),
            "the child reads our config, not the user's: {prepared:?}"
        );
        // Idempotent: a second finalize must not rewrite identical bytes.
        let before = std::fs::metadata(&a.overlay_config()).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        a.finalize(&mut prepared).unwrap();
        assert_eq!(before, std::fs::metadata(&a.overlay_config()).unwrap().modified().unwrap(), "no churn");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn finalize_reports_an_unwritable_target_instead_of_launching_a_windowed_game() {
        let dir = std::env::temp_dir().join("rosadeck-snes9x-blocked");
        std::fs::remove_dir_all(&dir).ok();
        // A file where the config directory should be: create_dir_all fails.
        std::fs::write(&dir, b"x").unwrap();
        let a = adapter(&dir);
        let mut prepared = a.prepare(&ctx()).unwrap();
        assert!(matches!(a.finalize(&mut prepared), Err(EmulatorError::PrepareFailed(_))));
        std::fs::remove_file(&dir).ok();
    }
}
