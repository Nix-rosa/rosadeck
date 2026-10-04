//! `rosadeck-library`: a cover-art retro game library for the terminal.
//!
//! The browser is a cover grid, not a list: real artwork rendered as half
//! blocks, favorites and play counts kept locally, and Enter delegating to the
//! single launch path (`rosadeck play --id <id> --yes`). Everything the frame
//! needs is pure (`browser`, `layout`, `cover`, `frame`); this file is only the
//! terminal runtime.
//!
//! Flags: `--dump` (one frame, headless), `--no-color`/`--ascii` (plain
//! terminal), `--roms <dir>` (extra ROM roots).

mod browser;
mod cover;
mod emulators;
mod frame;
mod icons;
mod images;
mod layout;
mod pywal;
mod theme;

use browser::{handle_key, Browser, BrowserKey, Key};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use frame::{build, Frame, FrameInput};
use images::Layer;
use rosadeck_game_library::{default_roots, scan_roots, GameEntry, Platform, StatsDb};
use layout::Layout;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use theme::Theme;

/// One status line explaining why a game did not launch.
///
/// Prefers the CLI's own stderr (`rosadeck: PREPARE_FAILED: binary not found:
/// retroarch`) and appends the package hint from the profiles we loaded, so the
/// user sees both what failed and what to do about it.
fn launch_error(game: &GameEntry, list: &[emulators::EmulatorStatus], stderr: Option<&[u8]>) -> String {
    let why = stderr
        .map(|b| String::from_utf8_lossy(b).to_string())
        .unwrap_or_default();
    let line = why.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_owned();
    // Only offer a package when *our* profile data agrees the binary is
    // missing; otherwise the CLI said something else and adding pacman noise
    // would be a guess.
    let hint = match emulators::availability(list, game.platform.default_emulator()) {
        emulators::Availability::Missing { hint: Some(h), .. } => Some(format!(" · {h}")),
        _ => None,
    };
    if line.is_empty() {
        format!("{} did not start{}", game.title, hint.unwrap_or_default())
    } else {
        format!("{line}{}", hint.unwrap_or_default())
    }
}

/// Directories that can hold artwork for `games`, deduped.
fn art_dirs(games: &[GameEntry]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for g in games {
        for d in rosadeck_game_library::candidate_dirs(&g.path) {
            if !dirs.contains(&d) {
                dirs.push(d);
            }
        }
    }
    dirs
}

/// Cover files that currently exist for `games` (the ones worth watching).
///
/// Resolved once per rescan, not per poll: the watched set must not grow while
/// the user pans the carousel, or every new game would look like new artwork.
fn art_files(games: &[GameEntry]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for g in games {
        if let Some(a) = rosadeck_game_library::resolve_artwork(&g.path) {
            out.push(a.cover);
        }
    }
    out
}

/// Fingerprint of everything artwork resolution depends on.
///
/// Two things are watched: the mtime of every candidate directory (a cover
/// added or deleted changes it) and the mtime/size of every cover file in use
/// (a cover *replaced in place* does not move any directory mtime). Without
/// this, dropping a new cover while the browser was open left the generated
/// placeholder on screen until the user pressed `r`.
fn art_stamp(dirs: &[PathBuf], files: &[PathBuf]) -> Vec<(u64, u128, u64)> {
    let mut out: Vec<(u64, u128, u64)> = dirs
        .iter()
        .map(|d| {
            let m = std::fs::metadata(d).ok();
            (0, stamp_of(&m), m.map(|m| m.len()).unwrap_or(0))
        })
        .collect();
    for f in files {
        let m = std::fs::metadata(f).ok();
        out.push((1, stamp_of(&m), m.map(|m| m.len()).unwrap_or(0)));
    }
    out
}

/// Metadata as one comparable number (seconds and nanoseconds).
fn stamp_of(m: &Option<std::fs::Metadata>) -> u128 {
    m.as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// `~/.config/rosadeck/emulators` — read-only, so the browser can say
/// whether a game's emulator is actually installed before Enter is pressed.
fn emulator_dir() -> PathBuf {
    let base = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()));
    rosadeck_game_library::dir_with_legacy(base.join(".config/rosadeck/emulators"), base.join(".config/hyprgame/emulators"))
}

/// Repintar la pantalla entera es un `[2J`, y el spec del protocolo de gráficos
/// dice que `[2J` limpia también las imágenes (los datos, no sólo las
/// colocaciones). Sin esto, el `a=p` de después responde `ENOENT` y las portadas
/// se quedan apagadas: comprobado en kitty real con píxeles y con su respuesta
/// (v26).
fn repaint_all(screen: &mut rosadeck_tui_frame::Screen, layer: &mut images::Layer) {
    screen.invalidate();
    layer.data_lost();
}

fn rom_roots(extra: Option<&str>) -> Vec<PathBuf> {
    let mut roots = default_roots();
    if let Some(dir) = extra {
        roots.insert(0, PathBuf::from(dir));
    }
    roots
}

/// Add or remove a ROM root typed with `d`, then save and rescan.
///
/// Typing a path that is already configured removes it: the same gesture that
/// adds one takes it away, and the status line says which happened. A path that
/// does not exist is accepted (an unplugged drive is a legitimate entry) but
/// said out loud, because `default_roots` will skip it until it appears.
fn set_root(app: &mut App, raw: &str) {
    let path = rosadeck_game_library::expand_home(raw);
    let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    let mut configured = app.configured.clone();
    let before = configured.len();
    configured.retain(|p| p != &canonical && p != &path);
    if configured.len() == before {
        configured.push(canonical.clone());
    }
    match rosadeck_game_library::save_configured_roots(&configured) {
        Ok(()) => {}
        Err(e) => {
            app.status = format!("no pude guardar {}: {e}", rosadeck_game_library::roots_config_path().display());
            return;
        }
    }
    app.configured = configured.clone();
    app.roots = rom_roots(app.extra_roms.as_deref());
    app.games = scan_roots(&app.roots);
    app.browser.platform = None;
    app.browser.cursor = 0;
    app.invalidate_art();
    let exists = canonical.is_dir();
    app.status = format!(
        "{} {} · {} juegos{}",
        if configured.len() < before { "quitado" } else { "añadido" },
        canonical.display(),
        app.games.len(),
        if exists { "" } else { " · la carpeta no existe todavía" },
    );
}

