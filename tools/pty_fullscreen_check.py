#!/usr/bin/env python3
"""Verify that a launched emulator really is fullscreen.

Presses Enter on a game in the real library TUI (pty), waits for the emulator
window and reads its geometry from `hyprctl clients -j` — read-only, no dispatch,
no mutation. The point is to catch the "the flag is accepted but does nothing"
class of bug: snes9x-gtk used to be launched with `--fullscreen`, which parsed
fine and left a 634x676 window.

Usage: pty_fullscreen_check.py <emulator-comm> [window-class-substring] [keys]
Exit 0 = the window is fullscreen (or fills the monitor).
"""
import json, os, pty, fcntl, termios, struct, select, signal, subprocess, sys, time

COLS, ROWS = 190, 46
comm = sys.argv[1] if len(sys.argv) > 1 else "snes9x-gtk"
cls = (sys.argv[2] if len(sys.argv) > 2 else comm).lower()
pre_keys = (sys.argv[3] if len(sys.argv) > 3 else "1").encode()


def pids(name):
    return subprocess.run(["pgrep", "-x", name], capture_output=True, text=True).stdout.split()


def windows():
    try:
        out = subprocess.run(["hyprctl", "clients", "-j"], capture_output=True, text=True).stdout
        return json.loads(out)
    except (json.JSONDecodeError, OSError):
        return []


def monitors():
    try:
        out = subprocess.run(["hyprctl", "monitors", "-j"], capture_output=True, text=True).stdout
        return json.loads(out)
    except (json.JSONDecodeError, OSError):
        return []


pid, fd = pty.fork()
if pid == 0:
    # El proyecto puede estar en cualquier sitio: se usa el de este repo.
    os.chdir(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    os.environ["TERM"] = "xterm-kitty"
    os.environ["ROSADECK_NO_IMAGES"] = "1"
    os.execvp("./target/debug/rosadeck-library", ["rosadeck-library"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))


def drain(seconds):
    end = time.time() + seconds
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try:
                if not os.read(fd, 65536):
                    break
            except OSError:
                break


drain(2.0)
if pre_keys:
    os.write(fd, pre_keys)
    drain(0.8)
os.write(fd, b"\r")

def matching():
    """Windows of this emulator.

    Dolphin has two (`org.kde.dolphin` main window, `dolphin-emu` game
    window, and a loading popup), so the check must not settle for the first
    hit: prefer a fullscreen one, else the biggest.
    """
    hits = [c for c in windows() if cls in (c.get("class") or "").lower()]
    if not hits:
        return None, hits
    full = [c for c in hits if c.get("fullscreen") in (1, 2)]
    if full:
        return max(full, key=lambda c: c["size"][0] * c["size"][1]), hits
    return max(hits, key=lambda c: c["size"][0] * c["size"][1]), hits


deadline = time.time() + 25
win = None
seen = []
while time.time() < deadline:
    time.sleep(0.6)
    if not pids(comm):
        continue
    win, seen = matching()
    if win and win.get("fullscreen") in (1, 2):
        break

# Leave the game the way a player does (Dolphin ignores the first SIGTERM).
for attempt in (signal.SIGTERM, signal.SIGKILL):
    if not pids(comm):
        break
    for p in pids(comm):
        try:
            os.kill(int(p), attempt)
        except ProcessLookupError:
            pass
    time.sleep(1.5)
os.kill(pid, signal.SIGTERM)
time.sleep(0.5)
subprocess.run(["pkill", "-x", comm], capture_output=True)

if not win:
    print(f"FALLO  no apareció ninguna ventana de '{comm}'")
    sys.exit(1)

if seen:
    print("ventanas candidatas: " + ", ".join(
        f"{c.get('class')} {c.get('size')} fs={c.get('fullscreen')}" for c in seen))
size, at = win.get("size"), win.get("at")
full = win.get("fullscreen")
mons = [m for m in monitors() if m.get("width")]
print(f"ventana {win.get('class')}: size={size} at={at} fullscreen={full}")
if full in (1, 2):
    print("OK  la ventana está en pantalla completa")
    sys.exit(0)
# No fullscreen flag but the window happens to cover the monitor: also fine.
if mons and size and at == [0, 0]:
    m = mons[0]
    if size[0] >= m["width"] and size[1] >= m["height"]:
        print(f"OK  la ventana cubre el monitor ({size[0]}x{size[1]})")
        sys.exit(0)
print(f"FALLO  ventana {size} en {at}: NO está a pantalla completa")
sys.exit(1)
