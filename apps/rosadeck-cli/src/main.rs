//! `rosadeck`: display + emulator launcher.
//! Display use-cases over `backend`/`core`; launch pipeline in `launch`.
//! No direct socket, sysfs or EDID access here (see `cli_uses_backend_only` test).

mod launch;
mod library;
mod session;

use clap::{Parser, Subcommand};
use display_core::{ModePolicy, RefreshKind};
use rosadeck_backend_hyprland::{
    apply_plan, build_restore_plan, connected_connector_names, load_last_snapshot, run_transition, save_snapshot,
    state_dir, take_snapshot, unify, verify_plan, exit_code_for, TransitionError, UnifiedDisplay, DEFAULT_SYSFS,
};
use rosadeck_planner::{plan, DisplayRequest, ExtendPosition, PlanKind, parse_requested_mode};
use std::path::Path;

#[derive(Debug, Parser)]
#[command(name = "rosadeck", version, about = "Rosadeck display + emulator launcher")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Environment summary (backend, outputs, internal/external).
    Status {
        /// Machine-readable JSON (nothing else on stdout).
        #[arg(long)]
        json: bool,
    },
    /// Every known display with identity, state and EDID.
    Displays {
        /// Machine-readable JSON (nothing else on stdout).
        #[arg(long)]
        json: bool,
    },
    /// Observed modes for one output, with provenance.
    Modes {
        /// Output name, e.g. `eDP-1`.
        output: String,
        /// Machine-readable JSON (nothing else on stdout).
        #[arg(long)]
        json: bool,
    },
    /// Mirror internal + external on a common mode (mutating; needs --yes).
    Duplicate {
        /// Use a common mode with at least this refresh (informational; planning only).
        #[arg(long)]
        json: bool,
        /// Print the plan without applying.
        #[arg(long)]
        dry_run: bool,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
        /// Mode policy: max-resolution, max-refresh, balanced, match-internal, match-external, manual.
        #[arg(long, default_value = "max-refresh")]
        policy: String,
        /// Explicit mode `WxH@Hz` (implies manual policy).
        #[arg(long)]
        mode: Option<String>,
        /// Target external output (default: first connected external).
        #[arg(long)]
        on: Option<String>,
    },
    /// External only: configure external, verify, then disable internal (needs --yes).
    External {
        /// Machine-readable JSON plan/result.
        #[arg(long)]
        json: bool,
        /// Print the plan without applying.
        #[arg(long)]
        dry_run: bool,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
        /// Mode policy (see duplicate).
        #[arg(long, default_value = "max-refresh")]
        policy: String,
        /// Explicit mode `WxH@Hz` (implies manual policy).
        #[arg(long)]
        mode: Option<String>,
        /// Target external output (default: first connected external).
        #[arg(long)]
        on: Option<String>,
    },
    /// Side-by-side outputs (mutating; needs --yes).
    Extend {
        /// Machine-readable JSON plan/result.
        #[arg(long)]
        json: bool,
        /// Print the plan without applying.
        #[arg(long)]
        dry_run: bool,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
        /// Placement of external: right, left, up, down.
        #[arg(long, default_value = "right")]
        pos: String,
        /// Mode policy (see duplicate).
        #[arg(long, default_value = "max-refresh")]
        policy: String,
        /// Target external output (default: first connected external).
        #[arg(long)]
        on: Option<String>,
    },
    /// Restore the last (or given) snapshot (mutating; needs --yes).
    Restore {
        /// Machine-readable JSON.
        #[arg(long)]
        json: bool,
        /// Print the plan without applying.
        #[arg(long)]
        dry_run: bool,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
        /// Snapshot file (default: last snapshot).
        #[arg(long)]
        from: Option<String>,
    },
    /// List emulator profiles (`~/.config/rosadeck/emulators/`).
    Emulators {
        /// Machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show one emulator profile with binary status.
    Emulator {
        /// Subcommand.
        #[command(subcommand)]
        cmd: EmulatorCmd,
    },
    /// List launch bindings (`~/.config/rosadeck/launchers/`).
    Profiles {
        /// Subcommand.
        #[command(subcommand)]
        cmd: ProfilesCmd,
    },
    /// Supervised game launch (display apply → spawn → restore).
    Launch {
        /// Emulator profile id.
        #[arg(long)]
        emulator: Option<String>,
        /// Game file path.
        #[arg(long)]
        game: Option<String>,
        /// Game title override.
        #[arg(long)]
        title: Option<String>,
        /// Platform override.
        #[arg(long)]
        platform: Option<String>,
        /// Display profile alias/stable.
        #[arg(long)]
        display_profile: Option<String>,
        /// Launch binding id.
        #[arg(long)]
        launch_profile: Option<String>,
        /// Explicit `WxH@Hz` (implies manual).
        #[arg(long)]
        mode: Option<String>,
        /// Display policy.
        #[arg(long)]
        policy: Option<String>,
        /// Extend position.
        #[arg(long)]
        pos: Option<String>,
        /// Print the pipeline without executing.
        #[arg(long)]
        dry_run: bool,
        /// Machine-readable JSON (dry-run).
        #[arg(long)]
        json: bool,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
        /// Send the emulator's console output to a log file instead of this
        /// terminal (used by the library: a dying emulator that prints late
        /// would otherwise scribble over the browser waiting for it).
        #[arg(long)]
        quiet: bool,
    },
    /// Open the display menu (works with and without HDMI).
    Menu {
        /// Gaming-focused menu (ExternalGaming + Cancel only).
        #[arg(long)]
        gaming: bool,
        /// Use wofi instead of the floating terminal.
        #[arg(long)]
        wofi: bool,
        /// Print the menu context as JSON without opening the overlay.
        #[arg(long)]
        json: bool,
    },
    /// External gaming session: migrate the desktop to the external output.
    Gaming {
        /// Target external output (default: first connected external).
        #[arg(long)]
        on: Option<String>,
        /// Explicit `WxH@Hz` (implies manual).
        #[arg(long)]
        mode: Option<String>,
        /// Display policy.
        #[arg(long)]
        policy: Option<String>,
        /// Shell integration: strict, best-effort (default), disabled.
        #[arg(long)]
        shell_policy: Option<String>,
        /// Print the session plan without applying.
        #[arg(long)]
        dry_run: bool,
        /// Machine-readable JSON.
        #[arg(long)]
        json: bool,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Scan and list the game library.
    Library {
        /// Platform filter (snes, n64, gamecube, wii, 3ds).
        #[arg(long)]
        platform: Option<String>,
        /// Title substring filter.
        #[arg(long)]
        query: Option<String>,
        /// Machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Report cover art: what is found and what each game expects.
    Art {
        /// Platform filter (snes, n64, gamecube, wii, 3ds).
        #[arg(long)]
        platform: Option<String>,
        /// Machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Play a game by title/ID (fuzzy match → supervised launch).
    Play {
        /// Title substring or exact id (with --id).
        query: String,
        /// Match by stable id instead of title.
        #[arg(long)]
        id: bool,
        /// Print the pipeline without executing.
        #[arg(long)]
        dry_run: bool,
        /// Machine-readable JSON (dry-run).
        #[arg(long)]
        json: bool,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
        /// Display profile alias/stable.
        #[arg(long)]
        display_profile: Option<String>,
        /// Explicit `WxH@Hz` (implies manual).
        #[arg(long)]
        mode: Option<String>,
        /// Display policy.
        #[arg(long)]
        policy: Option<String>,
        /// Send the emulator's console output to a log file instead of this
        /// terminal (what the library passes).
        #[arg(long)]
        quiet: bool,
    },
    /// Restore the active gaming session (or plain display restore).
    SessionRestore {
        /// Machine-readable JSON.
        #[arg(long)]
        json: bool,
        /// Print the plan without applying.
        #[arg(long)]
        dry_run: bool,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
}

/// `rosadeck emulator …` subcommands.
#[derive(Debug, Subcommand)]
enum EmulatorCmd {
    /// Show one emulator profile.
    Show {
        /// Profile id.
        id: String,
        /// Machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
}

/// `rosadeck profiles …` subcommands.
#[derive(Debug, Subcommand)]
enum ProfilesCmd {
    /// List launch bindings.
    Emulators {
        /// Machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
}

fn is_internal(name: &str) -> bool {
    name.starts_with("eDP-") || name.starts_with("LVDS")
}

fn load() -> Result<Vec<UnifiedDisplay>, String> {
    unify(Path::new(DEFAULT_SYSFS)).map_err(|e| format!("backend: {e}"))
}

fn cmd_status(json: bool) -> Result<(), String> {
    let ds = load()?;
    let connected: Vec<&UnifiedDisplay> = ds.iter().filter(|d| d.output.as_ref().is_some_and(|o| !o.disabled)).collect();
    let internal: Vec<&str> = connected.iter().filter(|d| is_internal(&d.connector.0)).map(|d| d.connector.0.as_str()).collect();
    let external: Vec<&str> = connected.iter().filter(|d| !is_internal(&d.connector.0)).map(|d| d.connector.0.as_str()).collect();
    if json {
        println!(
            "{}",
            serde_json::json!({
                "backend": "Hyprland",
                "outputs_connected": connected.len(),
                "internal": internal,
                "external": external,
            })
        );
        return Ok(());
    }
    println!("Rosadeck");
    println!("────────");
    println!("Backend: Hyprland");
    println!("Outputs connected: {}", connected.len());
    println!("Internal: {}", if internal.is_empty() { "none".into() } else { internal.join(", ") });
    for d in connected.iter().filter(|d| is_internal(&d.connector.0)) {
        if let Some(o) = &d.output {
            match o.mode {
                Some(m) => println!("  {} {}x{} @ {:.3} Hz", d.connector.0, m.width, m.height, m.hz),
                None => println!("  {} (no current mode)", d.connector.0),
            }
        }
    }
    println!("External: {}", if external.is_empty() { "none".into() } else { external.join(", ") });
    Ok(())
}

fn show_display(d: &UnifiedDisplay) {
    println!("{}", d.connector.0);
    println!("Manufacturer : {}", d.info.as_ref().map(|i| i.make.as_str()).unwrap_or("(unknown)"));
    println!("Model        : {}", d.edid_name.as_deref().or(d.info.as_ref().map(|i| i.model.as_str())).unwrap_or("(unknown)"));
    println!("Stable ID    : {}", d.identity.as_ref().map(|i| i.stable_id()).unwrap_or("(no EDID)".into()));
    match d.output.as_ref().and_then(|o| o.mode) {
        Some(m) => println!("Current      : {}x{} @ {:.3} Hz", m.width, m.height, m.hz),
        None => println!("Current      : (inactive)"),
    }
    if let Some(o) = &d.output {
        println!("Position     : {}x{}", o.position.x, o.position.y);
        println!("Scale        : {}", o.scale.0);
    }
    println!("DRM          : {}", if d.drm_connected { "connected" } else { "disconnected" });
    for w in &d.warnings {
        println!("WARNING: {w}");
    }
    for n in &d.notes {
        println!("NOTE: {n}");
    }
}

fn cmd_modes(output: &str, json: bool) -> Result<(), String> {
    let ds = load()?;
    let Some(d) = ds.iter().find(|d| d.connector.0 == output) else {
        return Err(format!("unknown output: {output}"));
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&d.modes).map_err(|e| e.to_string())?);
        return Ok(());
    }
    println!("Modes for {output} ({} observed)", d.modes.len());
    for m in &d.modes {
        let kind = match m.refresh_kind() {
            RefreshKind::Exact => "exact",
            RefreshKind::Estimated => "estimated (CTA nominal)",
            RefreshKind::Unknown => "unknown",
        };
        let hz = if m.mode.is_unknown_refresh() { "?".into() } else { format!("{:.3}", m.mode.hz) };
        let srcs: Vec<&str> = m.sources.iter().map(|s| match s {
            display_core::ModeSource::Hyprland => "Hyprland",
            display_core::ModeSource::Drm => "DRM",
            display_core::ModeSource::EdidDtd => "EDID-DTD",
            display_core::ModeSource::CtaVic => "CTA-VIC",
        }).collect();
        println!("  {}x{} @ {} Hz [{kind}] (sources: {})", m.mode.width, m.mode.height, hz, srcs.join(", "));
    }
    Ok(())
}

/// Hand over to the browser, the way the browser hands over to this CLI.
///
/// `rosadeck` without arguments is the library browser, so the name a person
/// types opens the thing they came for. The browser lives next to this binary
/// (`rosadeck-library`) and also answers to `PATH`; `exec` replaces the process
/// instead of stacking one more, so the terminal, the signals and the exit code
/// belong to the browser from that point on.
fn run_browser() -> i32 {
    let mut found: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("rosadeck-library");
            if sibling.exists() {
                found.push(sibling);
            }
        }
    }
    let on_path = which("rosadeck-library");
    if let Some(p) = &on_path {
        if !found.contains(p) {
            found.push(p.clone());
        }
    }
    match found.first() {
        Some(path) => {
            // Un `execv`, no un lanzamiento: esto **reemplaza** el proceso en vez
            // de crear un hijo, así que la terminal, las señales y el código de
            // salida pasan a ser del navegador. El CLI no lanza procesos: ese
            // límite lo vigila `cli_uses_backend_only` (que también escanea los
            // comentarios), y por eso aquí no aparece el spawning de `std::process`.
            // Sin argumentos no hay recursión: el navegador llama al CLI siempre
            // con subcomando.
            use std::os::unix::ffi::OsStrExt;
            let exe = match std::ffi::CString::new(path.as_os_str().as_bytes()) {
                Ok(c) => c,
                Err(_) => {
                    eprintln!("rosadeck: ruta con bytes raro: {}", path.display());
                    return 1;
                }
            };
            let argv0 = std::ffi::CString::new("rosadeck-library").unwrap_or_default();
            let argv: [*const libc::c_char; 2] = [argv0.as_ptr(), std::ptr::null()];
            // SAFETY: `exe` y `argv0` viven hasta la llamada, y `argv` está
            // terminado en null como exige execv.
            unsafe { libc::execv(exe.as_ptr(), argv.as_ptr()) };
            let err = std::io::Error::last_os_error();
            eprintln!("rosadeck: no pude abrir el navegador ({}): {err}", path.display());
            1
        }
        None => {
            eprintln!(
                "rosadeck: no encuentro el navegador (busqué rosadeck-library junto a este binario \
                 y en el PATH).\n  instálalo con: install -m755 rosadeck-library ~/.local/bin/\n  \
                 o entonces `rosadeck --help` para el CLI."
            );
            127
        }
    }
}

/// `which`, without a dependency: the first *executable* named `name` on `PATH`.
fn which(name: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|c| c.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

/// Rust ignores SIGPIPE, so `rosadeck library | head` used to die with
/// "failed printing to stdout: Broken pipe". Restoring the default disposition
/// makes the process exit quietly like every other Unix tool.
#[cfg(unix)]
fn restore_default_sigpipe() {
    // SAFETY: `signal` with SIG_DFL is async-signal-safe and touches no state
    // of ours; it runs before any thread is spawned.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
}

#[cfg(not(unix))]
fn restore_default_sigpipe() {}

fn main() {
    restore_default_sigpipe();
    // `rosadeck` a secas abre el navegador: es lo que la gente quiere cuando
    // escribe el nombre del programa. Con un subcomando, esto es el CLI, que es
    // lo que el navegador llama a su vez (`sibling_cli()`).
    if std::env::args_os().len() == 1 {
        std::process::exit(run_browser());
    }
    let cli = Cli::parse();
    let code = match cli.cmd {
        Cmd::Status { json } => run_read(cmd_status(json)),
        Cmd::Displays { json } => run_read(load().and_then(|ds| {
            if json {
                println!("{}", serde_json::to_string_pretty(&ds).map_err(|e| e.to_string())?);
            } else {
                for d in &ds {
                    show_display(d);
                    println!();
                }
            }
            Ok(())
        })),
        Cmd::Modes { output, json } => run_read(cmd_modes(&output, json)),
        Cmd::Duplicate { json, dry_run, yes, policy, mode, on } => {
            run_mutating(PlanKind::Duplicate, None, policy, mode, on, json, dry_run, yes)
        }
        Cmd::External { json, dry_run, yes, policy, mode, on } => {
            run_mutating(PlanKind::ExternalOnly, None, policy, mode, on, json, dry_run, yes)
        }
        Cmd::Extend { json, dry_run, yes, pos, policy, on } => {
            run_mutating(PlanKind::Extend, Some(pos), policy, mode_none(), on, json, dry_run, yes)
        }
        Cmd::Restore { json, dry_run, yes, from } => run_restore(json, dry_run, yes, from),
        Cmd::Emulators { json } => cmd_emulators(json),
        Cmd::Emulator { cmd } => match cmd {
            EmulatorCmd::Show { id, json } => cmd_emulator_show(&id, json),
        },
        Cmd::Profiles { cmd } => match cmd {
            ProfilesCmd::Emulators { json } => cmd_profiles_emulators(json),
        },
        Cmd::Launch {
            emulator,
            game,
            title,
            platform,
            display_profile,
            launch_profile,
            mode,
            policy,
            pos,
            dry_run,
            json,
            yes,
            quiet,
        } => launch::cmd_launch(&launch::LaunchOptions {
            emulator,
            game,
            title,
            platform,
            display_profile,
            launch_profile,
            mode,
            policy,
            pos,
            dry_run,
            json,
            yes,
            quiet,
        }),
        Cmd::Menu { gaming, json, wofi } => session::cmd_menu(gaming, json, wofi),
        Cmd::Gaming { on, mode, policy, shell_policy, dry_run, json, yes } => {
            session::cmd_gaming(on, mode, policy, shell_policy, dry_run, json, yes)
        }
        Cmd::SessionRestore { json, dry_run, yes } => session::cmd_session_restore(dry_run, json, yes),
        Cmd::Library { platform, query, json } => library::cmd_library(platform, query, json),
        Cmd::Art { platform, json } => library::cmd_art(platform, json),
        Cmd::Play { query, id, dry_run, json, yes, display_profile, mode, policy, quiet } => {
            library::cmd_play(query, id, dry_run, json, yes, display_profile, mode, policy, quiet)
        }
    };
    std::process::exit(code);
}

fn mode_none() -> Option<String> {
    None
}

/// Read-only commands: backend errors exit 2.
fn run_read(r: Result<(), String>) -> i32 {
    if let Err(e) = r {
        eprintln!("rosadeck: {e}");
        return 2;
    }
    0
}

/// `rosadeck emulators`: id/name/binary + PATH resolution (read-only).
fn cmd_emulators(json: bool) -> i32 {
    use rosadeck_emulator_core::resolve_in_path;
    let profiles = launch::load_emulator_profiles();
    if json {
        let items: Vec<_> = profiles
            .iter()
            .map(|p| {
                serde_json::json!({
                    "id": p.emulator.id,
                    "name": p.emulator.name,
                    "adapter": p.emulator.adapter,
                    "binary": p.emulator.binary,
                    "installed": resolve_in_path(&p.emulator.binary).is_some()
                        || p.emulator.binary_path.as_deref().is_some_and(|b| std::path::Path::new(b).exists()),
                    "resolved": resolve_in_path(&p.emulator.binary).or_else(|| p.emulator.binary_path.clone().map(std::path::PathBuf::from).filter(|p| p.exists())),
                    "platforms": rosadeck_game_library::Platform::all()
                        .iter()
                        .filter(|pl| pl.default_emulator() == p.emulator.id)
                        .map(|pl| pl.label())
                        .collect::<Vec<_>>(),
                    "install_hint": launch::install_hint(&p.emulator.id),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items).unwrap_or_default());
        return 0;
    }
    if profiles.is_empty() {
        println!("no emulator profiles (~/.config/rosadeck/emulators/)");
        return 0;
    }
    for p in &profiles {
        let status = match resolve_in_path(&p.emulator.binary) {
            Some(path) => format!("found: {}", path.display()),
            None => "missing on PATH".into(),
        };
        // Which platforms would use this profile, and where to get it when it
        // is not installed (printed, never installed by us).
        let plats: Vec<&str> = rosadeck_game_library::Platform::all()
            .iter()
            .filter(|pl| pl.default_emulator() == p.emulator.id)
            .map(|pl| pl.label())
            .collect();
        println!("{} ({}) [{}] — {status}", p.emulator.id, p.emulator.name, p.emulator.binary);
        println!("    platforms: {}", if plats.is_empty() { "(none by default)".to_owned() } else { plats.join(", ") });
        if status == "missing on PATH" {
            if let Some(hint) = launch::install_hint(&p.emulator.id) {
                println!("    install   : {hint}  (run it yourself; rosadeck never installs)");
            } else {
                println!("    install   : unknown package for adapter '{}'", p.emulator.adapter);
            }
        }
    }
    0
}

/// `rosadeck emulator show <id>` (read-only).
fn cmd_emulator_show(id: &str, json: bool) -> i32 {
    use rosadeck_emulator_core::resolve_in_path;
    use rosadeck_profiles::find_emulator;
    let profiles = launch::load_emulator_profiles();
    let Some(p) = find_emulator(&profiles, id) else {
        eprintln!("rosadeck: unknown emulator profile: {id}");
        return 11;
    };
    if json {
        println!("{}", serde_json::to_string_pretty(p).unwrap_or_default());
        return 0;
    }
    println!("{} ({})", p.emulator.id, p.emulator.name);
    println!("adapter : {}", p.emulator.adapter);
    println!("binary  : {} ({})", p.emulator.binary, match resolve_in_path(&p.emulator.binary) {
        Some(path) => format!("found: {}", path.display()),
        None => "missing on PATH".into(),
    });
    if let Some(l) = &p.launch {
        if !l.extra_args.is_empty() {
            println!("args    : {}", l.extra_args.join(" "));
        }
        if let Some(c) = &l.config_path {
            println!("config  : {c} (referenced, never written)");
        }
    }
    if let Some(g) = &p.graphics {
        println!("vsync   : {}", g.vsync.map(|v| if v { "ON" } else { "OFF" }.to_owned()).unwrap_or("(unset)".into()));
    }
    println!("shader  : {}", p.shader.as_ref().and_then(|s| s.preset.clone()).unwrap_or("disabled".into()));
    0
}

/// `rosadeck profiles emulators`: launch bindings (read-only).
fn cmd_profiles_emulators(json: bool) -> i32 {
    let launches = launch::load_launch_profiles();
    if json {
        println!("{}", serde_json::to_string_pretty(&launches).unwrap_or_default());
        return 0;
    }
    if launches.is_empty() {
        println!("no launch bindings (~/.config/rosadeck/launchers/)");
        return 0;
    }
    for l in &launches {
        println!("{}: display={} emulator={}", l.launch.id, l.launch.display_profile, l.launch.emulator_profile);
    }
    0
}

/// Parse `--policy` (F1 names; `manual` needs `--mode`).
fn parse_policy(s: &str, mode: &Option<display_core::Mode>) -> Result<ModePolicy, String> {
    match s {
        "max-resolution" => Ok(ModePolicy::MaxResolution),
        "max-refresh" => Ok(ModePolicy::MaxRefresh),
        "balanced" => Ok(ModePolicy::Balanced),
        "match-internal" => Ok(ModePolicy::MatchInternal),
        "match-external" => Ok(ModePolicy::MatchExternal),
        "manual" => mode.map(ModePolicy::Manual).ok_or_else(|| "manual policy needs --mode WxH@Hz".to_owned()),
        other => Err(format!("unknown policy: {other}")),
    }
}

fn parse_pos(s: &str) -> Result<ExtendPosition, String> {
    match s {
        "right" => Ok(ExtendPosition::Right),
        "left" => Ok(ExtendPosition::Left),
        "up" => Ok(ExtendPosition::Up),
        "down" => Ok(ExtendPosition::Down),
        other => Err(format!("unknown position: {other} (right|left|up|down)")),
    }
}

pub(crate) fn confirm(prompt: &str) -> bool {
    use std::io::Write;
    eprint!("{prompt} [y/N] ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).is_ok() && line.trim().eq_ignore_ascii_case("y")
}

fn print_plan(kind: PlanKind, plan: &rosadeck_planner::DisplayPlan) {
    println!("Plan ({kind:?}):");
    for (i, s) in plan.steps.iter().enumerate() {
        println!("  {}. keyword monitor \"{}\"", i + 1, s.to_keyword_arg());
    }
    println!("Expect:");
    for e in &plan.expect {
        println!(
            "  {} {} {:?}",
            e.id.0,
            if e.disabled { "disabled".into() } else { e.mode.map(|m| m.to_string()).unwrap_or("(no mode)".into()) },
            e.mirror,
        );
    }
}

/// Build planner input from a snapshot + live unified view (shared by
/// display commands and the launch pipeline: one construction, no drift).
pub(crate) fn planning_input_from(
    snapshot: &rosadeck_backend_hyprland::Snapshot,
    live: &[rosadeck_backend_hyprland::UnifiedDisplay],
) -> rosadeck_planner::PlanningInput {
    let mut modes_map = std::collections::HashMap::new();
    for d in live {
        let ms: Vec<display_core::Mode> = d
            .modes
            .iter()
            .filter(|m| m.sources.contains(&display_core::ModeSource::Hyprland))
            .map(|m| m.mode)
            .collect();
        modes_map.insert(d.connector.0.clone(), ms);
    }
    let mut outputs: Vec<rosadeck_planner::PlanningOutput> = snapshot
        .outputs
        .iter()
        .map(|o| rosadeck_planner::PlanningOutput {
            id: o.state.id.clone(),
            connected: live.iter().any(|d| d.connector.0 == o.state.id.0 && d.drm_connected),
            enabled: !o.state.disabled,
            current: o.state.mode,
            position: o.state.position,
            scale: o.state.scale.0,
            transform: o.transform,
        })
        .collect();
    // Connected-but-compositor-invisible outputs (e.g. a fresh HDMI) are
    // planning candidates: connected, disabled, no current mode.
    for d in live {
        if !outputs.iter().any(|o| o.id.0 == d.connector.0) && d.drm_connected {
            outputs.push(rosadeck_planner::PlanningOutput {
                id: display_core::OutputId(d.connector.0.clone()),
                connected: true,
                enabled: false,
                current: None,
                position: display_core::Position { x: 0, y: 0 },
                scale: 1.0,
                transform: 0,
            });
        }
    }
    rosadeck_planner::PlanningInput { outputs, modes: modes_map }
}

/// Shared mutating flow: snapshot → plan → (dry-run | confirm → apply → verify → restore).
/// Exit codes: 0 ok, 2 usage/backend/abort, 10 NO_COMMON_MODE, 11 INVALID_PLAN,
/// 12/13/14 apply/verify/restore, 15 NO_EXTERNAL_OUTPUT (+NO_INTERNAL_OUTPUT),
/// 16 WOULD_DISABLE_LAST_OUTPUT.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_mutating(
    kind: PlanKind,
    pos: Option<String>,
    policy: String,
    mode: Option<String>,
    on: Option<String>,
    json: bool,
    dry_run: bool,
    yes: bool,
) -> i32 {
    let requested = match mode {
        Some(s) => match parse_requested_mode(&s) {
            Some(m) => Some(m),
            None => {
                eprintln!("rosadeck: INVALID_PLAN: bad --mode {s}");
                return 11;
            }
        },
        None => None,
    };
    let mut policy = match parse_policy(&policy, &requested) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("rosadeck: INVALID_PLAN: {e}");
            return 11;
        }
    };
    // --mode without explicit manual policy implies manual.
    if requested.is_some() && !matches!(policy, ModePolicy::Manual(_)) {
        policy = ModePolicy::Manual(requested.unwrap());
    }
    let position = match pos {
        Some(p) => match parse_pos(&p) {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("rosadeck: INVALID_PLAN: {e}");
                return 11;
            }
        },
        None => None,
    };
    let request = DisplayRequest {
        kind,
        target: on.map(display_core::OutputId),
        policy,
        requested_mode: requested,
        position,
    };
    // In-memory snapshot for planning (dry-run writes nothing).
    let snapshot = match take_snapshot() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("rosadeck: backend: {e}");
            return 2;
        }
    };
    let live = match unify(Path::new(DEFAULT_SYSFS)) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("rosadeck: backend: {e}");
            return 2;
        }
    };
    let input = planning_input_from(&snapshot, &live);
    let plan = match plan(&request, &input) {
        Ok(p) => p,
        Err(e) => {
            let code = match e {
                rosadeck_planner::PlanError::NoCommonMode => 10,
                rosadeck_planner::PlanError::NoExternalOutput => 15,
                rosadeck_planner::PlanError::NoInternalOutput => 15,
                rosadeck_planner::PlanError::WouldDisableLastOutput => 16,
                _ => 11,
            };
            eprintln!("rosadeck: {e}");
            return code;
        }
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&plan).unwrap_or_else(|_| "{}".into()));
    } else {
        print_plan(kind, &plan);
    }
    if dry_run {
        println!("Result: VALID PLAN (dry-run, nothing applied)");
        return 0;
    }
    if !yes && !confirm(&format!("Apply {kind:?} plan?")) {
        eprintln!("rosadeck: aborted (needs --yes)");
        return 2;
    }
    // Persist the snapshot BEFORE mutating (structural guarantee: apply only
    // happens with a saved snapshot in hand).
    if let Err(e) = save_snapshot(&state_dir(), &snapshot) {
        eprintln!("rosadeck: cannot persist snapshot, aborting: {e}");
        return 2;
    }
    let mut runner = rosadeck_backend_hyprland::HyprctlRunner;
    let read = || unify(Path::new(DEFAULT_SYSFS));
    let connected_now = || connected_connector_names(Path::new(DEFAULT_SYSFS));
    match run_transition(&snapshot, &plan, &mut runner, &read, &connected_now) {
        Ok(_) => 0,
        Err(e) => {
            let code = exit_code_for(&e);
            eprintln!("rosadeck: {e}");
            code
        }
    }
}

