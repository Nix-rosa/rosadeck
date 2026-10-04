#!/usr/bin/env python3
"""Minimal VT emulator for the escape sequences Rosadeck emits.

Only what the TUIs use: cursor positioning, SGR colours, ED (erase display) and
EL (erase line). Kitty graphics payloads (`ESC _ ... ESC \\`) are skipped, so a
screen can be reconstructed exactly as the text layer would look.

Used by `vt_compare.py` and handy for probes: feed the byte stream of a real pty
session and print the screen.
"""
import codecs

class Screen:
    """Cells hold (char, fg, bold): exactly what the app relies on."""

    def __init__(self, cols, rows):
        self.cols, self.rows = cols, rows
        self.buf = [[(" ", "d", False)] * cols for _ in range(rows)]
        self.cx = self.cy = 0
        self.fg = self.bold = self.reverse = "d"

    def fit(self):
        while len(self.buf) < self.rows:
            self.buf.append([(" ", "d", False)] * self.cols)
        self.buf = self.buf[: self.rows]
        for row in self.buf:
            row.extend([(" ", "d", False)] * (self.cols - len(row)))
            del row[self.cols:]

    def put(self, ch):
        self.cx = min(self.cx, self.cols - 1)
        self.buf[self.cy][self.cx] = (ch, self.fg, self.bold or self.reverse)
        self.cx += 1

    def erase_down(self):
        for y in range(self.cy, self.rows):
            for x in range(self.cx if y == self.cy else 0, self.cols):
                self.buf[y][x] = (" ", self.fg, False)

    def erase_all(self):
        self.buf = [[(" ", self.fg, False)] * self.cols for _ in range(self.rows)]
        self.cx = self.cy = 0

    def dump(self):
        return "\n".join("|".join(f"{c}:{fg}{'+' if b else ''}" for c, fg, b in row) for row in self.buf)


def to_text(data):
    """UTF-8 decode a byte stream incrementally.

    Terminals advance one *cell* per character, so a 3-byte box-drawing glyph is
    one column, not three: decoding per byte would model the screen wrong and
    hide (or invent) real rendering bugs.
    """
    if isinstance(data, str):
        return data
    decoder = codecs.getincrementaldecoder("utf-8")("replace")
    return decoder.decode(bytes(data), False)


def feed(scr, data):
    """Replay a byte or text stream."""
    data = to_text(data)
    i, n = 0, len(data)
    while i < n:
        ch = data[i]
        if ch == "\x1b":
            two = data[i:i + 2]
            if two == "\x1b[":
                j = i + 2
                while j < n and not ("\x40" <= data[j] <= "\x7e"):
                    j += 1
                if j >= n:
                    break
                seq, final = data[i + 2:j], data[j]
                i = j + 1
                if final == "H":
                    parts = [p for p in seq.split(";") if p]
                    scr.cy = max(0, int(parts[0]) - 1) if parts else 0
                    scr.cx = max(0, int(parts[1]) - 1) if len(parts) > 1 else 0
                    scr.fit()
                elif final == "J":
                    mode = int(seq) if seq.isdigit() else 0
                    scr.erase_all() if mode == 2 else scr.erase_down()
                elif final == "K":
                    for x in range(scr.cx, scr.cols):
                        scr.buf[scr.cy][x] = (" ", scr.fg, False)
                elif final == "m":
                    if seq in ("", "0"):
                        scr.fg, scr.bold, scr.reverse = "d", False, False
                    for part in seq.split(";"):
                        if part == "1":
                            scr.bold = True
                        elif part == "7":
                            scr.reverse = True
                        elif part.startswith("38;2;"):
                            scr.fg = part[5:]
                        elif part.startswith("48;2;"):
                            pass
                        elif part.startswith("38;5;"):
                            scr.fg = f"256:{part[5:]}"
                continue
            if two in ("\x1b_", "\x1b]"):
                end, bell = data.find("\x1b\\", i), data.find("\x07", i)
                if bell != -1 and (end == -1 or bell < end):
                    i = bell + 1
                elif end != -1:
                    i = end + 2
                else:
                    break
                continue
            i += 2
            continue
        if ch == "\r":
            scr.cx = 0
        elif ch == "\n":
            scr.cy = min(scr.cy + 1, scr.rows - 1)
            scr.fit()
        elif ch == "\x07":
            pass
        else:
            scr.put(ch)
        i += 1




def show(screen, max_rows=None):
    """Human-readable screen dump (column ruler + rows)."""
    out = []
    out.append("    " + "".join(str(i // 10 % 10) for i in range(screen.cols)))
    out.append("    " + "".join(str(i % 10) for i in range(screen.cols)))
    for i, row in enumerate(screen.buf):
        if max_rows and i >= max_rows:
            break
        line = "".join(str(c) for c, _, _ in row).rstrip()
        out.append(f"{i:>3} |{line}")
    return "\n".join(out)


def replay(stream, cols, rows):
    """Screen contents after replaying `stream` (bytes or str)."""
    scr = Screen(cols, rows)
    feed(scr, stream)
    return scr
