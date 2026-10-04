//! `launch` use-case: Game → EmulatorProfile → DisplayProfile → DisplayPlan →
//! PreparedLaunch, then optionally the supervised run with display restore.
//!
//! The pipeline never touches displays directly: display work goes through
//! the F3 backend (`take_snapshot`/`run_transition`/restore); emulator work
//! goes through adapters. Dry-run executes neither.

use display_core::{ModePolicy, OutputId};
use rosadeck_backend_hyprland::{
    connected_connector_names, run_transition, save_snapshot, state_dir, take_snapshot, unify, exit_code_for,
    HyprctlRunner, Snapshot, DEFAULT_SYSFS,
};
use rosadeck_emulator_core::{Emulator, EmulatorSession, GameTarget, LaunchContext, PreparedLaunch, QuietSpawner};
use rosadeck_planner::{plan, DisplayRequest, ExtendPosition, PlanKind, parse_requested_mode};
use rosadeck_profiles::{find_emulator, load_emulator, load_launch, EmulatorProfile, LaunchProfile};
use std::path::{Path, PathBuf};

/// Extra exit codes for the launch pipeline (F3 codes 10–16 unchanged).
pub const EXIT_PREPARE_FAILED: i32 = 17;
/// Spawn failed.
pub const EXIT_LAUNCH_FAILED: i32 = 18;
/// Emulator exited non-zero (crash): display still restored first.
pub const EXIT_EMULATOR_NONZERO: i32 = 19;

fn config_base() -> PathBuf {
    let base = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()));
    rosadeck_game_library::dir_with_legacy(base.join(".config/rosadeck"), base.join(".config/hyprgame"))
}

/// All `*.toml` in `dir` (sorted; unreadable files reported, never fatal here).
fn toml_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut files: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "toml")).collect();
    files.sort();
    files
}

/// Load every valid emulator profile in `~/.config/rosadeck/emulators/`.
pub fn load_emulator_profiles() -> Vec<EmulatorProfile> {
    let mut out = Vec::new();
    for f in toml_files(&config_base().join("emulators")) {
        match load_emulator(&f) {
            Ok(p) => out.push(p),
            Err(e) => eprintln!("rosadeck: skip {}: {e}", f.display()),
        }
    }
    out
}

/// Load every valid launch binding in `~/.config/rosadeck/launchers/`.
pub fn load_launch_profiles() -> Vec<LaunchProfile> {
    let mut out = Vec::new();
    for f in toml_files(&config_base().join("launchers")) {
        match load_launch(&f) {
            Ok(p) => out.push(p),
            Err(e) => eprintln!("rosadeck: skip {}: {e}", f.display()),
        }
    }
    out
}

/// Where `--quiet` sends the emulator's stdout/stderr:
/// `$XDG_STATE_HOME/rosadeck/emulators.log`.
pub fn emulator_log() -> std::path::PathBuf {
    let base = std::env::var("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())).join(".local/state"));
    base.join("rosadeck/emulators.log")
}

/// Arch package that provides an emulator: printed for a human, never run.
pub fn install_hint(id: &str) -> Option<&'static str> {
    Some(match id {
        "dolphin" => "pacman -S dolphin-emu",
        "azahar" => "pacman -S azahar",
        "snes9x" => "pacman -S snes9x-gtk",
        "mupen64plus" => "pacman -S mupen64plus",
        "retroarch" => "pacman -S retroarch (+ a libretro core per system)",
        _ => return None,
    })
}

