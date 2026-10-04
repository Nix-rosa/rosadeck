#!/usr/bin/env python3
"""Comprobación en píxeles contra un kitty de verdad (el suelo de la verdad).

El usuario reporteó «al cerrar la ventana de directorios desaparecen las primeras
portadas». Ni `pty_kitty.py` (un pty falso) ni `graphics_store_check.py` (un
modelo del protocolo) pueden verlo, y `kitty_id_probe_child.py` ya respondió lo
único que responde el protocolo:

    a=p recién transmitido            -> OK
    a=p tras a=d,d=i (esconder)       -> OK     (los datos siguen ahí)
    a=p tras a=d,d=A                   -> ENOENT (los datos se fueron)

O sea: la secuencia de la app es correcta *según el protocolo*. Faltaba mirar los
píxeles, que es lo único que se parece a lo que el usuario ve.

Montaje: un kitty real con la app dentro y `--listen-on`, así que las teclas se
mandan con `kitten @ send-key` y las capturas se piden al propio kitty
(`kitten @ screenshot`), sin `grim` ni `wtype`. Tres momentos:

    estante -> `d` (ventana) -> Esc (estante de vuelta)

Si al volver la banda no recupera los píxeles de las portadas, la app las ha
perdido de verdad.

Uso:  python3 tools/kitty_pixel_check.py [binario]
"""
from __future__ import annotations

import glob
import json
import os
import subprocess
import sys
import time

CLASS = "rosadeck-pixel-probe"
SOCK_PREFIX = "/tmp/opencode/pixel.sock"
SHOTS = "/tmp/opencode/shots"
BAND = (0.16, 0.82)          # fracción de altura ocupada por la banda
TOLERANCE = 8                # diferencia por debajo = ruido


def run(cmd: list[str], timeout: float = 30) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)


def window_geometry() -> str | None:
    """`X,Y WxH` de la ventana del probe (el formato que quiere grim)."""
    res = run(["hyprctl", "clients", "-j"])
    try:
        clients = json.loads(res.stdout or "[]")
    except json.JSONDecodeError:
        return None
    for c in clients:
        if c.get("class") == CLASS:
            at, size = c["at"], c["size"]
            return f"{at[0]},{at[1]} {size[0]}x{size[1]}"
    return None


def socket_path() -> str | None:
    """kitty añade el pid al nombre: hay que descubrirlo."""
    hits = sorted(glob.glob(f"{SOCK_PREFIX}-*"))
    return hits[0] if hits else None