fn stats_path() -> PathBuf {
    state_dir().join("library.json")
}

/// `$XDG_STATE_HOME/rosadeck` (or `~/.local/state/rosadeck`), where the stats
/// and the PNG copies of non-PNG covers live.
fn state_dir() -> PathBuf {
    let base = std::env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())).join(".local/state"));
    rosadeck_game_library::dir_with_legacy(base.join("rosadeck"), base.join("hyprgame"))
}

fn map_event(ev: Event, typing: bool) -> Option<Key> {
    let Event::Key(k) = ev else { return None };
    // kitty reports key *releases* (progressive enhancement): without this
    // filter every keystroke fires twice (Down moves two rows).
    if k.kind == KeyEventKind::Release {
        return None;
    }
    match (k.code, typing) {
        // While an editor is open every character is text, `d`, `/`, `q` and the
        // digits included: they are characters of the path, not shortcuts.
        (KeyCode::Char(c), true) => Some(Key::Type(c)),
        (KeyCode::Esc, _) => Some(Key::Esc),
        (KeyCode::Enter, _) => Some(Key::Enter),
        (KeyCode::Up, _) => Some(Key::Up),
        (KeyCode::Down, _) => Some(Key::Down),
        (KeyCode::Left, _) => Some(Key::Left),
        (KeyCode::Right, _) => Some(Key::Right),
        (KeyCode::PageUp, _) => Some(Key::PageUp),
        (KeyCode::PageDown, _) => Some(Key::PageDown),
        (KeyCode::Home, _) => Some(Key::Home),
        (KeyCode::End, _) => Some(Key::End),
        (KeyCode::Char('f') | KeyCode::Char('F'), false) => Some(Key::Favorite),
        // `d` configures the ROM directories. Not while typing a search or a
        // path: there every letter belongs to the text.
        (KeyCode::Char('d') | KeyCode::Char('D'), false) => Some(Key::Dirs),
        (KeyCode::Char('/'), false) => Some(Key::Search),
        (KeyCode::Char('r') | KeyCode::Char('R'), false) if !k.modifiers.contains(KeyModifiers::CONTROL) => Some(Key::Rescan),
        // `w` reloads the pywal palette (run `pywal -i wall.jpg` in another terminal).
        (KeyCode::Char('w') | KeyCode::Char('W'), false) => Some(Key::Theme),
        (KeyCode::Backspace, _) => Some(Key::Backspace),
        _ => None,
    }
}

fn platform_filter(key: char) -> Option<Option<Platform>> {
    match key {
        '1' => Some(Some(Platform::Snes)),
        '2' => Some(Some(Platform::N64)),
        '3' => Some(Some(Platform::GameCube)),
        '4' => Some(Some(Platform::Wii)),
        '5' => Some(Some(Platform::Nintendo3DS)),
        '0' | '.' => Some(None),
        _ => None,
    }
}

fn sibling_cli() -> String {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("rosadeck");
            if p.exists() {
                return p.to_string_lossy().into_owned();
            }
        }
    }
    "rosadeck".to_owned()
}

#[allow(dead_code)] // el CLI mide el tiempo jugado; aquí ya no se necesita
fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Runtime state shared by the loop.
struct App {
    /// What the legend says about how the covers travel (see `FrameInput`).
    art_mode: String,
    /// `--roms <dir>`: a root that is not saved, only for this run.
    extra_roms: Option<String>,
    /// Every root the user configured with `d`, including the ones that are not
    /// there right now (a disk is unplugged, a mount is down): the window has to
    /// list them so they can be removed, which the scan roots cannot do.
    configured: Vec<PathBuf>,
    games: Vec<GameEntry>,
    browser: Browser,
    db: StatsDb,
    status: String,
    theme: Theme,
    cache: cover::CoverCache,
    roots: Vec<PathBuf>,
    w: u16,
    h: u16,
    /// pywal `colors.json` mtime we last loaded (0 = built-in).
    wal_stamp: u64,
    /// Terminal can display real cover images (kitty graphics).
    images_ok: bool,
    /// Emulator profiles and whether their binary exists (detail pane).
    emulators: Vec<emulators::EmulatorStatus>,
    /// Directories watched for artwork changes.
    art_dirs: Vec<PathBuf>,
    /// Cover files watched for changes (resolved at rescan, not per poll).
    art_files: Vec<PathBuf>,
    /// Last artwork fingerprint, to notice covers appearing or changing.
    art_stamp: Vec<(u64, u128, u64)>,
}

impl App {
    fn layout(&self) -> Layout {
        Layout::compute(
            rosadeck_tui_frame::safe_width(self.w),
            self.h as usize,
            self.browser.view(&self.games, &self.db.stats.favorites).len(),
        )
    }

    fn frame(&mut self) -> Frame {
        let layout = self.layout();
        self.browser.set_geometry(layout.cols, 1); // carousel: one row
        let played: HashMap<String, u64> =
            self.db.stats.plays.iter().map(|(k, v)| (k.clone(), v.played_secs)).collect();
        build(
            &FrameInput {
                browser: &self.browser,
                games: &self.games,
                favorites: &self.db.stats.favorites,
                played: &played,
                emulators: &self.emulators,
                roots: &self.configured,
                status: &self.status,
                art_mode: &self.art_mode,
            },
            layout,
            &self.theme,
            &mut self.cache,
            self.images_ok,
        )
    }

    /// Reset the memoized artwork (resize, rescan, theme reload).
    fn invalidate_art(&mut self) {
        self.cache = cover::CoverCache::new();
        self.art_dirs = art_dirs(&self.games);
        self.art_files = art_files(&self.games);
        self.art_stamp = art_stamp(&self.art_dirs, &self.art_files);
    }