/// Build the adapter object for a validated emulator profile.
fn make_adapter(profile: &EmulatorProfile) -> Result<Box<dyn Emulator>, String> {
    let launch = profile.launch.clone().unwrap_or(rosadeck_profiles::EmulatorLaunchSection {
        extra_args: vec![],
        environment: Default::default(),
        workdir: None,
        config_path: None,
        append_config: None,
        core_path: None,
        user_dir: None,
        batch: false,
        mute: None,
        nospeedlimit: None,
        resolution: None,
        gfx_plugin: None,
    });
    let vsync = profile.graphics.as_ref().and_then(|g| g.vsync);
    // New philosophy: fullscreen by default; profiles opt out explicitly.
    let fullscreen = profile.graphics.as_ref().and_then(|g| g.fullscreen).unwrap_or(true);
    match profile.emulator.adapter.as_str() {
        "retroarch" => Ok(Box::new(rosadeck_emulator_retroarch::RetroArch::new(
            rosadeck_emulator_retroarch::RetroArchConfig {
                binary: profile.emulator.binary.clone(),
                binary_path: profile.emulator.binary_path.clone(),
                config_path: launch.config_path.clone(),
                append_config: launch.append_config.clone(),
                core_path: launch.core_path.clone(),
                fullscreen,
                vsync,
            },
        ))),
        "dolphin" => Ok(Box::new(rosadeck_emulator_dolphin::Dolphin::new(
            rosadeck_emulator_dolphin::DolphinConfig {
                binary: profile.emulator.binary.clone(),
                binary_path: profile.emulator.binary_path.clone(),
                user_dir: launch.user_dir.clone(),
                batch: launch.batch,
                fullscreen,
                vsync,
            },
        ))),
        "azahar" => Ok(Box::new(rosadeck_emulator_azahar::Azahar::new(
            rosadeck_emulator_azahar::AzaharConfig {
                binary: profile.emulator.binary.clone(),
                binary_path: profile.emulator.binary_path.clone(),
                fullscreen,
            },
        ))),
        "snes9x" => Ok(Box::new(rosadeck_emulator_snes9x::Snes9x::new(
            rosadeck_emulator_snes9x::Snes9xConfig {
                binary: profile.emulator.binary.clone(),
                binary_path: profile.emulator.binary_path.clone(),
                fullscreen,
                mute: launch.mute.unwrap_or(false),
                // Snes9x keeps its settings in $XDG_CONFIG_HOME/snes9x; the
                // adapter seeds its own copy there so the user's file is read,
                // never written.
                config_home: state_dir().join("snes9x-config"),
            },
        ))),
        "mupen64plus" => Ok(Box::new(rosadeck_emulator_mupen64plus::Mupen64Plus::new(
            rosadeck_emulator_mupen64plus::Mupen64PlusConfig {
                binary: profile.emulator.binary.clone(),
                binary_path: profile.emulator.binary_path.clone(),
                fullscreen,
                nospeedlimit: launch.nospeedlimit.unwrap_or(false),
                resolution: launch.resolution.clone(),
                gfx_plugin: launch.gfx_plugin.clone(),
            },
        ))),
        other => Err(format!("unknown adapter: {other}")),
    }
}

/// Options for `rosadeck launch` (CLI flags + optional launch profile).
pub struct LaunchOptions {
    /// Emulator profile id (or from launch profile).
    pub emulator: Option<String>,
    /// Game path (or from launch profile).
    pub game: Option<String>,
    /// Game title override.
    pub title: Option<String>,
    /// Platform override (default from launch profile or `unknown`).
    pub platform: Option<String>,
    /// Display profile alias/stable (or from launch profile).
    pub display_profile: Option<String>,
    /// Launch binding id.
    pub launch_profile: Option<String>,
    /// Explicit `WxH@Hz` (implies manual).
    pub mode: Option<String>,
    /// Policy name.
    pub policy: Option<String>,
    /// Extend position.
    pub pos: Option<String>,
    /// Print only.
    pub dry_run: bool,
    /// Machine JSON.
    pub json: bool,
    /// Skip confirmation.
    pub yes: bool,
    /// Detach the emulator's console output to a log file.
    ///
    /// Used by the library: an emulator that writes *after* it exits (Dolphin's
    /// SIGTERM handler dumps its argv) would otherwise scribble over the browser
    /// that is waiting for it. Off by default, so a human at a shell still sees
    /// the emulator's own output.
    pub quiet: bool,
}

