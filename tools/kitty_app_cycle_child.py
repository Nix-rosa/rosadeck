#!/usr/bin/env python3
"""Reproduce, dentro de un kitty real, la secuencia exacta que manda la app.

`tools/kitty_id_probe_child.py` ya respondió lo que responde el *protocolo*:

    a=p recién transmitido        -> OK
    a=p tras a=d,d=i               -> OK      (los datos siguen)
    a=p tras a=d,d=A               -> ENOENT

Pero el usuario ve las portadas vacías al cerrar la ventana, y la captura de
píxeles (`tools/kitty_pixel_check.py`) lo confirma. Aquí se reproduce la
secuencia completa tal cual la manda `rosadeck-library`, incluida la parte que
faltaba en la sonda anterior — el `[2J` del pintor por diferencias — y se
comprueba con la respuesta de kitty y con los píxeles de la ventana.

Cada fase deja la ventana 6 s quieta para poder capturarla con `grim`.

Se lanza con:  kitty --class rosadeck-probe ... python3 tools/kitty_app_cycle_child.py
"""
from __future__ import annotations

import base64
import os
import sys
import termios
import time
import tty

REPORT = "/tmp/opencode/kitty-app-cycle.txt"
OUT = "/tmp/opencode/kitty-app-cycle.marks"
COVER_DIR = os.path.expanduser("~/roms/covers")
STATE_DIR = os.path.expanduser("~/.local/state/rosadeck/covers")

_log: list[str] = []


def say(text: str) -> None:
    _log.append(text)
    with open(REPORT, "w") as fh:
        fh.write("\n".join(_log) + "\n")


def out(data: bytes) -> None:
    os.write(1, data)


def g(body: str) -> None:
    out(f"\x1b_G{body}\x1b\\".encode())


def reply(seconds: float = 0.8) -> str:
    import select

    end = time.time() + seconds
    buf = b""
    while time.time() < end:
        r, _, _ = select.select([0], [], [], 0.05)
        if r:
            try:
                buf += os.read(0, 1 << 16)
            except OSError:
                break
    for piece in buf.split(b"\x1b_Gi=")[1:]:
        body = piece.split(b"\x1b\\", 1)[0].decode("latin1")
        verdict = body.split(";", 1)[1] if ";" in body else ""
        return "ENOENT" if verdict.startswith("ENOENT") else ("OK" if verdict.startswith("OK") else verdict[:24])
    return "sin respuesta"


def send_text(text: str, clear: bool = True) -> None:
    """Texto visible como el pintor: un [2J al principio y luego las filas."""
    if clear:
        out(b"\x1b[2J")
    for i, line in enumerate(text.split("\n"), start=1):
        out(f"\x1b[{i};1H\x1b[38;5;252m{line}".encode())


def find_covers() -> list[tuple[str, int, int, int, int]]:
    """(fichero, id, z, x, w) para una portada nativa y tres vecinas."""
    names = sorted(n for n in os.listdir(COVER_DIR) if n.endswith(".png")) if os.path.isdir(COVER_DIR) else []
    picks: list[tuple[str, int, int, int, int]] = []
    for i, name in enumerate(names[:4]):
        z = 1 if i == 0 else 0
        picks.append((os.path.join(COVER_DIR, name), i + 1, z, 50 if z == 0 else 0, 62 if z == 0 else 0))
    # Y las reducidas del caché, que es lo que usa de verdad la banda.
    if os.path.isdir(STATE_DIR):
        for j, name in enumerate(sorted(os.listdir(STATE_DIR))[:3]):
            picks.append((os.path.join(STATE_DIR, name), 20 + j, 0, 42, 51))
    return picks


def main() -> int:
    tty.setraw(0)
    attrs = termios.tcgetattr(1)
    attrs[1] &= ~termios.OPOST
    termios.tcsetattr(1, termios.TCSANOW, attrs)

    covers = find_covers()
    if not covers:
        say("no hay portadas en ~/roms/covers")
        return 1
    say(f"portadas: {[os.path.basename(c[0]) for c in covers]}")

    def transmit(path: str, img_id: int, cols: int, rows: int, z: int) -> None:
        g(f"a=t,f=100,t=f,q=2,i={img_id},z={z},c={cols},r={rows};{base64.b64encode(path.encode()).decode()}")

    def place_at(img_id: int, row: int, col: int, cols: int, rows: int, z: int, x: int, w: int, quiet: bool) -> None:
        out(f"\x1b[{row};{col}H".encode())
        crop = f",x={x},w={w}" if w else ""
        g(f"a=p,i={img_id},p={img_id},q=2,C=1,z={z}{crop},c={cols},r={rows}" if quiet else
          f"a=p,i={img_id},p={img_id},C=1,z={z}{crop},c={cols},r={rows}")

    # Fase 1: el arranque de la app — transmitir y colocar.
    positions = [(6, 14, 29, 19, 1, 0, 0), (6, 72, 13, 17, 0, 50, 62), (8, 92, 8, 13, 0, 42, 51), (10, 104, 5, 11, 0, 40, 35)]
    for (path, img_id, z, x, w), (row, col, c, r, pz, px, pw) in zip(covers[:4], positions):
        transmit(path, img_id, c, r, pz)
        place_at(img_id, row, col, c, r, pz, px, pw, quiet=True)
    say(f"fase 1 (arranque): preguntando al primero -> {reply()}")
    say("PHASE 1")
    time.sleep(8)

    # Fase 2: la ventana abre — el pintor pasa por [2J y la app borra las
    # colocaciones con d=i (datos dentro).
    send_text("VENTANA DE DIRECTORIOS DE ROM\n\n╭─directorios de ROM · 0 añadidas──╮\n│ ruta ▸ _                          │\n╰──────────────────────────────────╯")
    for (path, img_id, z, x, w) in covers[:4]:
        g(f"a=d,d=i,i={img_id},p={img_id},q=2")
    say("PHASE 2")
    time.sleep(8)

    # Fase 3: la ventana cierra — [2J, texto del estante y a=p sin payload.
    send_text("ROSADECK RETRO LIBRARY\n\n" + "\n".join("·" * 100 for _ in range(30)))
    for (path, img_id, z, x, w), (row, col, c, r, pz, px, pw) in zip(covers[:4], positions):
        place_at(img_id, row, col, c, r, pz, px, pw, quiet=False)
    say("PHASE 3")
    for (path, img_id, z, x, w) in covers[:4]:
        say(f"  id={img_id} tras cerrar -> {reply(0.5)}")
    time.sleep(10)

    # Fase 4: lo mismo, pero SIN el [2J de por medio (sólo texto).
    send_text("SIN [2J\n\n" + "\n".join("·" * 100 for _ in range(30)))
    for (path, img_id, z, x, w), (row, col, c, r, pz, px, pw) in zip(covers[:4], positions):
        place_at(img_id, row, col, c, r, pz, px, pw, quiet=True)
    say("PHASE 4")
    time.sleep(12)
    say("fin")
    return 0


if __name__ == "__main__":
    sys.exit(main())
