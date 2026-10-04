#!/usr/bin/env python3
"""Conduce `kitty_app_cycle_child.py` dentro de un kitty real y captura cada fase.

El niño (que corre *dentro* de kitty) va announcing `PHASE n` en
`/tmp/opencode/kitty-app-cycle.txt`; este conductor ve el aviso, saca una
captura de la ventana con `grim` y la guarda. Al final se puede mirar qué fase
tiene las portadas y cuál no, que es justo lo que el usuario está viendo.

Uso:  python3 tools/kitty_app_cycle_pixels.py
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import time

CLASS = "rosadeck-cycle-probe"
REPORT = "/tmp/opencode/kitty-app-cycle.txt"
SHOTS = "/tmp/opencode/cycle"


def run(cmd: list[str], timeout: float = 30) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)


def geometry() -> str | None:
    try:
        clients = json.loads(run(["hyprctl", "clients", "-j"]).stdout or "[]")
    except json.JSONDecodeError:
        return None
    for c in clients:
        if c.get("class") == CLASS:
            at, size = c["at"], c["size"]
            return f"{at[0]},{at[1]} {size[0]}x{size[1]}"
    return None


def ink(path: str) -> float:
    """Tinta (desviación) de toda la ventana: una portada es mucho más que texto."""
    from PIL import Image, ImageStat

    return ImageStat.Stat(Image.open(path).convert("L")).stddev[0]


def main() -> int:
    os.makedirs(SHOTS, exist_ok=True)
    if os.path.exists(REPORT):
        os.remove(REPORT)
    kitty = subprocess.Popen(
        [
            "kitty",
            "--class",
            CLASS,
            "-o",
            "background_opacity=1",
            "-o",
            "margin=0",
            "-o",
            "padding=0",
            "-o",
            "cursor_blink=0",
            "python3",
            os.path.join(os.path.dirname(os.path.abspath(__file__)), "kitty_app_cycle_child.py"),
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    seen: set[str] = set()
    try:
        deadline = time.time() + 75
        while time.time() < deadline:
            if os.path.exists(REPORT):
                for line in open(REPORT):
                    line = line.strip()
                    if line.startswith("PHASE ") and line not in seen:
                        seen.add(line)
                        time.sleep(0.7)      # que el terminal termine de pintar
                        geo = geometry()
                        if geo is None:
                            continue
                        path = os.path.join(SHOTS, f"{line.replace(' ', '_')}.png")
                        run(["grim", "-g", geo, path])
                        if os.path.exists(path):
                            print(f"  {line}: capturada (tinta {ink(path):.1f})")
                            continue
                    if line.startswith("fin"):
                        break
            time.sleep(0.3)
    finally:
        kitty.terminate()
        try:
            kitty.wait(timeout=5)
        except subprocess.TimeoutExpired:
            kitty.kill()
        run(["pkill", "-f", CLASS])
    print("--- informe del niño ---")
    if os.path.exists(REPORT):
        print(open(REPORT).read())
    print(f"capturas en {SHOTS}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
