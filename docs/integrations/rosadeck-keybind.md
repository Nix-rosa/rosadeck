
### Pywal colours

`rosadeck-library` picks up your wallpaper colours automatically: it reads
`$XDG_CACHE_HOME/wal/colors.json` (pywal ≥3.4 nested layout, and the legacy
flat one) and maps the palette to the six roles it needs, always checking WCAG
contrast against `special.background` because pywal keeps `color1..6` dark on
purpose.

- `--theme system` ignores pywal and uses the built-in magenta/cyan scheme.
- `--theme pywal` forces pywal (falls back to built-in if the cache is absent).
- `w` reloads the palette while the browser is open; if you run
  `pywal -i wall.jpg` in another terminal the browser notices on its own.
- `--no-color` (or `NO_COLOR=1`) drops all styling, so the layout still works
  in logs and screenshots.

Nothing is written to `~/.cache/wal` — the integration is read-only.

### Emulators: what Enter actually runs

Each platform has one default emulator, and it is the one that is really
installed on this machine:

| Platform | Emulator | Command used to launch |
| --- | --- | --- |
| Wii, GameCube | Dolphin | `dolphin-emu -b -C Dolphin.Display.Fullscreen=True -C GFX.Settings.VSync=True -e <rom>` |
| Nintendo 3DS | Azahar | `azahar -f <rom>` |
| SNES | Snes9x (GTK) | `snes9x-gtk <rom>` (fullscreen vía config propia, abajo) |
| Nintendo 64 | Mupen64Plus | `mupen64plus --fullscreen <rom>` |

RetroArch is still available as an opt-in profile (it needs one libretro core
per system, so it is not a default any more).

- Games always start fullscreen, and that is *measured*, not documented:
  `tools/pty_fullscreen_check.py` launches the game from the browser and reads
  the window geometry from `hyprctl clients -j` (read-only). Snes9x has no
  working fullscreen flag — its setting lives in `snes9x.conf`, so Rosadeck
  copies your config into
  `$XDG_STATE_HOME/rosadeck/snes9x-config/snes9x/snes9x.conf`, sets
  `Fullscreen`/`FullscreenOnOpen` on the copy and points the child's
  `XDG_CONFIG_HOME` at it. Your own `~/.config/snes9x/snes9x.conf` is read,
  never written, and your joystick/sound/filter settings come along.
- Pressing <kbd>Enter</kbd> asks the CLI first (`rosadeck play --id <id>
  --dry-run`, no spawn, no display change). If the emulator cannot run, nothing
  is launched and the status line says why:

  ```
  rosadeck: PREPARE_FAILED: emulator binary not found: retroarch
  rosadeck: install it yourself with: pacman -S retroarch (+ a libretro core per system)
  ```

  Rosadeck never installs packages; the hint is text to copy.
- The detail pane always states the state of the emulator
  (`emulator snes9x ✓`, `emulator retroarch NOT INSTALLED · pacman -S retroarch`,
  `emulator mednafen NOT CONFIGURED`), and the legend reads `⏎ needs emulator`
  instead of `⏎ play` while the selected game cannot start.
- The same information from the shell:

  ```bash
  rosadeck library            # emulator=dolphin ok / emulator=retroarch MISSING
  rosadeck emulators          # per profile: found path, platforms, install hint
  rosadeck emulators --json   # machine-readable, with "installed"
  rosadeck play "Mario Party 2" --dry-run
  ```

### Closing a game comes back clean

When you quit an emulator, the browser comes back exactly as it was: the whole
frame is repainted (a fresh alternate buffer is empty, so the painter repaints
every row) and the terminal is soft-reset first (Dolphin is a Qt app and leaves
SGR/cursor state behind).

The emulator's own console output never touches that screen: the browser always
launches with `--quiet`, which sends stdout/stderr to
`$XDG_STATE_HOME/rosadeck/emulators.log` and stdin to `/dev/null`. So messages
like Dolphin's `A second signal will force Dolphin to stop.` land in the log,
not on top of the UI.

The status line tells the truth about how the session ended:

```
played Luigi's Mansion          exit 0
Luigi's Mansion closed (exit 19)   the game ran and ended with an error — a normal quit
rosadeck: PREPARE_FAILED: …      never launched; the emulator cannot run
```

From a shell you can leave the output on your terminal (no `--quiet`):

```bash
rosadeck play "Mario Party 2" --yes
```

### Cover art: any format, named after the game

`rosadeck-library` picks up a cover for every game by convention — no config,
no scraping, no renaming of ROMs. Covers live in **`~/roms/covers/`**, which is
searched first so a drop-in always wins. Then, in order:

1. `~/roms/covers/`
2. `~/roms/covers/<platform>/`
3. next to the ROM
4. `~/roms/art/<platform>/` (older layout, still honoured)

Two names are accepted, matched case-insensitively:

1. **the game name**, e.g. `Luigi's Mansion.png`;
2. the ROM file name, e.g. `Luigi's Mansion (Europe) (En,Fr,De,Es,It).png`.

Titles that carry an article resolve in **both** spellings, because plenty of
ROM files and covers put it at the end:

```
The Legend of Zelda - A Link to the Past.png     <- the title as Rosadeck shows it
Legend of Zelda, The - A Link to the Past.png    <- the ROM's own spelling
```

The detail pane never claims a cover it cannot show: a file whose format the
decoder rejects is reported as `unreadable` instead of the game's name. And a
cover added while the browser is open is picked up on its own — no `r` needed;
the status line says `artwork updated`. Replacing a cover in place works too.

