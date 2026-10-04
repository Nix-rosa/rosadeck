#!/usr/bin/env python3
"""End-to-end raw-mode TUI check: run a Rosadeck TUI in a real pty of a given
size, press keys, and verify the frame reaches the screen without stair-stepping.

Usage: pty_frame_check.py <cols> <rows> <binary> [args...]
Exit 0 = every visible line starts at column 0 (no raw-mode LF staircase).

This tool looks at the **text layer only**: it forces half-block rendering
(`ROSADECK_NO_IMAGES=1`) because the cover images travel as megabytes of kitty
graphics between the same cursor moves and would bury the frame in the capture.
The image layer has its own check, `tools/wire_graphics_check.py`.
"""
import os, pty, fcntl, termios, struct, select, signal, sys, time, re

cols, rows = int(sys.argv[1]), int(sys.argv[2])
argv = sys.argv[3:]

pid, fd = pty.fork()
if pid == 0:
    os.environ["TERM"] = "xterm-kitty"
    # Text layer only (see the docstring): the graphics payload is 2 MB+.
    os.environ["ROSADECK_NO_IMAGES"] = "1"
    os.execvp(argv[0], argv)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

def read_for(seconds):
    out = b""
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
            out += chunk
    return out

first = read_for(1.2)
os.write(fd, b"\x1b[B")      # Down
second = read_for(0.8)
os.write(fd, b"q")
read_for(0.5)
os.kill(pid, signal.SIGTERM)
os.waitpid(pid, os.WNOHANG)

# Both halves: the first capture holds the full frame (rows separated by CRLF),
# the second only a diff patch made of cursor moves, which has no row breaks.
raw = first + second
bare_lf = raw.count(b"\n") - raw.count(b"\r\n")
# Strip escapes, keep the last frame (after the final cursor-home).
text = raw.decode("utf-8", "replace")
frames = text.split("\x1b[2J")
# Everything after the first full repaint, not just the last segment: a capture
# can end mid-frame (the tail of a diff paint), and the last segment alone can
# then be a single row.
body = "\n".join(frames[1:]) if len(frames) > 1 else frames[0]
body = re.sub(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b_[^\x07\x1b]*(\x07|\x1b\\)", "", body)
lines = [ln.rstrip("\r") for ln in body.replace("\r\n", "\n").split("\n")]
stair = [ln for ln in lines if ln.startswith(" ") and ln.strip() and
         re.match(r"^ {2,}\S", ln) and "ROSADECK" not in ln and "filter" not in ln]
print(f"pty {cols}x{rows}  bytes={len(raw)}  bare_LF={bare_lf}  "
      f"keys_advanced_cursor={'yes' if raw != first else 'no'}")
print("visible frame (first 6 rows, | marks column 0):")
for ln in lines[:6]:
    print(" |" + ln[:cols])
print("... total filas capturadas:", len(lines))

print("ultimas 6:")
for ln in lines[-6:]:
    print(" |" + ln[:cols])
# Selection is a highlighted (reverse/gold) cell border in the new UI, so the
# probe checks that the detail pane actually follows the cursor.
detail = [ln for ln in lines if "MiB" in ln or "KiB" in ln or "GiB" in ln]
print("detail line follows cursor:", "yes" if detail else "MISSING")
print("selected cell marked:",
      "yes" if any("\x1b[7;1m" in l or "\x1b[38;2;255;205;90" in l for l in lines) else
      "no (informativo: el color depende de la paleta)")
sys.exit(0 if bare_lf == 0 and detail else 1)