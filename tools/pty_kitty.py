"""Launch a Rosadeck TUI in a pty that behaves like kitty for the graphics
protocol.

What a real kitty does that a bare pty does not: answer the startup probe for
`t=f` (reading a file from disk) with `ESC_Gi=<id>;OKESC\\`, which is what lets
the covers travel as paths instead of megabytes of base64.

Everything the app writes is captured exactly as a terminal would receive it.
Set `answer=False` for a silent pty: the probe then fails and the app must fall
back to sending the bytes, which is the other path worth measuring.

Single reader: the answering thread is the only one that touches the pty and it
forwards everything to a queue, so a measurement can never race it for bytes
(that race used to make the probe fail at random).

    fd, pid = spawn(cols, rows, [binary, "rosadeck-library"])
    buf = drain(fd, 4.0)          # first frame
    os.write(fd, b"\\x1b[C")
    buf = drain(fd, 1.0)          # what one keypress produced
    stop(pid)
"""
import fcntl
import os
import pty
import queue
import re
import select
import signal
import struct
import termios
import threading
import time

STX = b"\x1b_G"
SEP = b"\x1b\\"
QUERY = re.compile(re.escape(STX) + rb"a=q,t=f,[^\x1b]*" + re.escape(SEP))

_QUEUE: "queue.Queue[bytes] | None" = None
_PUMP: "threading.Thread | None" = None
_STOP = threading.Event()


def _pump(fd, answer):
    """The one reader: forwards bytes to the queue and answers probes."""
    q = _QUEUE
    while not _STOP.is_set():
        r, _, _ = select.select([fd], [], [], 0.1)
        if not r:
            continue
        try:
            chunk = os.read(fd, 1 << 20)
        except OSError:
            break
        if not chunk:
            break
        q.put(chunk)
        if not answer:
            continue
        for m in QUERY.finditer(chunk):
            iid = re.search(rb"i=(\d+)", m.group(0))
            if not iid:
                continue
            # What kitty says when it read the file, and when it did not.
            reply = (
                b"\x1b_Gi=%s;OK\x1b\\" % iid.group(1)
                if answer
                else b"\x1b_Gi=%s;ENOENT:Failed to read image file\x1b\\" % iid.group(1)
            )
            try:
                os.write(fd, reply)
            except OSError:
                return
    q.put(b"")   # end of stream


def spawn(cols, rows, argv, env=None, answer=True):
    """Start `argv` in a pty of `cols`x`rows`. Returns (fd, pid)."""
    global _QUEUE, _PUMP, _STOP
    _STOP = threading.Event()
    _QUEUE = queue.Queue()
    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-kitty"
        os.environ["COLORTERM"] = "truecolor"
        os.environ["KITTY_WINDOW_ID"] = "1"
        for k in ("ROSADECK_NO_IMAGES", "ROSADECK_NO_SYNC"):
            os.environ.pop(k, None)
        for k, v in (env or {}).items():
            os.environ[k] = v
        os.execvp(argv[0], argv)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    _PUMP = threading.Thread(target=_pump, args=(fd, answer), daemon=True)
    _PUMP.start()
    return fd, pid


def drain(fd, seconds, buf=None):
    """Collect what the app writes for `seconds`. `buf` is appended to."""
    end = time.time() + seconds
    while time.time() < end:
        try:
            chunk = _QUEUE.get(timeout=max(0.0, end - time.time()))
        except queue.Empty:
            break
        if not chunk:
            break
        if buf is not None:
            buf.extend(chunk)
    return buf


def until_quiet(fd, limit=4.0, quiet=0.4, buf=None):
    """Collect until the app has been silent for `quiet` seconds."""
    start = time.time()
    last = None
    while time.time() - start < limit:
        try:
            chunk = _QUEUE.get(timeout=0.02)
        except queue.Empty:
            if last is not None and time.time() - last > quiet:
                break
            continue
        if not chunk:
            break
        if buf is not None:
            buf.extend(chunk)
        last = time.time()
    return time.time() - start if last else None


def stop(pid):
    try:
        os.kill(pid, signal.SIGTERM)
    except OSError:
        pass
    _STOP.set()