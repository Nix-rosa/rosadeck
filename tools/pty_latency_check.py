"""Latency of the carousel: how long from a keypress to the terminal having the
whole frame, and how many bytes had to travel to get there.

Run twice: against a pty that answers the `t=f` probe like kitty does (covers
travel as ~200-byte paths) and against a silent one (the probe fails and the app
must fall back to streaming the bytes). The difference is the whole reason the
probe exists.

Usage: tools/pty_latency_check.py [binary]
"""
import os
import re
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pty_kitty

COLS, ROWS = 190, 46
KEYS = 8
QUIET = 0.4


def measure(binary, answer):
    fd, pid = pty_kitty.spawn(COLS, ROWS, [binary, "rosadeck-library"], answer=answer)
    startup = bytearray()
    pty_kitty.drain(fd, 4.0, startup)
    keys = []
    for _ in range(KEYS):
        t0 = time.time()
        os.write(fd, b"\x1b[C")
        buf = bytearray()
        painted = pty_kitty.until_quiet(fd, limit=4.0, quiet=0.4, buf=buf)
        if painted is None:
            keys.append(None)      # the keypress produced nothing at all
            continue
        # `until_quiet` includes the quiet window it waits for.
        keys.append(((painted - QUIET) * 1000, len(buf) / 1024))
    pty_kitty.stop(pid)
    data = bytes(startup)
    paths = len(re.findall(rb"a=t,[^\x1b]*t=f", data))
    blobs = len(re.findall(rb"a=t,[^\x1b]*t=d", data))
    return len(startup) / 1024, paths, blobs, keys


def report(label, binary, answer):
    startup, paths, blobs, keys = measure(binary, answer)
    ok = [k for k in keys if k]
    missing = len(keys) - len(ok)
    how = f"{paths} por ruta" + (f", {blobs} en bytes" if blobs else "")
    if not ok:
        print(f"{label}: SIN RESPUESTA a las teclas ({missing}/{KEYS}) — sonda: {how}")
        return
    paint = sorted(k[0] for k in ok)
    kib = sorted(k[1] for k in ok)
    print(
        f"{label}\n"
        f"  arranque {startup:.0f} KiB ({how})\n"
        f"  tecla -> frame pintado {paint[0]:.0f}-{paint[-1]:.0f} ms "
        f"(mediana {paint[len(paint)//2]:.0f} ms)\n"
        f"  {kib[0]:.0f}-{kib[-1]:.0f} KiB por tecla"
        + (f" · {missing} teclas sin respuesta" if missing else "")
    )


binary = sys.argv[1] if len(sys.argv) > 1 else "./target/release/rosadeck-library"
report("terminal que lee ficheros (t=f, como kitty local)", binary, True)
report("terminal que no puede leerlos (bytes por el pty)", binary, False)