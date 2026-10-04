# Hyprland keybind: `rosadeck menu` (Super + H)

> Rosadeck never edits your Hyprland configuration. Add the bind below
> manually; adjust the key to taste.

Hyprland ≥ 0.55 uses Lua configuration. The callback must not block: it only
spawns the external process; all work happens outside Lua.

```lua
-- ~/.config/hypr/hyprland.lua (or your lua config)
hl.bind("SUPER + H", function()
  hl.dispatch(hl.dsp.exec_cmd("rosadeck menu --wofi"))
end)
```

Classic syntax equivalent (currently in use on this machine):

```ini
bind = SUPER, H, exec, /home/rosa/rosadeck/target/debug/rosadeck menu --wofi
```

Notes:

- `rosadeck menu` works with and without HDMI (state-only menu otherwise).
- `rosadeck menu --gaming` limits the overlay to *External Gaming* + Cancel.
- For a direct gaming entry: bind `rosadeck gaming --yes` (snapshots, plans,
  applies, verifies; restores on failure).
- Never put filesystem, socket or sleep logic inside the Lua callback.
- Verify the bind: press Super+H with no HDMI → small state menu appears.

## Optional: open the game library

The library is a full-screen TUI, so bind it to something that does not clash
with the display menu (`SUPER + H` is already taken):

```ini
bind = SUPER, L, exec, /home/rosa/rosadeck/target/debug/rosadeck-library
```

Inside it: `↑↓←→` move · `⏎` play · `f` favorite · `/` search · `1-5`
platform · `0` all · `r` rescan · `q` quit.

It is a terminal UI, so it must run inside a real terminal (kitty, foot,
alacritty…) — launching it from a keybind works because kitty is your default
terminal; from a bare `exec` rule in Hyprland, spawn it inside kitty
explicitly instead. Nothing about it touches displays: `Enter` only delegates
to `rosadeck play`, which owns the (optional) display transition.
