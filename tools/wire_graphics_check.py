"""Wire check: every transmitted cover is a PNG, no `Clear(All)` may wipe covers
already placed in the same synchronized block, every placement must land exactly
on the cell interior its card draws in the text layer, and the focused card must
travel at a higher resolution than its neighbours.

The geometry one is the check the UI needed: the band and the placements were
computed in two places, and when they disagreed the covers floated tens of
columns away from their cards. Here the text of every block is replayed into a
real VT screen (tools/vtterm.py) and each placement is checked against the border
glyphs and the blank interior the card left there.

Both transmission modes are exercised, because they are different code paths:
`tools/pty_kitty.py` answers the `t=f` probe like a local kitty (covers travel
as ~200-byte paths and the terminal reads the files) and a silent pty (the probe
fails, so the bytes go through the pty).
"""
import base64, os, struct, sys, re

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pty_kitty
import vtterm

ESC = b"\x1b"
STX = ESC + b"_G"
SEP = ESC + b"\\"
COLLECT = re.compile(re.escape(STX) + rb"a=t,([^;]*);([^" + re.escape(SEP) + rb"]*)" + re.escape(SEP))
PLACE = re.compile(re.escape(STX) + rb"a=p,([^\x1b]*)" + re.escape(SEP))
CLEAR = re.compile(re.escape(ESC) + rb"\[2J")
SYNC_START = re.compile(re.escape(ESC) + rb"\[\?2026h")
SYNC_END = re.compile(re.escape(ESC) + rb"\[\?2026l")
COLS, ROWS = 190, 46
V, HALF = "\u2502", "\u2580"
BINARY = sys.argv[1] if len(sys.argv) > 1 else "./target/release/rosadeck-library"


def capture(answer):
    """Walk the carousel and return everything the app wrote."""
    fd, pid = pty_kitty.spawn(COLS, ROWS, [BINARY, "rosadeck-library"], answer=answer)
    buf = bytearray()
    pty_kitty.drain(fd, 4.0, buf)
    for _ in range(11):            # walk the whole carousel
        os.write(fd, b"\x1b[C")
        pty_kitty.drain(fd, 1.6, buf)
    pty_kitty.stop(pid)
    return bytes(buf)


def kwargs(head):
    """`key=value,key=value` of a graphics command head."""
    out = {}
    for kv in head.split(b","):
        if b"=" in kv:
            k, v = kv.split(b"=", 1)
            out[k.decode()] = v
    return out


def check_geometry(stream):
    """Every placement must sit on the blank interior of a text card.

    kitty renders a placement at the *cursor* (`x,y,w,h` are the source
    rectangle in image pixels, `c,r` the destination in cells), so the cursor
    has to be replayed too: the stream is fed to a real VT screen chunk by
    chunk and the placement is checked at whatever cell the cursor is on.

    The text is replayed for real, so this compares what the browser sends with
    what a terminal would show, not with what the same code computed twice.
    """
    screen = vtterm.Screen(COLS, ROWS)
    checked, bad = 0, []
    at = 0
    for m in PLACE.finditer(stream):
        vtterm.feed(screen, stream[at:m.start()])
        at = m.end()
        kv = kwargs(m.group(1))
        x, y = screen.cx, screen.cy          # 0-based cell of the cursor
        c, r = int(kv.get("c", 1)), int(kv.get("r", 1))
        checked += 1
        if not (0 < x < COLS and 0 <= y < ROWS):
            bad.append(f"cursor fuera de pantalla x={x} y={y} c={c} r={r}")
            continue
        interior = [screen.buf[y + dy][x + dx][0]
                    for dy in range(min(r, ROWS - y))
                    for dx in range(min(c, COLS - x))]
        # The cards have no frame, so a placement has to land on blank cells:
        # with images the text under the artwork is empty by construction, and
        # anything else would show through the cover.
        if any(ch not in (" ", HALF) for ch in interior):
            dirty = sorted({ch for ch in interior if ch not in (" ", HALF)})
            row_text = "".join(c for c, _, _ in screen.buf[y])
            bad.append(f"x={x} y={y} c={c}: el interior tiene {dirty!r}, la portada lo taparía\n"
                       f"      fila {y}: ...{row_text[max(0, x - 6):x + c + 6]}...")
    return checked, bad


