# Quickshell integration (capability-based, no auto-modification)

Rosadeck never writes `~/.config/quickshell/`. It probes, then degrades:

| Observed | Rosadeck state |
|---|---|
| `quickshell` running + screen/monitor/output IPC handler | `Available` |
| running, toggle-only handlers (reference shell) | `NotConfigured` → warning, session continues |
| not running / no `qs` CLI | `Unsupported` → skipped |

Probe details: process scan of `/proc`, `qs ipc show` (read-only; never
`call`/`prop` from Rosadeck in F6).

Observed on the reference machine: 7 targets (`wifi`, `randomwallpaper`,
`music`, `launcher`, `dashboard`, `wallpaper`, `bluetooth`), all toggles
except `randomwallpaper.apply(path)`. No screen-assignment or reload
handler exists there, so that shell reports `NotConfigured`.

## Recommended shell-side pattern (user opt-in)

Keep screens dynamic instead of naming outputs:

```qml
// inside your shell root: track the target screen, never "eDP-1"/"HDMI-A-1"
property var targetScreen: Quickshell.screens.find(s => s.name !== internalName) ?? Quickshell.screens[0]
```

and bind panel windows to `screen: targetScreen` where isolation matters.
`PanelWindow` instances without an explicit `screen` already follow
per-screen placement on typical shells (assumption: standard Quickshell
behavior — report deviations).

## Wallpaper

The reference setup delegates wallpapers to `awww` per output
(`awww query` / `awww img <output> <image>`); the shell's
`applyWallpaper` shells out to it. Rosadeck therefore drives `awww`
directly when available and leaves shell wallpaper UI untouched.
