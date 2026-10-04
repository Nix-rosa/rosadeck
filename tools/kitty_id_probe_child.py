#!/usr/bin/env python3
"""Sonda del protocolo de gráficos: se ejecuta DENTRO de un kitty de verdad.

`tools/pty_kitty.py` es un pty falso que sólo contesta el sondeo de `t=f`, así
que no puede decir nada sobre el almacén de imágenes de kitty. Este script habla
el protocolo de verdad (escribe en stdout, lee las respuestas en stdin) y
contrasta las dos formas de quitar una colocación:

    a=d,d=i,i=<id>,p=<id>   minúscula: colocación fuera, datos dentro
    a=d,d=A                 mayúscula: también libera los datos

Cada coloca va sin `q=2` a propósito: el spec dice que kitty responde
`OK` o `ENOENT:…` y eso es lo único que de verdad prueba si la portada se está
pintando.

Se lanza con:  kitty --start-as=hidden python3 tools/kitty_id_probe_child.py
"""
from __future__ import annotations

import base64
import os
import struct
import sys
import time
import zlib

REPLY = b"\x1b_Gi="


def png_bytes(w: int = 12, h: int = 18) -> bytes:
    """Un PNG diminuto, escrito a pelo para no depender de la biblioteca."""

    def chunk(tag: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    raw = b"".join(
        b"\x00" + b"".join(bytes([(x * 17) % 256, (y * 11) % 256, 128]) for x in range(w)) for y in range(h)
    )
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def send(body: str) -> None:
    sys.stdout.buffer.write(f"\x1b_G{body}\x1b\\".encode())
    sys.stdout.buffer.flush()


def read_reply(seconds: float = 0.6) -> str:
    """Lee la respuesta de kitty a la última colocación (`OK` o `ENOENT:…`)."""
    import select

    end = time.time() + seconds
    buf = b""
    while time.time() < end:
        r, _, _ = select.select([sys.stdin.buffer], [], [], 0.05)
        if r:
            buf += os.read(sys.stdin.fileno(), 1 << 16)
            if REPLY in buf:
                break
    for piece in buf.split(REPLY)[1:]:
        body = piece.split(b"\x1b\\", 1)[0].decode("latin1")
        if body.startswith("ENOENT"):
            return "ENOENT"
        if body.endswith("OK") or ",p=" in body and body.endswith("OK"):
            return "OK"
        if body.endswith("OK"):
            return "OK"
    return f"sin respuesta ({buf[:60]!r})"


def transmit(path: str, img_id: int) -> None:
    send(f"a=t,f=100,t=f,q=2,i={img_id};{base64.b64encode(path.encode()).decode()}")


def place(img_id: int, quiet: bool = True, **extra: str) -> None:
    bits = f"a=p,i={img_id},p={img_id},C=1,z={extra.pop('z', 0)},c={extra.pop('c', 8)},r={extra.pop('r', 6)}"
    if extra.get("crop"):
        bits += f",x={extra['crop']},w=8"
    if quiet:
        bits += ",q=2"
    send(bits)


REPORT = "/tmp/opencode/kitty-probe-result.txt"
_lines: list[str] = []


def say(text: str) -> None:
    """El informe va a un fichero: la salida de este script la se come el pty de
    kitty (la ventana está oculta, así que no se lee por pantalla)."""
    _lines.append(text)


def flush_report() -> None:
    with open(REPORT, "w") as fh:
        fh.write("\n".join(_lines) + "\n")


def main() -> int:
    # Sin esto el pty está en modo canónico y las respuestas de kitty (que no
    # llevan salto de línea) no se pueden leer nunca: es lo que hacía que esta
    # sonda no viera nada.
    import termios
    import tty

    tty.setraw(sys.stdin.fileno())
    attrs = termios.tcgetattr(sys.stdout.fileno())
    attrs[1] &= ~termios.OPOST          # sin \r\n en la salida, por si acaso
    termios.tcsetattr(sys.stdout.fileno(), termios.TCSANOW, attrs)

    path = "/tmp/opencode/kitty-probe.png"
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as fh:
        fh.write(png_bytes())
    b64 = base64.b64encode(path.encode()).decode()
    say("kitty real: TERM=" + os.environ.get("TERM", "?"))

    # 0) ¿Contesta kitty a una *consulta*? (`a=q` es la sonda que ya usa la app)
    send(f"a=q,t=f,i=99;q=2,{b64}")
    say(f"  a=q,t=f,i=99 -> {read_reply(1.0)}")

    # 1) Colocar un id que nunca se transmitió: debe contestar ENOENT.
    place(98, quiet=False)
    say(f"  a=p con id desconocido -> {read_reply(1.0)}")

    # 2) Transmitir y colocar sin `q`: ¿contesta OK?
    transmit(path, 97)
    place(97, quiet=False)
    say(f"  a=p recién transmitido -> {read_reply(1.0)}")

    # 3) Colocar en silencio y luego preguntar (lo que hace la app al cerrar).
    send("a=d,d=i,i=97,p=97,q=2")
    place(97)
    place(97, quiet=False)
    say(f"  a=p tras esconder en silencio -> {read_reply(1.0)}")

    # 4) Y con `d=A`, que sí libera los datos.
    send("a=d,d=A,q=2")
    place(97, quiet=False)
    say(f"  a=p tras d=A -> {read_reply(1.0)}")

    flush_report()
    try:
        os.remove(path)
    except OSError:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
