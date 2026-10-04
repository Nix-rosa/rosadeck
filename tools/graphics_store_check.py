#!/usr/bin/env python3
"""¿Siguen en pie las portadas? Modelo del almacén de imágenes de kitty.

Contar `a=p` no demuestra nada: una colocación puede apuntar a una imagen que
kitty ya no tiene (justo lo que pasaba al cerrar la ventana de directorios, tras
un `a=d,d=A` que libera los datos). Aquí se lleva el flujo de escape de la app a
un modelo del protocolo y se comprueba lo que el terminal mostraría de verdad:

  data   = ids-transmitidos-vivos      (a=t / a=T; `a=d,d=A` los borra todos)
  placed = colocaciones-vivas          (a=p las crea; `a=d,d=i` quita UNA)

Regla de oro: una colocación cuyo id no está en `data` no pinta nada, aunque se
emita. Eso es un fallo aunque el frame y el texto seanperfectos.

Y la otra regla, la que costó dos vueltas de diagnóstico: **`\x1b[2J` también borra los
datos**. El spec dice que el clear screen "should also clear all images", y en
kitty real se comprobó: después de un `[2J` un `a=p` responde `ENOENT`. Por eso
este modelo limpia las dos cosas, y por eso `rosadeck-library` tiene que
re-transmitir tras cada repintado completo (`repaint_all`).

Uso:  python3 tools/graphics_store_check.py [binario]
"""
from __future__ import annotations

import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pty_kitty  # noqa: E402

# `\x1b_G<clave=valor,...>\x1b\\` y también `\x1b_G...\x1b/` (ST Alternate).
# El cuerpo lleva `;` delante del payload, así que se corta por ahí.
APC = re.compile(rb"\x1b_G(.*?)(?:\x1b\\|\x1b/)", re.S)
KV = re.compile(rb"([a-zA-Z]+)=([^,;]+)")


class Store:
    """Lo que kitty debería tener, según el flujo que le hemos mandado."""

    def __init__(self) -> None:
        self.data: set[int] = set()
        self.placed: dict[int, tuple[int, int]] = {}
        self.dangling: list[int] = []          # a=p sobre datos inexistentes
        self.clears = 0                        # `[2J` que ha visto

    def feed(self, raw: bytes) -> None:
        # `[2J` (el pintor por diferencias lo manda en cada repintado completo)
        # limpia imágenes *y* datos: por spec, y comprobado contra kitty real.
        if b"\x1b[2J" in raw:
            self.placed.clear()
            self.data.clear()
            self.clears += 1
        for body in APC.findall(raw):
            kv = dict(KV.findall(body.split(b";", 1)[0]))
            action = kv.get(b"a")
            if action == b"t" or action == b"T":
                # `a=t` también: transmitiendo por ruta (t=f), el id cuenta.
                if b"i" in kv:
                    self.data.add(int(kv[b"i"]))
                else:
                    self.data.add(-1)         # id asignado por kitty
            elif action == b"p":
                i = int(kv.get(b"i", b"-1"))
                c, r = int(kv.get(b"c", b"1")), int(kv.get(b"r", b"1"))
                if i not in self.data and i != -1:
                    self.dangling.append(i)
                self.placed[i] = (c, r)
            elif action == b"d":
                scope = kv.get(b"d")
                if scope == b"A":
                    self.placed.clear()
                    self.data.clear()
                elif scope == b"N":
                    self.data.clear()
                elif scope in (b"i", b"f", b"I", b"F"):
                    for key in (b"i", b"p"):
                        if key in kv:
                            self.placed.pop(int(kv[key]), None)
                    # `i` quita la colocación, `f` el fichero: los datos quedan.
                else:
                    for key in (b"i", b"p"):
                        if key in kv:
                            self.placed.pop(int(kv[key]), None)

    def check(self, label: str, want_placed: int | None = None) -> bool:
        ok = True
        if self.dangling:
            print(f"  FALLO {label}: {len(self.dangling)} colocaciones sin datos {self.dangling[:6]}")
            ok = False
        if want_placed is not None and len(self.placed) != want_placed:
            print(f"  FALLO {label}: {len(self.placed)} portadas en pantalla, se esperaban {want_placed}")
            ok = False
        print(
            f"  {'ok  ' if ok else 'mal '} {label}: {len(self.placed)} colocadas · {len(self.data)} con datos"
            f" · {self.clears} [2J"
        )
        return ok