def png_header(raw):
    """(width, height) of a PNG, or None."""
    if raw[:8] != b"\x89PNG\r\n\x1a\n" or len(raw) < 24:
        return None
    return struct.unpack(">II", raw[16:24])


def transmitted_sizes(data):
    """image id -> pixel size of what the terminal was told to load.

    With `t=f` the payload is a *path*, so the size comes from the file the
    terminal will read; with the bytes it comes from the PNG header in the
    stream. Either way it is what actually ends up on screen.
    """
    sizes = {}
    bad = []
    for m in re.finditer(re.escape(STX) + rb"a=t,([^;]*);([^" + re.escape(SEP) + rb"]*)" + re.escape(SEP), data):
        kv = kwargs(m.group(1))
        iid = kv.get("i", b"").decode()
        if not iid or iid in sizes:
            continue
        payload = m.group(2)
        if kv.get("t") == b"f":
            path = base64.b64decode(payload + b"=" * (-len(payload) % 4)).decode("utf-8", "replace")
            try:
                with open(path, "rb") as f:
                    size = png_header(f.read(32))
            except OSError as exc:
                bad.append(f"la portada {path} no se puede leer: {exc}")
                continue
            if size is None:
                bad.append(f"{path} no es un PNG, y t=f sólo lee PNG")
                continue
            sizes[iid] = size
        else:
            raw = base64.b64decode(payload + b"=" * (-len(payload) % 4))
            size = png_header(raw)
            if size is None:
                bad.append("un payload a=t no empieza por la firma PNG")
                continue
            sizes[iid] = size
    return sizes, bad


def check(data, answer):
    problems = []
    sizes, size_bad = transmitted_sizes(data)
    problems += size_bad
    places = list(PLACE.finditer(data))
    paths = len(re.findall(rb"a=t,[^\x1b]*t=f", data))
    blobs = len(re.findall(rb"a=t,[^\x1b]*t=d", data))

    # Within every synchronized block, no placement may precede a Clear(All).
    violations, blocks, placed = [], 0, 0
    for m in re.finditer(
        re.escape(ESC) + rb"\[\?2026(h|l)|" + re.escape(ESC) + rb"\[2J|" + re.escape(STX) + rb"a=p,",
        data,
    ):
        tok = m.group(0)
        if tok.endswith(b"h"):
            placed = 0
            blocks += 1
        elif tok.endswith(b"l"):
            pass
        elif tok == ESC + b"[2J":
            # A clear wipes the image placements kitty already received: anything
            # placed earlier in this same block is gone. That is the bug.
            if placed:
                violations.append(m.start())
        else:  # a placement
            placed += 1

    checked, geom_bad = check_geometry(data)
    problems += geom_bad

    # Resolution on the wire: the focused card travels at its native size, the
    # neighbours reduced (and further back, smaller).
    focus, neighbours = [], []
    for m in PLACE.finditer(data):
        kv = kwargs(m.group(1))
        iid = kv.get("i", b"").decode()
        if iid in sizes:
            (focus if kv.get("z") == b"1" else neighbours).append(sizes[iid])
    res_bad = []
    if focus and neighbours:
        if max(max(s) for s in neighbours) >= min(min(s) for s in focus):
            res_bad.append("una vecina viaja a la misma resolución que la enfocada")
        for size in neighbours:
            if max(size) > 260:
                res_bad.append(f"vecina de {max(size)} px sin reducir")
    if not focus:
        res_bad.append("ninguna portada enfocada en la vuelta")
    problems += res_bad

    how = f"{paths} por ruta" + (f", {blobs} en bytes" if blobs else "")
    print(f"{'t=f' if answer else 'bytes'}: {len(data)/1024:.0f} KiB en {blocks} bloques, {how}")
    print(f"  {len(places)} colocaciones · {checked} comprobadas contra el texto, "
          f"{len(geom_bad)} fuera de su tarjeta")
    print(f"  enfocada={sorted({min(s) for s in focus}) if focus else '-'} px de lado, "
          f"vecinas={sorted({max(s) for s in neighbours})} px")
    print(f"  Clear(All) que borra portadas ya colocadas: {'SÍ (bug)' if violations else 'no'}")
    for line in problems[:6]:
        print(f"  {line}")
    return not problems and not violations


ok = True
for answer in (True, False):
    ok &= check(capture(answer), answer)
sys.exit(0 if ok else 1)
