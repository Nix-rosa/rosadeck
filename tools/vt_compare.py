#!/usr/bin/env python3
"""Verify that partial repaints leave the terminal exactly as a full repaint would.

Runs `rosadeck-library` in a real pty, presses keys (partial diff repaints),
then forces a full repaint by bouncing the window size (the app invalidates its
screen on resize). Both byte streams are replayed through `tools/vtterm.py` and
the resulting screens must be identical: any stale cells left behind by a
column-level patch would show up here.

Usage: vt_compare.py <cols> <rows> <binary> [keyhex,keyhex,...]
Exit 0 = partial repaint == full repaint.
"""
import os, pty, fcntl, termios, struct, select, sys, time, signal
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from vtterm import replay, show

COLS, ROWS = int(sys.argv[1]), int(sys.argv[2])
BIN = sys.argv[3]
KEYS = [bytes.fromhex(h) for h in sys.argv[4].split(",")] if len(sys.argv) > 4 else []


def session(force_repaint):
    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-kitty"
        os.environ["COLORTERM"] = "truecolor"
        os.execvp(BIN, [BIN])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))

    def rd(t=0.6):
        out = b""
        end = time.time() + t
        while time.time() < end:
            r, _, _ = select.select([fd], [], [], 0.05)
            if r:
                try:
                    c = os.read(fd, 1 << 20)
                except OSError:
                    break
                if not c:
                    break
                out += c
        return out

    stream = rd(1.6)
    for k in KEYS:
        os.write(fd, k)
        stream += rd(0.5)
    stream += rd(0.3)
    if force_repaint:
        # Bounce the size: the app invalidates and repaints every row.
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS + 1, COLS, 0, 0))
        os.kill(pid, signal.SIGWINCH)
        stream += rd(0.7)
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        os.kill(pid, signal.SIGWINCH)
        stream += rd(0.9)
    try:
        os.write(fd, b"q")
        rd(0.3)
        os.kill(pid, signal.SIGTERM)
    except OSError:
        pass
    os.waitpid(pid, os.WNOHANG)
    return stream


keys = KEYS or [b"\x1b[B", b"\x1b[C", b"\x1b[B", b"\x1b[A"]
patched = session(False)
full = session(True)
a = replay(patched, COLS, ROWS)
b = replay(full, COLS, ROWS)
print(f"parche: {len(patched):,} bytes · con repintado completo: {len(full):,} bytes")
if a.dump() == b.dump():
    print("PASS: el parcheo por columnas deja la pantalla idéntica al repintado completo")
    sys.exit(0)
print("FAIL: hay celdas obsoletas (parcheo incompleto)")
for idx, (la, lb) in enumerate(zip(a.dump().split("\n"), b.dump().split("\n"))):
    if la != lb:
        print(f" fila {idx}")
        print("  parche  :", la[:160])
        print("  completo:", lb[:160])
        break
if "--show" in sys.argv:
    print(show(a, 20))
sys.exit(1)