/// Resolve options → (emulator profile, game, display request or None).
/// Pure against loaded profiles (no display IO); errors are INVALID (11).
fn resolve_launch(
    opts: &LaunchOptions,
    emulators: &[EmulatorProfile],
    launches: &[LaunchProfile],
) -> Result<(EmulatorProfile, GameTarget, Option<DisplayRequest>, Option<String>), String> {
    let binding = match &opts.launch_profile {
        Some(id) => Some(
            launches.iter().find(|l| l.launch.id == *id).ok_or_else(|| format!("unknown launch profile: {id}"))?,
        ),
        None => None,
    };
    let emulator_id = opts
        .emulator
        .clone()
        .or_else(|| binding.map(|b| b.launch.emulator_profile.clone()))
        .ok_or_else(|| "--emulator or --launch-profile required".to_owned())?;
    let emulator = find_emulator(emulators, &emulator_id)
        .ok_or_else(|| format!("unknown emulator profile: {emulator_id}"))?
        .clone();
    let game_path = opts.game.clone().or_else(|| binding.and_then(|b| b.game.as_ref().map(|g| g.path.clone())))
        .ok_or_else(|| "--game or launch profile game required".to_owned())?;
    let game = GameTarget {
        platform: opts.platform.clone().or_else(|| binding.and_then(|b| b.game.as_ref().map(|g| g.platform.clone()))).unwrap_or_else(|| "unknown".into()),
        path: game_path,
        title: opts.title.clone().or_else(|| binding.and_then(|b| b.game.as_ref().and_then(|g| g.title.clone()))),
    };
    game.validate().map_err(|e| format!("INVALID game: {e}"))?;
    // Display request (None = launch without touching displays).
    let disp_cfg = binding.and_then(|b| b.display.as_ref());
    let disposition = disp_cfg.map(|d| d.disposition.clone());
    let want_display = opts.display_profile.is_some() || binding.is_some() || opts.mode.is_some() || opts.policy.is_some() || opts.pos.is_some();
    if !want_display {
        return Ok((emulator, game, None, None));
    }
    let kind = match disposition.as_deref().unwrap_or("external-only") {
        "duplicate" => PlanKind::Duplicate,
        "external-only" => PlanKind::ExternalOnly,
        "extend" => PlanKind::Extend,
        other => return Err(format!("INVALID disposition: {other}")),
    };
    let policy_name = opts.policy.clone().or_else(|| disp_cfg.map(|d| d.policy.clone())).unwrap_or_else(|| "max-refresh".into());
    let requested = match opts.mode.clone().or_else(|| disp_cfg.and_then(|d| d.resolution.clone())) {
        Some(s) => Some(parse_requested_mode(&s).ok_or_else(|| format!("INVALID --mode {s}"))?),
        None => None,
    };
    let mut policy = match policy_name.as_str() {
        "max-resolution" => ModePolicy::MaxResolution,
        "max-refresh" => ModePolicy::MaxRefresh,
        "balanced" => ModePolicy::Balanced,
        "match-internal" => ModePolicy::MatchInternal,
        "match-external" => ModePolicy::MatchExternal,
        "manual" => ModePolicy::Manual(requested.ok_or_else(|| "manual policy needs --mode".to_owned())?),
        other => return Err(format!("INVALID policy: {other}")),
    };
    if requested.is_some() && !matches!(policy, ModePolicy::Manual(_)) {
        policy = ModePolicy::Manual(requested.unwrap());
    }
    let position = match opts.pos.as_deref() {
        Some("right") | None if kind != PlanKind::Extend => None,
        Some("right") => Some(ExtendPosition::Right),
        Some("left") => Some(ExtendPosition::Left),
        Some("up") => Some(ExtendPosition::Up),
        Some("down") => Some(ExtendPosition::Down),
        Some(other) => return Err(format!("INVALID --pos {other}")),
        None => None,
    };
    let display_alias = opts.display_profile.clone().or_else(|| binding.map(|b| b.launch.display_profile.clone()));
    Ok((
        emulator,
        game,
        Some(DisplayRequest {
            kind,
            target: None, // resolved below against live outputs via alias
            policy,
            requested_mode: requested,
            position,
        }),
        display_alias,
    ))
}

