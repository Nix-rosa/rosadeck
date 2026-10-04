#!/usr/bin/env python3
"""End-to-end launch check: drive the real library TUI in a pty, press Enter on
the selected game, and verify the emulator process actually starts.

Usage: pty_launch_check.py <emulator-comm> [keys-before-enter]

`keys-before-enter` are raw bytes sent first (e.g. "5" to filter 3DS, "2" for
Nintendo 64, "1" for SNES), so the cursor sits on the right game before Enter.

`emulator-comm` is matched against the process *name* (pgrep -x), never the full
command line: a `pkill -f` on an emulator name also matches the shell that
launched this script, which kills the very session running the check.

Exit 0 = the emulator process appeared with a ROM from ~/roms in its command
line, so "the games launch" is verified end to end, not merely described.
"""
import os, pty, fcntl, termios, struct, select, signal, subprocess, sys, time, re

# CSI/OSC escapes: enough to read the status line out of a raw-mode frame.
ESC = re.compile(rb"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b[@-Z\\-_]|\x1b\][^\x07]*\x07")

comm = sys.argv[1] if len(sys.argv) > 1 else "snes9x-gtk"
pre_keys = (sys.argv[2] if len(sys.argv) > 2 else "").encode()
roms = os.path.expanduser("~/roms")


seen = bytearray()


def drain(fd, seconds):
    """Read whatever the TUI/emulator writes for `seconds`, returning bytes."""
    total = 0
    end = time.time() + seconds
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try:
                chunk = os.read(fd, 65536)
            except OSError:
                break
            if not chunk:
                break
            seen.extend(chunk)
            total += len(chunk)
    return total


def emulator_lines():
    """Command lines of emulator processes (name match, never self)."""
    out = subprocess.run(["pgrep", "-x", comm], capture_output=True, text=True).stdout
    lines = []
    for pid in out.split():
        try:
            cmdline = open(f"/proc/{pid}/cmdline", "rb").read().replace(b"\0", b" ").decode(errors="replace")
        except OSError:
            continue
        if roms in cmdline:
            lines.append(cmdline.strip())
    return lines


pid, fd = pty.fork()
if pid == 0:
    # El proyecto puede estar en cualquier sitio: se usa el de este repo.
    os.chdir(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    os.environ["TERM"] = "xterm-kitty"
    os.environ["ROSADECK_NO_IMAGES"] = "1"
    os.execvp("./target/debug/rosadeck-library", ["rosadeck-library"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 46, 190, 0, 0))

drain(fd, 1.5)          # first frame
if pre_keys:
    os.write(fd, pre_keys)
    drain(fd, 0.6)
os.write(fd, b"\r")     # Enter on the selected game
found = None
end = time.time() + 14
while time.time() < end:
    time.sleep(0.8)
    hits = emulator_lines()
    if hits:
        found = hits[0]
        break
drain(fd, 0.3)
os.kill(pid, signal.SIGTERM)          # close the browser
time.sleep(0.8)
for pid_str in subprocess.run(["pgrep", "-x", comm], capture_output=True, text=True).stdout.split():
    try:
        os.kill(int(pid_str), signal.SIGTERM)
    except (ProcessLookupError, ValueError):
        pass

if found:
    print(f"OK  proceso del emulador: {found}")
    sys.exit(0)
print(f"FALLO  no apareció ningún proceso '{comm}' con un ROM de ~/roms en la línea de órdenes")
# Show what the browser said instead, so a failure is diagnosable: the status
# line is the honest reason (e.g. PREPARE_FAILED + package hint).
text = ESC.sub(b" ", bytes(seen)).decode(errors="replace")
flat = " ".join(text.split())
for keyword in ("PREPARE_FAILED", "binary not found", "pacman -S", "NOT INSTALLED", "needs emulator", "did not start"):
    i = flat.rfind(keyword)
    if i >= 0:
        print(f"      navegador: ...{flat[max(0, i - 60):i + 160]}")
        break
else:
    print(f"      navegador: ...{flat[-260:]}")
sys.exit(1)