class Kitty:
    def __init__(self, binary: str) -> None:
        for stale in glob.glob(f"{SOCK_PREFIX}-*"):
            os.remove(stale)
        self.proc = subprocess.Popen(
            [
                "kitty",
                "--class",
                CLASS,
                # Ojo: el flag `--listen-on` se ignora en kitty 0.48; por
                # configuración sí funciona (y el socket real lleva el pid).
                "-o",
                f"listen_on=unix:{SOCK_PREFIX}",
                # Sin esto el socket de control no aparece (medido: con y sin).
                "-o",
                "allow_remote_control=yes",
                "-o",
                "background_opacity=1",
                "-o",
                "margin=0",
                "-o",
                "padding=0",
                "-o",
                "confirm_os_window_close=0",
                "-o",
                "cursor_blink=0",
                "rosadeck-library",
            ],
            env=dict(os.environ, ROSADECK_BIN=binary),
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        self.sock = None
        for _ in range(60):
            self.sock = socket_path()
            if self.sock:
                return
            time.sleep(0.25)
        raise SystemExit("kitty no abrió el socket de control remoto")

    def at(self, *args: str) -> subprocess.CompletedProcess:
        return run(["kitten", "@", "--to", f"unix:{self.sock}", *args])

    def key(self, *keys: str) -> None:
        self.at("send-key", *keys)

    def shot(self, name: str) -> str:
        """Captura de la ventana del probe con `grim` (kitty 0.48 no trae la
        acción `screenshot` por control remoto)."""
        os.makedirs(SHOTS, exist_ok=True)
        path = os.path.join(SHOTS, f"{name}.png")
        if os.path.exists(path):
            os.remove(path)
        geo = window_geometry()
        if geo is None:
            raise SystemExit("no encuentro la ventana del probe en hyprctl")
        res = run(["grim", "-g", geo, path])
        if not os.path.exists(path):
            raise SystemExit(f"grim no capturó ({geo}): {res.stderr.strip()}")
        return path

    def close(self) -> None:
        try:
            self.at("close-window")
            time.sleep(0.4)
        except Exception:
            pass
        self.proc.terminate()
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        run(["pkill", "-f", CLASS])


def band(path: str):
    from PIL import Image

    img = Image.open(path).convert("RGB")
    w, h = img.size
    return img.crop((0, int(h * BAND[0]), w, int(h * BAND[1])))


def stats(path: str) -> tuple[float, float]:
    """(tinta = desviación, color =max-min de la media por canal)."""
    from PIL import ImageStat

    b = band(path)
    ink = ImageStat.Stat(b.convert("L")).stddev[0]
    mean = ImageStat.Stat(b.convert("RGB")).mean
    return ink, max(mean) - min(mean)


def difference(a: str, b: str) -> tuple[float, float]:
    """(% de píxeles distintos, diferencia media)."""
    from PIL import ImageChops, ImageStat

    pa, pb = band(a), band(b)
    if pa.size != pb.size:
        return 100.0, 255.0
    d = ImageChops.difference(pa, pb).convert("L")
    hist = d.histogram()
    total = pa.size[0] * pa.size[1]
    return 100.0 * sum(hist[TOLERANCE:]) / total, ImageStat.Stat(d).mean[0]


def columns(path: str, count: int = 40) -> list[float]:
    """Tinta por columna normalizada: perfil de la banda de izquierda a derecha.

    Las portadas son bloques anchos de color; el texto son puntitos. Este perfil
    dice *cuáles* tarjetas siguen having artwork, que es justo lo que reportó el
    usuario («las primeras 3 portadas»).
    """
    from PIL import ImageStat

    b = band(path).convert("L")
    w, h = b.size
    step = max(1, w // count)
    return [ImageStat.Stat(b.crop((i * step, 0, min((i + 1) * step, w), h))).stddev[0] for i in range(count)]


def profile_line(values: list[float]) -> str:
    return "".join("#" if v > 18 else ("+" if v > 8 else ".") for v in values)


def main() -> int:
    binary = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "./target/release/rosadeck-library")
    # El comando de kitty es fijo (`rosadeck-library`): se invoca por el PATH,
    # así que se comprueba que apunte al binario que se quiere probar.
    installed = os.path.expanduser("~/.local/bin/rosadeck-library")
    target = installed if os.path.exists(installed) else binary
    print(f"  binario: {target}")

    kitty = Kitty(target)
    bad = 0
    try:
        time.sleep(4.0)
        before = kitty.shot("antes")
        kitty.key("d")
        time.sleep(1.2)
        opened = kitty.shot("ventana")
        kitty.key("escape")
        time.sleep(1.5)
        after = kitty.shot("despues")

        # Un redimensionado también repinta entero (mismo `[2J`), así que la banda
        # tiene que seguir teniendo sus portadas después de crecer.
        kitty.at("resize-window", "--margin=0", "--cols=132", "--rows=44")
        time.sleep(2.0)
        grown = kitty.shot("crecida")
        kitty.at("resize-window", "--margin=0", "--cols=118", "--rows=39")
        time.sleep(1.5)
        pct_grown, _ = difference(after, grown)

        pct_open, _ = difference(before, opened)
        pct_back, mean_back = difference(before, after)
        ink_b, col_b = stats(before)
        ink_o, col_o = stats(opened)
        ink_a, col_a = stats(after)
        prof_b, prof_o, prof_a = columns(before), columns(opened), columns(after)

        print(f"  al abrir  la ventana: {pct_open:5.1f} % de la banda cambia")
        print(f"  al cerrar la ventana: {pct_back:5.1f} % de la banda cambia (media {mean_back:.1f}/255)")
        print(f"  tinta: estante {ink_b:.1f} · ventana {ink_o:.1f} · vuelta {ink_a:.1f}")
        print(f"  color: estante {col_b:.1f} · ventana {col_o:.1f} · vuelta {col_a:.1f}")
        print(f"  perfil estante: {profile_line(prof_b)}")
        print(f"  perfil ventana: {profile_line(prof_o)}")
        print(f"  perfil vuelta : {profile_line(prof_a)}")
        ink_g, col_g = stats(grown)
        print(f"  tras crecer la ventana: {pct_grown:.1f} % de cambio · tinta {ink_g:.1f} · color {col_g:.1f}")

        if pct_open < 1.0:
            print("  FALLO: la ventana no aparece (misma imagen antes y después de `d`)")
            bad += 1
        if ink_b < 12:
            print("  AVISO: la banda inicial apenas tiene tinta; la prueba no vale")
            bad += 1
        if ink_a < ink_b * 0.75:
            print(f"  FALLO: al cerrar la ventana la banda pierde artwork ({ink_b:.1f} -> {ink_a:.1f})")
            bad += 1
        if pct_back > 6.0 or mean_back > 10.0:
            print("  FALLO: la banda al cerrar no es la del principio")
            bad += 1
        if ink_g < 12:
            print(f"  FALLO: la ventana se quedó sin portadas al redimensionar (tinta {ink_g:.1f})")
            bad += 1
    except SystemExit as err:
        print(f"  ERROR: {err}")
        bad += 1
    finally:
        kitty.close()
    print("RESULTADO:", "FALLO" if bad else "ok")
    print(f"capturas en {SHOTS}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