/// Target output = connected external whose EDID identity or alias matches,
/// else first connected external (explicit `--on` equivalent comes later).
fn resolve_target(live: &[rosadeck_backend_hyprland::UnifiedDisplay], alias: Option<&str>) -> Option<OutputId> {
    let externals: Vec<_> = live.iter().filter(|d| d.drm_connected && !rosadeck_planner::is_internal(&d.connector.0)).collect();
    if let Some(a) = alias {
        for d in &externals {
            let stable = d.identity.as_ref().map(|i| i.stable_id());
            if stable.as_deref() == Some(a) || d.edid_name.as_deref() == Some(a) {
                return Some(OutputId(d.connector.0.clone()));
            }
        }
        // Named display not present: fall through to explicit failure below.
        return None;
    }
    externals.first().map(|d| OutputId(d.connector.0.clone()))
}

/// `rosadeck launch`. Dry-run prints the whole pipeline and changes nothing.
pub fn cmd_launch(opts: &LaunchOptions) -> i32 {
    cmd_launch_timed(opts).0
}

/// Like [`cmd_launch`], and it also says how many seconds the emulator ran.
///
/// The browser waits for the CLI, so the CLI is the only place that knows how
/// long the game was on screen; measuring anywhere else would time the wrong
/// thing (a preflight, or nothing at all).
pub fn cmd_launch_timed(opts: &LaunchOptions) -> (i32, u64) {
    let emulators = load_emulator_profiles();
    let launches = load_launch_profiles();
    let (emulator_profile, game, display_req, display_alias) = match resolve_launch(opts, &emulators, &launches) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("rosadeck: {e}");
            return (11, 0);
        }
    };
    // Shader preset ( RetroArch only, resolved read-only).
    let shader_preset = emulator_profile.shader.as_ref().and_then(|s| s.preset.clone());
    let shader_path = match shader_preset {
        Some(p) if p != "off" => {
            let reg = rosadeck_shader_manager::ShaderRegistry::from_dir(&config_base().join("shaders"));
            match reg.resolve(&p, &emulator_profile.emulator.adapter) {
                Ok(r) => Some(r.path.to_string_lossy().into_owned()),
                Err(e) => {
                    eprintln!("rosadeck: shader: {e}");
                    return (11, 0);
                }
            }
        }
        _ => None,
    };
    let adapter = match make_adapter(&emulator_profile) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("rosadeck: {e}");
            return (11, 0);
        }
    };
    let ctx = LaunchContext {
        game: game.clone(),
        emulator_id: emulator_profile.emulator.id.clone(),
        display_mode_label: None,
        display_mode_requested: None,
        shader_path,
        extra_env: emulator_profile.launch.as_ref().map(|l| l.environment.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default(),
        extra_args: emulator_profile.launch.as_ref().map(|l| l.extra_args.clone()).unwrap_or_default(),
        workdir: emulator_profile.launch.as_ref().and_then(|l| l.workdir.clone()).map(PathBuf::from),
    };
    let prepared: PreparedLaunch = match adapter.prepare(&ctx) {
        Ok(p) => p,
        Err(rosadeck_emulator_core::EmulatorError::BinaryNotFound(b)) => {
            // Say which binary and where to get it. Rosadeck never installs.
            eprintln!("rosadeck: PREPARE_FAILED: emulator binary not found: {b}");
            if let Some(hint) = install_hint(&emulator_profile.emulator.id) {
                eprintln!("rosadeck: install it yourself with: {hint}");
            }
            eprintln!("rosadeck: check with: rosadeck emulators");
            return (EXIT_PREPARE_FAILED, 0);
        }
        Err(e) => {
            eprintln!("rosadeck: PREPARE_FAILED: {e}");
            return (EXIT_PREPARE_FAILED, 0);
        }
    };
    // Display plan (in-memory unless executing).
    let mut session = EmulatorSession::new(&emulator_profile.emulator.id, game.title.as_deref().unwrap_or(&game.path));
    if session.step(rosadeck_emulator_core::SessionState::Preparing).is_err() {
        eprintln!("rosadeck: session error");
        return (2, 0);
    }
    let (snapshot, plan) = match display_req {
        Some(mut req) => {
            let snapshot = match take_snapshot() {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("rosadeck: backend: {e}");
                    return (2, 0);
                }
            };
            let live = match unify(std::path::Path::new(DEFAULT_SYSFS)) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("rosadeck: backend: {e}");
                    return (2, 0);
                }
            };
            req.target = resolve_target(&live, display_alias.as_deref());
            if req.target.is_none() {
                eprintln!("rosadeck: NO_EXTERNAL_OUTPUT");
                return (15, 0);
            }
            let input = super::planning_input_from(&snapshot, &live);
            match plan(&req, &input) {
                Ok(p) => (Some(snapshot), Some(p)),
                Err(e) => {
                    eprintln!("rosadeck: {e}");
                    return (
                        match e {
                            rosadeck_planner::PlanError::NoCommonMode => 10,
                            rosadeck_planner::PlanError::NoExternalOutput | rosadeck_planner::PlanError::NoInternalOutput => 15,
                            rosadeck_planner::PlanError::WouldDisableLastOutput => 16,
                            _ => 11,
                        },
                        0,
                    );
                }
            }
        }
        None => (None, None),
    };
    if opts.json {
        println!(
            "{}",
            serde_json::json!({
                "game": game,
                "emulator": emulator_profile.emulator.id,
                "display_plan": plan,
                "prepared": prepared,
                "result": "VALID",
            })
        );
    } else {
        println!("Display:");
        match &plan {
            Some(p) => {
                for s in &p.steps {
                    println!("  keyword monitor \"{}\"", s.to_keyword_arg());
                }
            }
            None => println!("  (unchanged)"),
        }
        println!("Emulator:\n  {}", prepared.summary());
        for n in &prepared.notes {
            println!("  note: {n}");
        }
        println!("Game:\n  {} {}", game.platform, game.path);
        println!("Shader:\n  {}", ctx.shader_path.as_deref().unwrap_or("disabled"));
        println!("Result: VALID");
    }
    if opts.dry_run {
        return (0, 0);
    }
    if !opts.yes && !super::confirm("Launch (applies display plan first)?") {
        eprintln!("rosadeck: aborted (needs --yes)");
        return (2, 0);
    }
    let started = std::time::Instant::now();
    let code = run_pipeline(session, snapshot, plan, adapter.as_ref(), &ctx, &prepared, opts.quiet);
    (code, started.elapsed().as_secs())
}