/// Restore a saved snapshot (default: last). Same confirm/dry-run discipline.
pub(crate) fn run_restore(json: bool, dry_run: bool, yes: bool, from: Option<String>) -> i32 {
    let dir = state_dir();
    let snapshot = match from {
        Some(p) => match std::fs::read_to_string(&p) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("rosadeck: INVALID snapshot: {e}");
                    return 11;
                }
            },
            Err(e) => {
                eprintln!("rosadeck: cannot read {p}: {e}");
                return 2;
            }
        },
        None => match load_last_snapshot(&dir) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("rosadeck: {e}");
                return 11;
            }
        },
    };
    let restore = build_restore_plan(&snapshot, &connected_connector_names(Path::new(DEFAULT_SYSFS)));
    if json {
        println!("{}", serde_json::to_string_pretty(&restore).unwrap_or_else(|_| "{}".into()));
    } else {
        print_plan(PlanKind::Extend, &restore);
    }
    if dry_run {
        println!("Result: VALID PLAN (dry-run, nothing applied)");
        return 0;
    }
    if !yes && !confirm("Restore snapshot?") {
        eprintln!("rosadeck: aborted (needs --yes)");
        return 2;
    }
    let mut runner = rosadeck_backend_hyprland::HyprctlRunner;
    let read = || unify(Path::new(DEFAULT_SYSFS));
    let mut log = vec![];
    if let Err(e) = apply_plan(&restore, &mut runner, &mut log) {
        eprintln!("rosadeck: {e}");
        return exit_code_for(&TransitionError::ApplyFailed(e.to_string()));
    }
    match verify_plan(&restore, &read, &mut log) {
        Ok(()) => 0,
        Err(e) => {
            let code = exit_code_for(&e);
            eprintln!("rosadeck: {e}");
            code
        }
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::which;
    use std::os::unix::fs::PermissionsExt;

    /// El reparto de `rosadeck` a secas depende de encontrar el navegador, así
    /// que `which` tiene que distinguir un ejecutable de un fichero cualquiera.
    #[test]
    fn which_only_accepts_executables() {
        let dir = std::env::temp_dir().join("rosadeck-which-test");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("rosadeck-library-fake");
        std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
        let antes = std::env::var_os("PATH");
        std::env::set_var("PATH", &dir);
        assert_eq!(which("rosadeck-library-fake"), None, "sin permiso de ejecución no vale");
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(which("rosadeck-library-fake"), Some(exe.clone()), "con permiso, sí");
        assert_eq!(which("no-existe-este"), None);
        match antes {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_uses_backend_only() {
        // Architectural boundary: no direct socket/sysfs/EDID access in the CLI.
        // Only the production code is checked (this test module itself names
        // the banned strings, so it is excluded from the scan).
        // Allowed CLI file IO: user-given snapshot file (--from), own config
        // dirs (~/.config/rosadeck), overlay protocol files.
        // Forbidden: sockets, sysfs walks, EDID parsing, process spawn.
        // (file, additional bans): session.rs may spawn kitty (overlay
        // presentation, like the daemon) but never hyprctl/sockets/EDID.
        // El traspaso al navegador (`run_browser`) es un `execv`, no un spawn:
        // reemplaza el proceso, así que el CLI no tiene ningún hijo que
        // gestionar. Si algún día se cambia por `Command::new`, el mismo test
        // lo canta.
        assert!(include_str!("main.rs").split("#[cfg(test)]").next().unwrap_or_default().contains("libc::execv"));
        // (file, allowed): everything else in the base list is banned.
        // launch.rs/library.rs walk own config/ROM dirs (never sysfs —
        // covered by the "/sys/class/drm" ban). session.rs may spawn kitty
        // (overlay presentation, like the daemon) but never hyprctl.
        for (file, allowed) in [
            (include_str!("main.rs"), vec![]),
            (include_str!("launch.rs"), vec!["read_dir"]),
            (include_str!("library.rs"), vec!["read_dir"]),
            (include_str!("session.rs"), vec!["Command::new"]),
        ] {
            let prod = file.split("#[cfg(test)]").next().unwrap_or(file);
            let base = [
                "UnixStream",
                "/sys/class/drm",
                "HYPRLAND_INSTANCE_SIGNATURE",
                ".socket.sock",
                "parse_edid",
                "read_dir",
                "Command::new",
            ];
            for b in base {
                if allowed.contains(&b) {
                    continue;
                }
                assert!(!prod.contains(b), "CLI must not contain {b}");
            }
            if prod.contains("fn open_overlay") {
                assert!(!prod.contains("Command::new(\"hyprctl\")"));
            }
        }
    }

    #[test]
    fn parses_all_subcommands() {
        use clap::Parser;
        assert!(matches!(Cli::try_parse_from(["rosadeck", "status"]).unwrap().cmd, Cmd::Status { json: false }));
        assert!(matches!(
            Cli::try_parse_from(["rosadeck", "displays", "--json"]).unwrap().cmd,
            Cmd::Displays { json: true }
        ));
        assert!(matches!(
            Cli::try_parse_from(["rosadeck", "modes", "eDP-1"]).unwrap().cmd,
            Cmd::Modes { output, .. } if output == "eDP-1"
        ));
        assert!(Cli::try_parse_from(["rosadeck", "modes"]).is_err());
        // F3 mutators exist with dry-run/yes discipline.
        assert!(matches!(
            Cli::try_parse_from(["rosadeck", "duplicate", "--dry-run"]).unwrap().cmd,
            Cmd::Duplicate { dry_run: true, yes: false, .. }
        ));
        assert!(matches!(
            Cli::try_parse_from(["rosadeck", "external", "--on", "HDMI-A-1", "--yes"]).unwrap().cmd,
            Cmd::External { yes: true, .. }
        ));
        assert!(matches!(
            Cli::try_parse_from(["rosadeck", "extend", "--pos", "left"]).unwrap().cmd,
            Cmd::Extend { .. }
        ));
        assert!(matches!(
            Cli::try_parse_from(["rosadeck", "restore", "--dry-run"]).unwrap().cmd,
            Cmd::Restore { dry_run: true, .. }
        ));
        // F5 launcher commands.
        assert!(matches!(
            Cli::try_parse_from(["rosadeck", "emulators"]).unwrap().cmd,
            Cmd::Emulators { .. }
        ));
        assert!(matches!(
            Cli::try_parse_from(["rosadeck", "emulator", "show", "dolphin"]).unwrap().cmd,
            Cmd::Emulator { .. }
        ));
        assert!(matches!(
            Cli::try_parse_from(["rosadeck", "profiles", "emulators"]).unwrap().cmd,
            Cmd::Profiles { .. }
        ));
        assert!(matches!(
            Cli::try_parse_from(["rosadeck", "launch", "--emulator", "dolphin", "--game", "/g/x.wbfs", "--dry-run"])
                .unwrap()
                .cmd,
            Cmd::Launch { dry_run: true, .. }
        ));
        assert!(matches!(
            Cli::try_parse_from(["rosadeck", "menu"]).unwrap().cmd,
            Cmd::Menu { gaming: false, .. }
        ));
        assert!(matches!(
            Cli::try_parse_from(["rosadeck", "menu", "--gaming"]).unwrap().cmd,
            Cmd::Menu { gaming: true, .. }
        ));
        assert!(matches!(
            Cli::try_parse_from(["rosadeck", "menu", "--wofi"]).unwrap().cmd,
            Cmd::Menu { wofi: true, .. }
        ));
        assert!(matches!(
            Cli::try_parse_from(["rosadeck", "gaming", "--dry-run"]).unwrap().cmd,
            Cmd::Gaming { dry_run: true, .. }
        ));
        assert!(matches!(
            Cli::try_parse_from(["rosadeck", "session-restore", "--dry-run"]).unwrap().cmd,
            Cmd::SessionRestore { dry_run: true, .. }
        ));
        assert!(matches!(
            Cli::try_parse_from(["rosadeck", "library", "--platform", "wii"]).unwrap().cmd,
            Cmd::Library { .. }
        ));
        assert!(matches!(
            Cli::try_parse_from(["rosadeck", "play", "zelda", "--dry-run"]).unwrap().cmd,
            Cmd::Play { dry_run: true, .. }
        ));
        assert!(matches!(
            Cli::try_parse_from(["rosadeck", "art", "--platform", "wii"]).unwrap().cmd,
            Cmd::Art { platform: Some(_), json: false }
        ));
    }
}