def self_test() -> int:
    """El modelo tiene que saber ver el fallo que causadas (dientes)."""
    def g(*cmds: bytes) -> bytes:
        return b"".join(b"\x1b_G" + c + b"\x1b\\" for c in cmds)

    cases = [
        # El bug de v26: `d=A` libera los datos y luego se recoloca sin enviar.
        ("recolocar sin datos", g(b"a=t,i=1,q=2", b"a=d,d=A,q=2", b"a=p,i=1,q=2"), 1, True),
        # Lo correcto: `d=i` quita la colocación y el dato sigue ahí.
        (
            "quitar colocación y reponer",
            g(b"a=t,i=1,q=2", b"a=p,i=1,q=2", b"a=d,d=i,i=1,p=1,q=2", b"a=p,i=1,q=2"),
            1,
            False,
        ),
        ("borrar datos sueltos", g(b"a=t,i=1,q=2", b"a=p,i=1,q=2", b"a=d,d=N,q=2", b"a=p,i=1,q=2"), 1, True),
        ("transmitir por ruta", g(b"a=t,f=100,t=f,q=2,i=1;AAAA", b"a=p,i=1,q=2"), 1, False),
        # El caso real de v26: `[2J` (repintado completo) y luego colocar sin
        # reenviar. El spec dice que el clear screen limpia también los datos.
        ("[2J y colocar sin reenviar", b"\x1b[2J" + g(b"a=p,i=1,p=1,q=2"), 1, True),
    ]
    bad = 0
    for label, stream, want_placed, want_dangling in cases:
        st = Store()
        st.feed(stream)
        good = (bool(st.dangling) == want_dangling) and len(st.placed) == want_placed
        print(f"  {'ok  ' if good else 'mal '} {label}: {len(st.placed)} colocadas, {len(st.dangling)} sin datos")
        bad += not good
    print("RESULTADO:", "ok" if not bad else "FALLO")
    return 1 if bad else 0


def main() -> int:
    binary = sys.argv[1] if len(sys.argv) > 1 else "./target/release/rosadeck-library"
    fd, pid = pty_kitty.spawn(140, 34, [binary, "rosadeck-library"])
    buf = bytearray()
    store = Store()

    def press(
        data: bytes,
        pause: float = 0.7,
        label: str = "",
        want_placed: int | None = None,
        retransmit: bool = False,
    ) -> bool:
        """Escribe, deja pintar, mete lo nuevo en el modelo y comprueba.

        `retransmit=True` dice que este repintado tiene que **reenviar** las
        portadas: tras un `[2J` kitty ya no las tiene (comprobado en kitty real:
        responde `ENOENT` a un `a=p` sin payload).
        """
        mark = len(buf)
        os.write(fd, data)
        pty_kitty.drain(fd, pause, buf)
        chunk = bytes(buf[mark:])
        store.feed(chunk)
        good = store.check(label or repr(data), want_placed=want_placed)
        sent = len(re.findall(rb"a=t,[^\x1b]*", chunk))
        if retransmit and shelf and sent == 0:
            print(f"  FALLO {label}: tras el [2J] no se reenvió ninguna portada ({sent} envíos)")
            good = False
        return good

    pty_kitty.drain(fd, 4.0, buf)
    store.feed(bytes(buf))
    # La biblioteca real tiene 4 carátulas y 3 carátulas generadas (bloques).
    shelf = len(store.placed)
    print(f"arranque: {shelf} portadas colocadas, {len(store.data)} con datos")
    if shelf == 0 or store.dangling:
        print("FALLO: el estante no arrancó con portadas reales")
        pty_kitty.stop(pid)
        return 1

    ok = store.check("estante abierto", want_placed=shelf)
    # Abrir la ventana repinta (=[2J=) y cierra: el estante vuelve entero, con
    # los datos re-transmitidos porque kitty los perdió con el clear.
    ok &= press(b"d", 0.9, "ventana abierta (estante vacío)", want_placed=0)
    ok &= press(b"\x1b", 0.9, "ventana cerrada (estante de vuelta)", want_placed=shelf, retransmit=True)
    for i in range(3):
        ok &= press(b"d", 0.5, f"ciclo {i + 1}: abierta", want_placed=0)
        ok &= press(b"\x1b", 0.5, f"ciclo {i + 1}: cerrada", want_placed=shelf, retransmit=True)
    ok &= press(b"d", 0.5, "ventana abierta escribiendo", want_placed=0)
    ok &= press(b"/tmp", 0.5, "ruta a medio escribir", want_placed=0)
    ok &= press(b"\x7f\x7f\x7f", 0.5, "borrando", want_placed=0)
    ok &= press(b"\x1b", 0.7, "cancelada", want_placed=shelf, retransmit=True)
    # Moviendo el cursor entra una portada nueva: lo que no puede pasar es que
    # aparezca una colocación sin datos (eso sí es un fallo).
    ok &= press(b"\x1b[C", 0.5, "moviendo el cursor")

    if not shelf <= len(store.placed) <= shelf + 2:
        print(f"  FALLO: el estante quedó en {len(store.placed)} portadas,no cerca de {shelf}")
        ok = False
    pty_kitty.stop(pid)
    print("RESULTADO:", "ok" if ok else "FALLO")
    return 0 if ok else 1


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.exit(self_test())
    sys.exit(main())