    /// True when artwork appeared, vanished or was replaced since last time.
    fn artwork_changed(&mut self) -> bool {
        let now = art_stamp(&self.art_dirs, &self.art_files);
        if now == self.art_stamp {
            return false;
        }
        self.art_stamp = now;
        true
    }
}

/// Parsed command line.
struct Args {
    /// Headless: print one frame and exit.
    dump: bool,
    /// No styling and no artwork.
    plain: bool,
    /// Extra ROM root.
    roms: Option<String>,
    /// Palette policy.
    theme: pywal::Mode,
    /// Never use the kitty graphics protocol (half-block covers only).
    no_images: bool,
}

fn parse_args() -> Args {
    parse_args_with(std::env::args().skip(1).collect::<Vec<String>>())
}

/// Flag parsing over an explicit argv (testable).
fn parse_args_with(argv: impl IntoIterator<Item = String>) -> Args {
    let argv: Vec<String> = argv.into_iter().collect();
    let mut dump = false;
    let mut plain = false;
    let mut roms = None;
    let mut theme = None;
    let mut no_images = false;
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--dump" => dump = true,
            "--no-color" | "--ascii" => plain = true,
            "--roms" => {
                i += 1;
                roms = argv.get(i).cloned();
            }
            "--theme" => {
                i += 1;
                theme = argv.get(i).cloned();
            }
            "--no-images" | "--ascii-art" => no_images = true,
            _ => {}
        }
        i += 1;
    }
    Args { dump, plain, roms, theme: theme_mode(theme), no_images }
}

/// `--theme <mode>` or `$ROSADECK_THEME`.
fn theme_mode(arg: Option<String>) -> pywal::Mode {
    let raw = arg.unwrap_or_else(|| std::env::var("ROSADECK_THEME").unwrap_or_else(|_| "auto".into()));
    match raw.as_str() {
        "pywal" => pywal::Mode::Pywal,
        "system" | "builtin" | "none" => pywal::Mode::System,
        _ => pywal::Mode::Auto,
    }
}

/// stdout that remembers how much went through it, so a session can say how
/// many bytes it cost instead of guessing.
struct Counter {
    inner: std::io::Stdout,
    bytes: u64,
}

impl Counter {
    fn new() -> Self {
        Self { inner: std::io::stdout(), bytes: 0 }
    }
}

impl std::io::Write for Counter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.bytes += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// What the last session actually did, written to the state directory.
///
/// "Se siente lento" is not measurable from outside, and guessing wrong wastes
/// days: which binary ran (a debug build is 10x slower), whether the terminal
/// can read our files, how many covers went as bytes or as paths, and how long
/// the app itself took per frame.
#[derive(Default)]
struct Diag {
    frames: u32,
    sends: u32,
    places: u32,
    prepared: u32,
    ticks: u32,
    bytes: u64,
    build_ms: f64,
    images_ms: f64,
    prep_ms: f64,
}

fn write_diag(probe: bool, images_ok: bool, diag: &Diag, out: &Counter) {
    let report = serde_json::json!({
        "binary": std::env::current_exe().ok().map(|p| p.display().to_string()),
        "debug_build": cfg!(debug_assertions),
        "term": std::env::var("TERM").ok(),
        "kitty_window": std::env::var("KITTY_WINDOW_ID").is_ok(),
        "images": images_ok,
        "probe_local_files": probe,
        "frames": diag.frames,
        "covers_sent": diag.sends,
        "covers_prepared": diag.prepared,
        "placements": diag.places,
        "bytes_written": diag.bytes,
        "max_frame_build_ms": (diag.build_ms * 100.0).round() / 100.0,
        "max_images_ms": (diag.images_ms * 100.0).round() / 100.0,
        "max_prepare_ms": (diag.prep_ms * 100.0).round() / 100.0,
        "bytes_total": out.bytes,
    });
    let path = state_dir().join("last-session.json");
    if let Ok(mut f) = std::fs::File::create(&path) {
        use std::io::Write as _;
        let _ = writeln!(f, "{report}");
    }
}

/// A frame for a *different* cursor, used only to prepare artwork ahead of
/// time. Nothing it draws is shown, and it must not move the real selection.
fn frame_for(app: &mut App, cursor: usize) -> Frame {
    let saved = app.browser.cursor;
    app.browser.cursor = cursor.min(app.browser.view(&app.games, &app.db.stats.favorites).len().saturating_sub(1));
    let frame = app.frame();
    app.browser.cursor = saved;
    frame
}

/// What the legend tells the user about the artwork path, and whether this is
/// an optimised build: "se siente lento" is one of these two things, and the
/// interface should say which instead of the user having to guess.
fn art_mode_label(local_files: bool, images_ok: bool) -> String {
    let mode = match (images_ok, local_files) {
        (true, true) => "portadas: rutas",
        (true, false) => "portadas: bytes",
        (false, _) => "portadas: bloques",
    };
    if cfg!(debug_assertions) {
        format!("{mode} · BINARIO DEBUG (lento)")
    } else {
        mode.to_owned()
    }
}

fn main() -> std::process::ExitCode {
    match run() {
        0 => std::process::ExitCode::SUCCESS,
        code => std::process::ExitCode::from(code.clamp(1, 255) as u8),
    }
}