/// Supervised run: display apply→verify → spawn → wait → cleanup → restore.
/// Order rule: never launch before the display verifies.
fn run_pipeline(
    mut session: EmulatorSession,
    snapshot: Option<Snapshot>,
    plan: Option<rosadeck_planner::DisplayPlan>,
    adapter: &dyn rosadeck_emulator_core::Emulator,
    ctx: &LaunchContext,
    prepared: &PreparedLaunch,
    quiet: bool,
) -> i32 {
    // `finalize` may add env/args (a generated config), so work on a copy and
    // only then hand it to the spawner.
    let mut prepared = prepared.clone();
    use rosadeck_emulator_core::SessionState;
    // 1. Display first (or skip when no plan).
    if let (Some(snap), Some(p)) = (&snapshot, &plan) {
        if session.step(SessionState::DisplayApplying).is_err() {
            return fail(&mut session, "session");
        }
        if save_snapshot(&state_dir(), snap).is_err() {
            return fail(&mut session, "cannot persist snapshot");
        }
        let mut runner = HyprctlRunner;
        let read = || unify(std::path::Path::new(DEFAULT_SYSFS));
        let connected = || connected_connector_names(std::path::Path::new(DEFAULT_SYSFS));
        match run_transition(snap, p, &mut runner, &read, &connected) {
            Ok(_) => {
                if session.step(SessionState::DisplayVerified).is_err() {
                    return fail(&mut session, "session");
                }
            }
            Err(e) => {
                let code = exit_code_for(&e);
                let _ = session.step(SessionState::Failed(e.to_string()));
                eprintln!("rosadeck: display failed, emulator NOT launched: {e}");
                return code;
            }
        }
    }
    // 2. Spawn supervised.
    if session.step(SessionState::Launching).is_err() {
        return fail(&mut session, "session");
    }
    // The emulator draws in its own window (X11/Wayland); its console output
    // only has to go somewhere that is not the terminal we are about to repaint.
    // Only now, with the display verified and a spawn certain, may the adapter
    // materialise files (Snes9x needs a patched config for fullscreen).
    if let Err(e) = adapter.finalize(&mut prepared) {
        restore_display(&mut session, snapshot.as_ref());
        let _ = session.step(SessionState::Failed(e.to_string()));
        eprintln!("rosadeck: PREPARE_FAILED: {e}");
        return EXIT_PREPARE_FAILED;
    }
    let quiet_spawner = QuietSpawner::new(emulator_log());
    let std_spawner = rosadeck_emulator_core::process::StdSpawner;
    let spawner: &dyn rosadeck_emulator_core::Spawner = if quiet { &quiet_spawner } else { &std_spawner };
    let mut child = match adapter.spawn(&prepared, spawner) {
        Ok(c) => c,
        Err(e) => {
            restore_display(&mut session, snapshot.as_ref());
            let _ = session.step(SessionState::Failed(e.to_string()));
            eprintln!("rosadeck: LAUNCH_FAILED: {e}");
            return EXIT_LAUNCH_FAILED;
        }
    };
    let _ = session.step(SessionState::Running);
    let code = child.wait().unwrap_or(-1);
    let _ = session.step(SessionState::Stopping);
    // 3. Cleanup (idempotent) then restore display, always.
    finish_session(&mut session, adapter, ctx, snapshot.as_ref());
    if code != 0 {
        // Only claim a display was restored when one was actually taken over:
        // on the library's path (no HDMI, no plan) nothing was touched, and
        // saying otherwise is a lie the player reads on close.
        let display_note = if snapshot.is_some() { "display restored" } else { "display untouched" };
        eprintln!("rosadeck: emulator exited {code} ({display_note})");
        return EXIT_EMULATOR_NONZERO;
    }
    0
}