### Keys

The browser's own legend shows only what is not obvious from looking at it:
`←→` move, `⏎` play (or `needs emulator`), `f` favorite, `/` search, `d` ROM
directories, `q` quit. The rest still works and lives here:

| key | what it does |
|---|---|
| `↑` `↓` `PgUp` `PgDn` | jump a screenful of covers |
| `Home` `End` | first / last game |
| `1`-`5` | filter by platform (SNES, N64, GameCube, Wii, 3DS) |
| `0` or `.` | clear the platform filter |
| `r` | rescan the ROM roots |
| `w` | reload the pywal palette (`pywal -i wall.jpg`) |
| `Esc` | leave the search box, or quit if nothing is open |

Under the focused cover, next to the platform and region, the `▶` icon is **time
played**, not a launch counter: `45s`, `12m`, `3h 20m`, or `nunca` if the game has
never been opened. The CLI measures it (it is the one waiting for the emulator)
and writes it to `~/.local/state/rosadeck/library.json`; a session counts whether
the game exits cleanly or the way emulators normally exit (exit 19).

The status line (bottom left) says what the last action did; its right end always
says how the covers are reaching the terminal (`portadas: rutas`, `bytes` or
`bloques`) — that is the difference between «the app is slow» and «this binary is
a debug build».

### ROM directories: the `d` key

`rosadeck-library` looks in `~/roms/<platform>/`. To add more, press **`d`**
inside the browser: a window opens in the middle of the screen, over the shelf,
listing every root with the number of games it contributes:

```
╭─directorios de ROM · 2 en uso────────────────────────╮
│1   /home/rosa/roms                          12 juegos│
│2   /mnt/roms2                          no existe│
│                                                       │
│ruta ▸ /mnt/roms3_                                      │
│   Enter añade · repetirla la quita · Esc cancela       │
╰───────────────────────────────────────────────────────╯

/home/user/roms2 ⏎       added, saved, and the library is rescanned
/home/user/roms2 ⏎       typing it again removes it
Esc                      closes the window without changing anything
```

The covers disappear while the window is open and come back when it closes —
the terminal paints images over text, so a text window could not sit on top of
them. Inside the window every key is part of the path, and the arrows do not
move the shelf behind it.

(If a cover ever stays missing after closing the window, that is a bug, not a
style: opening and closing repaints the screen and the terminal drops its
images, so they have to be sent again. `tools/kitty_pixel_check.py` checks it
against a real kitty, in pixels.)

The paths are stored in **`$XDG_CONFIG_HOME/rosadeck/roms`** (usually
`~/.config/rosadeck/roms`), one per line — `rosadeck library` reads the same
file, so the CLI and the browser always agree:

```
# Rosadeck: directorios de ROM adicionales, uno por línea.
# Se añaden con la tecla `d` en rosadeck-library. `~` vale por $HOME.
/home/user/roms2
```

Order is `~/roms`, then the configured directories in the order they were added,
then `$ROSADECK_ROMS`; duplicates are dropped. A directory that does not exist
(an unplugged drive, a network mount that is down) is kept in the file and
skipped while it is missing, so it comes back on its own once mounted. Editing
that file by hand is fine too.

Set `ROSADECK_COVERS` (colon-separated) to use a different primary folder. Any
of these formats works:

`png jpg jpeg webp avif heic heif gif tif tiff bmp tga ico qoi pnm ppm pgm pbm
pam dds exr hdr ff`

Check what is missing and what name each cover needs:

```bash
rosadeck art            # ok / missing + the exact file name to create
rosadeck art --json     # machine-readable
```

The browser is a one-row **carousel**: the selected game is the focused card, at
full size and full colour, in the middle. Its neighbours fan out on both sides,
each one **narrower and shorter** than the one in front of it and **overlapping**
it — the hidden 45% of a neighbour is what makes it look tucked behind — and each
one *recedes*: blurred, desaturated to 30% of its chroma and blended 40%
towards the page background, so it looks further away instead of merely dimmer.
The focus is the top layer in both renderers (`z=1` in the graphics protocol,
painted last in the text).

A card is cropped, never squeezed: its artwork keeps the scale of the whole card
and only the part another card would cover is dropped (kitty crops with the
source rectangle). The same treatment and cropping apply to generated covers, so
a game without artwork still sits correctly in the fan. In a terminal with the kitty
graphics protocol (kitty itself) each cover is transmitted **once** as a real
image and displayed at the terminal's native resolution; everywhere else the same
artwork is drawn with half blocks (one pixel per character column). Games with no
cover get a generated one (gradient + title initials in the platform colour), so
the shelf never has holes.

- `--no-images` (or `ROSADECK_NO_IMAGES=1`) forces the half-block renderer.
- Every graphics command is sent quietly (`q=2`). This matters: the terminal
  answers on stdin when an image id is referenced, and those bytes would reach
  the keyboard parser as phantom key presses.
- The payload is always **PNG**, whatever the file is (`f=100` means PNG; a JPEG
  sent as such is rejected and, being quiet, fails invisibly). A JPEG cover is
  decoded once and re-encoded.
- Covers are placed with `z=0`, i.e. **above** the text: the artwork area is
  blank by construction, and a repainted space must never be able to hide a
  cover.
- The detail pane states which path is in use: `cover <file> image`,
  `cover <file> blocks` or `cover generated`.
