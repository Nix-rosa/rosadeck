#!/usr/bin/env python3
"""Regression check for "the UI is garbled when the emulator closes".

Runs the real library TUI in a pty, presses Enter, waits for the emulator to
appear, kills it (that is what "closing the emulator" means for a player) and
then compares two *terminal models*:

* screen A = the frame before the launch,
* screen B = the frame after the emulator is gone (replayed from the browser's
  last `?1049h`, because a terminal clears the alternate buffer on entry).

Both are replayed through `tools/vtterm.py`, i.e. compared cell by cell, not by
comparing bytes. Only the rows that *should* change are allowed to differ: the
platform chips and the footer (status, rule, detail, legend) — the play count
really goes up when a game starts. The header box, the rules and the whole cover
band must come back identical, because a row that was not repainted after the
alternate screen was re-entered is exactly the bug this checks.

Usage: pty_return_check.py [emulator-comm] [keys-before-enter]

`keys-before-enter` are raw bytes sent before Enter ("1" SNES, "2" N64, "4"
Wii, "5" 3DS), so the cursor is on the game of that platform.

Exit 0 = the browser came back exactly as it left.
"""
import os, pty, fcntl, termios, struct, select, signal, subprocess, sys, time, re

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from vtterm import replay

COLS, ROWS = 190, 46
comm = sys.argv[1] if len(sys.argv) > 1 else "dolphin-emu"
pre_keys = (sys.argv[2] if len(sys.argv) > 2 else "").encode()


def emulator_pids():
    return subprocess.run(["pgrep", "-x", comm], capture_output=True, text=True).stdout.split()


pid, fd = pty.fork()
if pid == 0:
    # El proyecto puede estar en cualquier sitio: se usa el de este repo.
    os.chdir(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    os.environ["TERM"] = "xterm-kitty"
    os.environ["COLORTERM"] = "truecolor"
    # Half-block rendering: a graphics protocol would hide the text bugs behind
    # images, and this check is about the text layer the player reads.
    os.environ["ROSADECK_NO_IMAGES"] = "1"
    os.execvp("./target/debug/rosadeck-library", ["rosadeck-library"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))

stream = bytearray()


def drain(seconds):
    end = time.time() + seconds
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try:
                chunk = os.read(fd, 65536)
            except OSError:
                break
            if not chunk:
                break
            stream.extend(chunk)
    return len(stream)


drain(2.0)                       # first frame
if pre_keys:
    os.write(fd, pre_keys)
    drain(0.8)
launch_at = len(stream)
os.write(fd, b"\r")              # Enter

# Wait for the emulator to really be running.
deadline = time.time() + 20
while time.time() < deadline and not emulator_pids():
    time.sleep(0.5)
if not emulator_pids():
    os.kill(pid, signal.SIGTERM)
    print(f"FALLO  el emulador '{comm}' no llegó a arrancar")
    sys.exit(1)

drain(1.0)
# Closing the game the way a player does. Dolphin answers the first SIGTERM with
# "A second signal will force Dolphin to stop." and keeps running, so escalate:
# without this the emulator never exits and the browser never comes back (which
# made an earlier version of this check pass without proving anything).
for attempt in (signal.SIGTERM, signal.SIGKILL):
    pids = emulator_pids()
    if not pids:
        break
    for p in pids:
        try:
            os.kill(int(p), attempt)
        except ProcessLookupError:
            pass
    deadline = time.time() + 5
    while time.time() < deadline and emulator_pids():
        time.sleep(0.2)

# Wait for the browser to take the screen back and repaint.
deadline = time.time() + 20
while time.time() < deadline and emulator_pids():
    time.sleep(0.3)
mark = len(stream)
drain(3.0)                       # its poll loop is 400 ms; give it two frames

os.kill(pid, signal.SIGTERM)
time.sleep(0.5)
subprocess.run(["pkill", "-x", comm], capture_output=True)

data = bytes(stream)
alt_in = b"\x1b[?1049h"
last_enter = data.rfind(alt_in, 0, mark)
if last_enter < 0:
    print("FALLO  el navegador nunca volvió a entrar en la pantalla alternativa")
    sys.exit(1)
if len(data) - last_enter < 2000:
    # Without this guard a browser that never repaints at all would replay as
    # "identical" and the check would pass without testing anything.
    print(f"FALLO  sólo {len(data) - last_enter} bytes después de volver: no hubo repintado completo")
    sys.exit(1)

before = replay(data[:launch_at], COLS, ROWS)
after = replay(data[last_enter:], COLS, ROWS)


def row_text(screen, y):
    return "".join(c for c, _, _ in screen.buf[y]).rstrip()


def normalized(screen, y):
    """Row text with play counters (`2x`) folded: they legitimately change
    when a game starts, and a diff there is not a repaint bug."""
    return re.sub(r"\d+x", "Nx", row_text(screen, y))


# Rows that may legitimately change: the chips (play counters) and the three
# detail rows plus the status line. The rule and the legend are static chrome —
# if they differ, something was not repainted (that was the bug).
STATUS_ROW = ROWS - 6
ALLOWED = {3, STATUS_ROW} | {ROWS - 4, ROWS - 3, ROWS - 2}
diffs = [y for y in range(ROWS) if normalized(before, y) != normalized(after, y)]
expected = [y for y in diffs if y in ALLOWED]
surprises = [y for y in diffs if y not in ALLOWED]
for y in diffs:
    print(f"  fila {y}: antes |{row_text(before, y)[:96]}")
    print(f"          ahora |{row_text(after, y)[:96]}")
if not surprises:
    print(f"OK  cabecera, carrusel, regla y leyenda idénticos; cambiaron {expected} (estado/detalle/chips, esperado)")
    sys.exit(0)
print(f"FALLO  {len(surprises)} filas fuera del pie no se repintaron al volver del emulador: {surprises}")
sys.exit(1)