fn run() -> i32 {
    let args = parse_args();
    let detected = Theme::from_env();
    let (mut theme, mut theme_note) = pywal::resolve(args.theme, detected.depth, detected.art);
    if args.plain {
        theme = Theme::plain();
        theme_note = None;
    }
    let roots = rom_roots(args.roms.as_deref());
    let games = scan_roots(&roots);
    let (db, corrupt) = StatsDb::load(&stats_path());
    let mut status = corrupt.unwrap_or_default();
    if games.is_empty() {
        status = format!("no ROMs under {}", roots.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "));
    }
    if let Some(note) = theme_note {
        if !status.is_empty() {
            status = format!("{status} · {note}");
        } else {
            status = note;
        }
    }

    if args.dump {
        // ROSADECK_DUMP_W/H override the frame size (default 120x40).
        let w: usize = std::env::var("ROSADECK_DUMP_W").ok().and_then(|v| v.parse().ok()).unwrap_or(120);
        let h: usize = std::env::var("ROSADECK_DUMP_H").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
        let mut app = App {
            art_mode: "bloques (--dump)".to_owned(),
            extra_roms: args.roms.clone(),
            configured: rosadeck_game_library::configured_roots(),
            games,
            browser: Browser::new(),
            db,
            status: status.clone(),
            theme,
            cache: cover::CoverCache::new(),
            roots,
            w: w as u16,
            h: h as u16,
            wal_stamp: 0,
            images_ok: false,
            emulators: emulators::load_from(&emulator_dir()),
            art_dirs: Vec::new(),
            art_files: Vec::new(),
            art_stamp: Vec::new(),
        };
        let frame = app.frame();
        print!("{}", frame.text);
        return 0;
    }

    // Sin terminal no hay navegador: se dice con palabras, no con un panic.
    // `rosadeck` a secas es lo que escribe la gente, y una redirección
    // (`rosadeck | cat`, `rosadeck < /dev/null`) es un error normal de uso.
    if let Err(e) = enable_raw_mode() {
        eprintln!(
            "rosadeck-library: necesito una terminal (raw mode: {e}).\n  \
             ejecútalo en una terminal de verdad, o usa `rosadeck --dump` para ver un frame."
        );
        return 1;
    }
    let mut out = Counter::new();
    out.execute(EnterAlternateScreen).ok();
    out.execute(Hide).ok();
    let sync = rosadeck_tui_frame::supports_synchronized_output(&std::env::var("TERM").unwrap_or_default())
        && std::env::var("ROSADECK_NO_SYNC").is_err();
    let mut screen = rosadeck_tui_frame::Screen::new();
    let mut diag = Diag::default();
    let mut build_ms;
    // Can the terminal read our files? Ask, before anything is drawn, with a
    // real file: if it can, a cover travels as a ~200-byte path instead of a
    // megabyte of base64, which is the difference between a carousel that
    // follows the arrows and one that lags a keypress behind.
    let local_files = images::probe_local_files(&state_dir());
    let mut layer = Layer::with_local_files(local_files, Some(&state_dir()));
    // Real cover images when the terminal can show them, half blocks otherwise.
    let images_ok = images::available(
        &std::env::var("TERM").unwrap_or_default(),
        std::env::var("KITTY_WINDOW_ID").is_ok(),
        args.no_images || std::env::var("ROSADECK_NO_IMAGES").is_ok(),
    );
    // Written before the first frame too, so the report exists even if the app
    // never gets past startup.
    write_diag(local_files, images_ok, &diag, &out);
    let wal_stamp = pywal::load().map(|w| w.stamp).unwrap_or(0);
    let mut app = App {
        art_mode: art_mode_label(local_files, images_ok),
        extra_roms: args.roms.clone(),
        configured: rosadeck_game_library::configured_roots(),
        games,
        browser: Browser::new(),
        db,
        status,
        theme,
        cache: cover::CoverCache::new(),
        roots,
        w: 80,
        h: 24,
        wal_stamp,
        images_ok,
        emulators: emulators::load_from(&emulator_dir()),
        art_dirs: Vec::new(),
        art_files: Vec::new(),
        art_stamp: Vec::new(),
    };
    // Fill the watched dirs and prime the fingerprint, so the first real change
    // is the one we report (and the first frame already has its artwork).
    app.invalidate_art();
    let code = loop {
        let (w, h) = crossterm::terminal::size().unwrap_or((80, 24));
        if (w, h) != (app.w, app.h) {
            app.w = w;
            app.h = h;
            app.invalidate_art(); // covers resize with the band
            repaint_all(&mut screen, &mut layer);
            layer.invalidate(); // placements are in cells, so they must be redone
        }
        let view_len = app.browser.view(&app.games, &app.db.stats.favorites).len();
        app.browser.clamp(view_len);
        let t_frame = std::time::Instant::now();
        let frame = app.frame();
        build_ms = t_frame.elapsed().as_secs_f64() * 1000.0;
        // One frame, one synchronized-output block, in this order:
        //
        //   1. text (a diff paint, or a full repaint that starts with `2J`), then
        //   2. the covers.
        //
        // The order is the whole trick. A full repaint clears the screen, and
        // kitty drops image placements on `[2J`: placing the covers *first*
        // meant the very first frame — and every resize, theme reload or
        // artwork change — erased the covers it had just placed. They went
        // missing only for that window, while panning (no clear) worked, which
        // is exactly the "some covers never show" symptom.
        // Nothing to do? Then say nothing at all: no empty sync blocks on the
        // 400 ms poll.
        let text_dirty = screen.dirty_rows(&frame.text).is_some();
        let diff = if app.images_ok { layer.diff(&frame.images) } else { layer.diff(&[]) };
        let image_work = !diff.hide.is_empty() || !diff.place.is_empty();
        if text_dirty || image_work {
            if sync {
                out.write_all(rosadeck_tui_frame::SYNC_BEGIN).ok();
            }
            // Diff paint: only the rows that changed are rewritten. A cursor
            // move no longer repaints 58 KB.
            if text_dirty {
                screen.draw(&mut out, &frame.text, app.w, app.h, false).ok();
            }
            if image_work {
                let t_img = std::time::Instant::now();
                let before = out.bytes;
                match layer.write(&mut out, &diff) {
                    Ok(written) => layer.commit(&diff, &written),
                    Err(_) => layer.invalidate(),
                }
                diag.images_ms = diag.images_ms.max(t_img.elapsed().as_secs_f64() * 1000.0);
                diag.frames += 1;
                diag.bytes += out.bytes - before;
                diag.sends += diff.place.iter().filter(|p| p.data_needed).count() as u32;
                diag.places += diff.place.len() as u32;
            }
            if sync {
                out.write_all(rosadeck_tui_frame::SYNC_END).ok();
            }
            // Keep the report current, so it is there even if the app is killed
            // instead of quit.
            diag.ticks += 1;
            if diag.ticks % 5 == 0 {
                diag.build_ms = diag.build_ms.max(build_ms);
                write_diag(local_files, images_ok, &diag, &out);
            }
        }
        out.flush().ok();

        // Poll instead of blocking so a new pywal palette can be picked up
        // without touching the keyboard.
        let key = match event::poll(std::time::Duration::from_millis(400)) {
            Ok(true) => match event::read() {
                Ok(ev) => ev,
                Err(_) => break 2,
            },
            Ok(false) => {
                // Idle. Use it: encode the artwork the *next* position will
                // need, so a keypress only places images the terminal already
                // holds. Never while a key is already waiting — that would
                // move the delay, not remove it.
                if app.images_ok
                    && app.w > 0
                    && crossterm::event::poll(std::time::Duration::from_millis(0)).unwrap_or(false) == false
                {
                    let t_pre = std::time::Instant::now();
                    let before = out.bytes;
                    let next = app.browser.cursor + 1;
                    let probe = frame_for(&mut app, next);
                    // Transmit what the *next* position will need, now, while
                    // nothing is waiting. The keystroke then only has to place
                    // images the terminal already holds.
                    let prefetched = layer.prefetch(&mut out, &probe.images, 3).unwrap_or(0);
                    let made = layer.prepare(&probe.images, 3);
                    if prefetched > 0 || made > 0 {
                        diag.prepared += (prefetched + made) as u32;
                        diag.prep_ms = diag.prep_ms.max(t_pre.elapsed().as_secs_f64() * 1000.0);
                        diag.bytes += out.bytes - before;
                    }
                }
                // Covers dropped in (or replaced) while the browser was open:
                // noticed here instead of waiting for `r`.
                if app.artwork_changed() {
                    app.invalidate_art();
                    layer.invalidate();
                    repaint_all(&mut screen, &mut layer);
                    app.status = "artwork updated".to_owned();
                    continue;
                }
                if pywal::changed_since(app.wal_stamp) && !args.plain {
                    let (t, note) = pywal::resolve(args.theme, app.theme.depth, app.theme.art);
                    app.theme = t;
                    app.wal_stamp = pywal::load().map(|w| w.stamp).unwrap_or(0);
                    app.invalidate_art();
                    layer.invalidate();
                    app.status = note.unwrap_or_else(|| format!("palette: {}", app.theme.source.label()));
                    repaint_all(&mut screen, &mut layer);
                }
                continue;
            }
            Err(_) => break 2,
        };
        // Platform digits, theme reload and quit live outside the pure model.
        // Not while a text editor is open: `1`, `q` and `/` are characters of
        // the path being typed there, not shortcuts.
        let typing = app.browser.searching || app.browser.roots_input;
        let was_roots = app.browser.roots_input;
        if let Event::Key(k) = &key {
            if k.kind != KeyEventKind::Release && !typing {
                if let KeyCode::Char(c) = k.code {
                    if let Some(f) = platform_filter(c) {
                        app.browser.platform = f;
                        app.browser.cursor = 0;
                        continue;
                    }
                    if c == 'q' || c == 'Q' {
                        break 0;
                    }
                }
            }
        }
        let Some(k) = map_event(key, typing) else { continue };
        let view_len = app.browser.view(&app.games, &app.db.stats.favorites).len();
        match handle_key(&mut app.browser, view_len, k) {
            BrowserKey::Continue => {}
            BrowserKey::Quit => break 0,
            BrowserKey::SetRoot(path) => set_root(&mut app, &path),
            BrowserKey::Rescan => {
                app.games = scan_roots(&app.roots);
                app.invalidate_art();
                repaint_all(&mut screen, &mut layer);
                // Covers may have changed on disk: drop them and their data.
                layer.forget(&mut out).ok();
                app.status = format!("rescanned: {} games", app.games.len());
            }
            BrowserKey::Theme => {
                // `w` reloads the pywal palette (and re-paints every cover).
                let (t, note) = pywal::resolve(args.theme, app.theme.depth, app.theme.art);
                app.theme = if args.plain { Theme::plain() } else { t };
                app.wal_stamp = pywal::load().map(|w| w.stamp).unwrap_or(0);
                app.invalidate_art();
                repaint_all(&mut screen, &mut layer);
                layer.invalidate();
                let where_ = if app.images_ok { format!(" · {} covers cached by kitty", layer.transmitted()) } else { String::new() };
                app.status = format!("{}{where_}", note.unwrap_or_else(|| format!("palette: {}", app.theme.source.label())));
            }
            BrowserKey::ToggleFavorite(i) => {
                if let Some(g) = app.browser.view(&app.games, &app.db.stats.favorites).get(i) {
                    let id = g.id.clone();
                    let now_fav = app.db.toggle_favorite(&id);
                    app.status = if now_fav { format!("favorited {}", g.title) } else { format!("unfavorited {}", g.title) };
                    let _ = app.db.save();
                }
            }
            BrowserKey::Play(i) => {
                let Some(g) = app.browser.view(&app.games, &app.db.stats.favorites).get(i).cloned() else { continue };
                // Pre-flight (read-only, no spawn, no mutation): ask the CLI
                // whether it *could* launch. Its answer is the source of truth,
                // so a missing emulator is reported with its own words instead
                // of a silent failure after the screen has already gone dark.
                let cli = sibling_cli();
                let preflight = std::process::Command::new(&cli)
                    .args(["play", "--id", &g.id, "--dry-run"])
                    .output();
                let blocked = match &preflight {
                    Ok(o) if !o.status.success() => {
                        // El path no está en el pie (fuera de serie), así que
                        // un lanzamiento que falla lo dice aquí, donde hace falta.
                        app.status = format!("{} · {}", g.path.display(), launch_error(&g, &app.emulators, Some(&o.stderr)));
                        true
                    }
                    Ok(_) => false,
                    Err(e) => {
                        // Almost always "the CLI is not installed": the browser
                        // looks for `rosadeck` next to itself and then in PATH.
                        // "No such file or directory (os error 2)" is not an
                        // answer a user can act on.
                        app.status = if e.kind() == std::io::ErrorKind::NotFound {
                            format!("no encuentro el CLI rosadeck (busqué {cli}); instálalo o ponlo en el PATH")
                        } else {
                            format!("play failed: {e}")
                        };
                        true
                    }
                };
                if blocked {
                    continue;
                }
                // The emulator owns the screen from here: drop the covers.
                layer.clear(&mut out).ok();
                out.flush().ok();
                out.execute(LeaveAlternateScreen).ok();
                out.execute(Show).ok();
                disable_raw_mode().ok();
                // `--quiet`: the emulator's console output goes to
                // ~/.local/state/rosadeck/emulators.log. Without it, an emulator
                // that writes *after* it exits (Dolphin's SIGTERM handler dumps
                // its argv through Qt) lands on top of this browser when it comes
                // back — the "the UI breaks when I close the game" bug.
                let st = std::process::Command::new(&cli)
                    .args(["play", "--id", &g.id, "--yes", "--quiet"])
                    .status();
                let mut closed = false;
                match st {
                    Ok(s) if s.success() => {
                        app.status = format!("played {}", g.title);
                        closed = true;
                    }
                    Ok(s) => {
                        // 19 means the emulator *did* run and exited non-zero —
                        // that is a closed game, not a failed launch. Saying
                        // "did not start" would be a lie after every normal quit.
                        let code = s.code().map_or(-1, |c| c as i32);
                        app.status = if code == 19 {
                            format!("{} closed (exit {code})", g.title)
                        } else {
                            format!("{} did not start (exit {code})", g.title)
                        };
                        closed = code == 19;
                    }
                    Err(e) => app.status = format!("play failed: {e}"),
                }
                // El CLI es quien mide el tiempo jugado (es quien espera al
                // emulador) y quien lo escribe; aquí sólo se recarga el fichero.
                // Antes los dos contaban y cada partida sumaba dos veces.
                if closed {
                    let (fresh, _) = StatsDb::load(&stats_path());
                    app.db = fresh;
                }
                // The emulator owned the terminal, so nothing about it can be
                // assumed: soft-reset the modes it touched (Dolphin is a Qt app
                // and leaves SGR/cursor state behind), then come back with a
                // *fresh* alternate buffer and repaint every single row.
                out.write_all(b"\x1b[!p").ok(); // DECSTR: soft terminal reset
                out.write_all(b"\x1b[0m").ok(); // and drop any lingering style
                enable_raw_mode().ok();
                out.execute(EnterAlternateScreen).ok();
                out.execute(Hide).ok();
                out.flush().ok();
                // Without this the painter still believes the pre-launch frame
                // is on screen and only rewrites the rows whose text changed:
                // the rest of the UI comes back as whatever the emulator left.
                repaint_all(&mut screen, &mut layer);
                app.games = scan_roots(&app.roots); // ids are stable, paths may change
                app.invalidate_art();
                layer.forget(&mut out).ok();
            }
        }
        // The roots editor is a modal window over the shelf, so the covers leave
        // while it is open: kitty paints images over text, and a text frame
        // cannot survive on top of a transmitted cover (a negative z-index, the
        // only way to put an image under text, is erased by the text itself).
        if was_roots != app.browser.roots_input {
            // Abrir y cerrar la ventana repintan la pantalla entera, así que
            // kitty pierde las imágenes (ver `repaint_all`).
            repaint_all(&mut screen, &mut layer);
        }
        // It also explains itself where the eye already is: the status line says
        // what Enter will do to the path just typed.
        if !was_roots && app.browser.roots_input {
            app.status = format!(
                "d: escribe una ruta y Enter la añade; repetirla la quita · {} configuradas",
                app.configured.len()
            );
        }
    };
    // Remove the covers before leaving the alternate screen.
    layer.clear(&mut out).ok();
    out.flush().ok();
    // What this session cost, on disk, before the screen goes away.
    diag.build_ms = diag.build_ms.max(build_ms);
    write_diag(local_files, images_ok, &diag, &out);
    out.execute(LeaveAlternateScreen).ok();
    out.execute(Show).ok();
    disable_raw_mode().ok();
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_mapping_covers_the_legend() {
        let k = |code, searching| map_event(Event::Key(crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)), searching);
        assert_eq!(k(KeyCode::Up, false), Some(Key::Up));
        assert_eq!(k(KeyCode::Down, false), Some(Key::Down));
        assert_eq!(k(KeyCode::Left, false), Some(Key::Left));
        assert_eq!(k(KeyCode::Right, false), Some(Key::Right));
        assert_eq!(k(KeyCode::PageDown, false), Some(Key::PageDown));
        assert_eq!(k(KeyCode::Enter, false), Some(Key::Enter));
        assert_eq!(k(KeyCode::Char('f'), false), Some(Key::Favorite));
        assert_eq!(k(KeyCode::Char('/'), false), Some(Key::Search));
        assert_eq!(k(KeyCode::Char('r'), false), Some(Key::Rescan));
        assert_eq!(k(KeyCode::Char('w'), false), Some(Key::Theme));
        assert_eq!(k(KeyCode::Char('a'), true), Some(Key::Type('a')));
        assert_eq!(k(KeyCode::Char('a'), false), None, "letters only type in search mode");
        assert_eq!(k(KeyCode::Backspace, true), Some(Key::Backspace));
        assert_eq!(k(KeyCode::Esc, false), Some(Key::Esc));
    }

    /// `sibling_cli` points at the CLI: next to the browser first, PATH after.
    /// If the browser is installed alone (which is how it reached this machine),
    /// Enter used to do nothing and say `play failed: No such file or directory
    /// (os error 2)`. The path it looked at has to be visible in the message.
    #[test]
    fn the_cli_the_browser_looks_for_is_reported_by_name() {
        let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
        // The message the loop builds for that case.
        let cli = sibling_cli();
        let status = format!("no encuentro el CLI rosadeck (busqué {cli}); instálalo o ponlo en el PATH");
        assert!(status.contains(&cli), "nombra lo que buscó: {status}");
        assert!(!status.contains("os error"), "no suelta un error del sistema: {status}");
        // And when it *is* installed next to the browser, that is what it uses.
        let sibling = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("rosadeck")))
            .expect("el navegador vive en un directorio");
        assert_eq!(sibling.file_name().unwrap(), "rosadeck");
    }

    /// Regression: inside a text editor every character is text. `/` is the one
    /// that bites, because it is also the search shortcut: typing a path with a
    /// `/` used to toggle the search box and lose the rest of the path.
    #[test]
    fn typing_a_path_never_trips_a_shortcut() {
        let ev = |c: char| {
            map_event(
                Event::Key(crossterm::event::KeyEvent::new(
                    KeyCode::Char(c),
                    crossterm::event::KeyModifiers::NONE,
                )),
                true,
            )
        };
        for c in ['/', 'd', 'q', 'r', 'w', 'f', '1', '0', '~', ' ', 'ñ'] {
            assert_eq!(ev(c), Some(Key::Type(c)), "`{c}` debe escribirse, no ser atajo");
        }
        // Control keys keep working while typing: they are not characters.
        assert_eq!(
            map_event(Event::Key(crossterm::event::KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE)), true),
            Some(Key::Enter)
        );
        // Outside an editor the shortcuts are still shortcuts.
        assert_eq!(ev_shortcut('/'), Some(Key::Search));
        assert_eq!(ev_shortcut('d'), Some(Key::Dirs));
        assert_eq!(ev_shortcut('1'), None, "los dígitos los filtra el runtime, no map_event");
    }

    fn ev_shortcut(c: char) -> Option<Key> {
        map_event(
            Event::Key(crossterm::event::KeyEvent::new(KeyCode::Char(c), crossterm::event::KeyModifiers::NONE)),
            false,
        )
    }

    /// The roots editor: every character is a path while it is open, and the two
    /// text editors never overlap.
    #[test]
    fn the_roots_editor_types_paths_and_never_eats_shortcuts() {
        let mut b = Browser::new();
        assert_eq!(handle_key(&mut b, 4, Key::Dirs), BrowserKey::Continue);
        assert!(b.roots_input && b.roots_text.is_empty());
        assert_eq!(handle_key(&mut b, 4, Key::Enter), BrowserKey::Continue, "Enter vacío no inventa una raíz");
        assert!(b.roots_input, "y no cierra el editor: Esc es lo que cancela");
        for c in [Key::Type('/'), Key::Type('m'), Key::Type('n'), Key::Type('t'), Key::Type('~')] {
            assert_eq!(handle_key(&mut b, 4, c), BrowserKey::Continue);
        }
        assert_eq!(b.roots_text, "/mnt~");
        assert_eq!(handle_key(&mut b, 4, Key::Backspace), BrowserKey::Continue);
        assert_eq!(b.roots_text, "/mnt");
        assert_eq!(handle_key(&mut b, 4, Key::Type('g')), BrowserKey::Continue);
        assert_eq!(handle_key(&mut b, 4, Key::Enter), BrowserKey::SetRoot("/mntg".into()));
        assert!(!b.roots_input && b.roots_text.is_empty());
        assert_eq!(handle_key(&mut b, 4, Key::Enter), BrowserKey::Play(0), "Enter vuelve a lanzar");
        assert_eq!(handle_key(&mut b, 4, Key::Dirs), BrowserKey::Continue);
        assert_eq!(handle_key(&mut b, 4, Key::Type('x')), BrowserKey::Continue);
        assert_eq!(handle_key(&mut b, 4, Key::Esc), BrowserKey::Continue);
        assert!(!b.roots_input);
        assert_eq!(handle_key(&mut b, 4, Key::Dirs), BrowserKey::Continue);
        assert_eq!(handle_key(&mut b, 4, Key::Search), BrowserKey::Continue);
        assert!(b.searching && !b.roots_input, "buscar y editar raíces no se pisan");
    }

    /// A modal window takes the keyboard: the shelf behind it must not move.
    #[test]
    fn the_window_swallows_navigation() {
        let mut b = Browser::new();
        handle_key(&mut b, 9, Key::Right);
        assert_eq!(b.cursor, 1);
        assert_eq!(handle_key(&mut b, 9, Key::Dirs), BrowserKey::Continue);
        for k in [
            Key::Left,
            Key::Right,
            Key::Up,
            Key::Down,
            Key::PageUp,
            Key::PageDown,
            Key::Home,
            Key::End,
            Key::Favorite,
        ] {
            assert_eq!(handle_key(&mut b, 9, k), BrowserKey::Continue, "{k:?} no mueve el estante");
            assert_eq!(b.cursor, 1, "{k:?} movió el cursor detrás de la ventana");
        }
        assert!(b.roots_input, "la ventana sigue abierta");
        // Y al cerrarse, el estante vuelve a obedecer.
        assert_eq!(handle_key(&mut b, 9, Key::Esc), BrowserKey::Continue);
        assert_eq!(handle_key(&mut b, 9, Key::End), BrowserKey::Continue);
        assert_eq!(b.cursor, 8);
    }

    #[test]
    fn launch_error_quotes_the_cli_and_adds_the_package_only_when_missing() {
        let game = GameEntry {
            id: "n64-1".into(),
            title: "Mario Party 2".into(),
            platform: Platform::N64,
            path: "/roms/n64/mario party.z64".into(),
            region: None,
            size: 12,
        };
        let missing = emulators::EmulatorStatus {
            id: "mupen64plus".into(),
            name: "Mupen64Plus".into(),
            binary: "mupen64plus".into(),
            resolved: None,
        };
        let installed = emulators::EmulatorStatus { resolved: Some("/usr/sbin/mupen64plus".into()), ..missing.clone() };

        let msg = launch_error(&game, &[missing], Some(b"rosadeck: PREPARE_FAILED: binary not found: mupen64plus\n"));
        assert!(msg.contains("PREPARE_FAILED"), "{msg}");
        assert!(msg.contains("pacman -S mupen64plus"), "package hint when the binary is missing: {msg}");

        let msg = launch_error(&game, &[installed], Some(b"rosadeck: PREPARE_FAILED: invalid emulator profile: bad\n"));
        assert!(msg.contains("invalid emulator profile"), "{msg}");
        assert!(!msg.contains("pacman"), "no pacman noise when the binary exists: {msg}");

        let msg = launch_error(&game, &[], None);
        assert!(msg.contains("Mario Party 2"), "{msg}");
    }

    #[test]
    fn artwork_stamp_notices_new_and_replaced_covers() {
        // The bug this fixes: a cover dropped in while the browser was open
        // stayed invisible until `r`, because resolution was memoized forever.
        let dir = std::env::temp_dir().join("rosadeck-art-stamp");
        std::fs::remove_dir_all(&dir).ok();
        let roms = dir.join("roms");
        let covers = roms.join("covers");
        std::fs::create_dir_all(roms.join("snes")).unwrap();
        std::fs::create_dir_all(&covers).unwrap();
        let rom = roms.join("snes/Game (Europe).sfc");
        std::fs::write(&rom, b"rom").unwrap();

        let games = vec![GameEntry {
            id: "g".into(),
            title: "Game".into(),
            platform: Platform::Snes,
            path: rom,
            region: Some("Europe".into()),
            size: 3,
        }];
        let dirs = art_dirs(&games);
        assert!(dirs.contains(&covers), "the covers folder must be watched: {dirs:?}");
        assert!(art_files(&games).is_empty(), "nothing to watch yet");

        // A cover appears: the directory mtime moves.
        let before = art_stamp(&dirs, &art_files(&games));
        std::fs::write(covers.join("Game.png"), b"first").unwrap();
        let after_add = art_stamp(&dirs, &art_files(&games));
        assert_ne!(before, after_add, "adding a cover must change the stamp");

        // It is picked up once the cache is reset (that is what
        // `invalidate_art` does when the stamp moves)...
        let mut cache = cover::CoverCache::new();
        assert_eq!(cache.art_path(&games[0].path), Some(covers.join("Game.png")));
        // ...and replacing its contents must also be noticed: the directory
        // mtime does not move for an in-place write, the file stamp does.
        let files = art_files(&games);
        assert_eq!(files, vec![covers.join("Game.png")]);
        let resolved = art_stamp(&dirs, &files);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(covers.join("Game.png"), b"second and longer").unwrap();
        assert_ne!(resolved, art_stamp(&dirs, &files), "replacing a cover must change the stamp too");

        // Nothing changed: no churn, so no needless repaints. Crucially the
        // watched set is fixed at rescan, so panning the carousel (which
        // resolves covers lazily) must never look like new artwork.
        let stable = art_stamp(&dirs, &files);
        assert_eq!(stable, art_stamp(&dirs, &files));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn key_releases_are_ignored() {
        let mut ev = crossterm::event::KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE);
        ev.kind = KeyEventKind::Release;
        assert_eq!(map_event(Event::Key(ev), false), None);
        assert_eq!(map_event(Event::FocusGained, false), None);
    }

    #[test]
    fn platform_digits_and_all() {
        assert_eq!(platform_filter('1'), Some(Some(Platform::Snes)));
        assert_eq!(platform_filter('5'), Some(Some(Platform::Nintendo3DS)));
        assert_eq!(platform_filter('0'), Some(None));
        assert_eq!(platform_filter('x'), None);
    }

    #[test]
    fn runtime_writes_frames_through_tui_frame() {
        // Regression guard: writeln!/write! straight to stdout in raw mode
        // stair-steps the frame (OPOST is cleared by cfmakeraw).
        let prod = include_str!("main.rs").split("#[cfg(test)]").next().unwrap();
        // Frames go through the diff painter (no full-screen repaint per key).
        assert!(prod.contains("rosadeck_tui_frame::Screen"), "must paint with tui-frame::Screen");
        assert!(prod.contains("screen.draw("), "must paint with the diffing screen");
        assert!(!prod.contains("Clear(All)"), "no full-screen clear per frame (flicker)");
        assert!(!prod.contains("write!(out,"), "must not write frames with bare write!");
        assert!(prod.contains("KeyEventKind::Release"), "must ignore kitty key releases");
    }

    #[test]
    fn library_never_touches_hyprland_itself() {
        // The browser is read-only: no hyprctl, no socket, no sysfs. Enter
        // delegates to `rosadeck play`, the single sanctioned launch path.
        let prod = include_str!("main.rs").split("#[cfg(test)]").next().unwrap();
        for banned in ["hyprctl", ".socket.sock", "HYPRLAND_INSTANCE_SIGNATURE", "/sys/class/drm"] {
            assert!(!prod.contains(banned), "library must not reference {banned}");
        }
        assert!(prod.contains("sibling_cli()"), "launch must go through the rosadeck CLI");
    }

    #[test]
    fn depth_plain_forces_no_art() {
        let t = Theme::plain();
        assert_eq!(t.depth, theme::Depth::Plain);
        assert!(!t.art);
    }

    #[test]
    fn theme_mode_parsing() {
        assert_eq!(theme_mode(Some("pywal".into())), pywal::Mode::Pywal);
        assert_eq!(theme_mode(Some("system".into())), pywal::Mode::System);
        assert_eq!(theme_mode(Some("builtin".into())), pywal::Mode::System);
        assert_eq!(theme_mode(Some("nonsense".into())), pywal::Mode::Auto);
    }

    #[test]
    fn flags_are_parsed() {
        let a = parse_args_with(["rosadeck-library", "--dump", "--no-color", "--roms", "/tmp/x", "--theme", "pywal"].map(String::from));
        assert!(a.dump && a.plain);
        assert_eq!(a.roms.as_deref(), Some("/tmp/x"));
        assert_eq!(a.theme, pywal::Mode::Pywal);
        let b = parse_args_with(["rosadeck-library"].map(String::from));
        assert!(!b.dump && !b.plain && b.roms.is_none());
    }
}