/// Cleanup (twice-proof) + display restore + terminal state.
fn finish_session(
    session: &mut EmulatorSession,
    adapter: &dyn rosadeck_emulator_core::Emulator,
    ctx: &LaunchContext,
    snapshot: Option<&Snapshot>,
) {
    use rosadeck_emulator_core::SessionState;
    let _ = session.step(SessionState::Cleaning);
    let _ = adapter.cleanup(ctx);
    session.note_cleanup();
    restore_display(session, snapshot);
}

/// Re-apply the pre-launch snapshot (best effort, then terminal state).
fn restore_display(session: &mut EmulatorSession, snapshot: Option<&Snapshot>) {
    use rosadeck_emulator_core::SessionState;
    use rosadeck_backend_hyprland::build_restore_plan;
    let Some(snap) = snapshot else {
        let _ = session.step(SessionState::Finished);
        return;
    };
    let _ = session.step(SessionState::Restoring);
    let connected = connected_connector_names(std::path::Path::new(DEFAULT_SYSFS));
    let restore = build_restore_plan(snap, &connected);
    let mut runner = HyprctlRunner;
    let read = || unify(std::path::Path::new(DEFAULT_SYSFS));
    let mut log = vec![];
    match rosadeck_backend_hyprland::apply_plan(&restore, &mut runner, &mut log) {
        Ok(()) => match rosadeck_backend_hyprland::verify_plan(&restore, &read, &mut log) {
            Ok(()) => {
                let _ = session.step(SessionState::Finished);
            }
            Err(e) => {
                let _ = session.step(SessionState::Failed(format!("restore verify: {e}")));
            }
        },
        Err(e) => {
            let _ = session.step(SessionState::Failed(format!("restore apply: {e}")));
        }
    }
    // Idempotence witness: a second cleanup pass stays Ok.
    for line in log {
        eprintln!("[rosadeck] {line}");
    }
}

fn fail(session: &mut EmulatorSession, msg: &str) -> i32 {
    use rosadeck_emulator_core::SessionState;
    let _ = session.step(SessionState::Failed(msg.into()));
    eprintln!("rosadeck: {msg}");
    2
}
