#!/usr/bin/env python3
"""¿Tiene esta fuente los glifos de Nerd Font? (parseo de cmap a pelo, sin fonttools)"""
import struct, sys

def tables(data):
    num = struct.unpack(">H", data[4:6])[0]
    out = {}
    for i in range(num):
        off = 12 + i * 16
        tag, _, o, l = struct.unpack(">4sIII", data[off:off+16])
        out[tag.decode()] = (o, l)
    return out

def codepoints(path):
    data = open(path, "rb").read()
    if data[:4] == b"ttcf":
        base = struct.unpack(">I", data[12:16])[0]
    else:
        base = 0
    t = tables(data)
    if "cmap" not in t:
        return set()
    off, _ = t["cmap"]
    n = struct.unpack(">H", data[off+2:off+4])[0]
    best = None
    for i in range(n):
        rec = off + 4 + i * 8
        pid, eid, sub = struct.unpack(">HHI", data[rec:rec+8])
        fmt = struct.unpack(">H", data[off+sub:off+sub+2])[0]
        if fmt in (4, 12):
            best = (fmt, off + sub)
            if fmt == 12:
                break
    if not best:
        return set()
    fmt, so = best
    cps = set()
    if fmt == 4:
        segx2 = struct.unpack(">H", data[so+6:so+8])[0]
        seg = segx2 // 2
        ends = struct.unpack(f">{seg}H", data[so+14:so+14+segx2])
        starts = struct.unpack(f">{seg}H", data[so+16+segx2:so+16+2*segx2])
        deltas = struct.unpack(f">{seg}h", data[so+16+2*segx2:so+16+3*segx2])
        ro_off = so + 16 + 3 * segx2
        ros = struct.unpack(f">{seg}H", data[ro_off:ro_off+segx2])
        for i in range(seg):
            if starts[i] == 0xFFFF:
                continue
            for c in range(starts[i], ends[i] + 1):
                if ros[i] == 0:
                    g = (c + deltas[i]) & 0xFFFF
                else:
                    gp = ro_off + i * 2 + ros[i] + (c - starts[i]) * 2
                    if gp + 2 > len(data):
                        continue
                    g = struct.unpack(">H", data[gp:gp+2])[0]
                    if g:
                        g = (g + deltas[i]) & 0xFFFF
                if g:
                    cps.add(c)
    else:
        ngroups = struct.unpack(">I", data[so+12:so+16])[0]
        for i in range(ngroups):
            g = so + 16 + i * 12
            start, end, _ = struct.unpack(">III", data[g:g+12])
            cps.update(range(start, min(end, start + 0x20000) + 1))
    return cps

WANT = {
    "nf-fa-star        U+F005": 0xF005,
    "nf-fa-play        U+F04B": 0xF04B,
    "nf-fa-gamepad     U+F11B": 0xF11B,
    "nf-fa-heart       U+F004": 0xF004,
    "nf-fa-search      U+F002": 0xF002,
    "nf-fa-home        U+F015": 0xF015,
    "nf-seti-snes      U+E7AD": 0xE7AD,
    "nf-mdi-*          U+F0000": 0xF0000,
    "nf-oct-game       U+EAF5": 0xEAF5,
    "box drawing U+2502": 0x2502,
    "star  U+2605": 0x2605,
    "tri   U+25B8": 0x25B8,
    "dia   U+25C6": 0x25C6,
    "check U+2713": 0x2713,
}
for path in sys.argv[1:]:
    cps = codepoints(path)
    have = [n for n, c in WANT.items() if c in cps]
    print(f"{path.split('/')[-1]}: {len(cps)} glifos")
    for n in WANT:
        print(f"   {'sí' if WANT[n] in cps else 'NO'}  {n}")
