"""Test demo mode in a PTY without an API key or Google requests."""
import fcntl
import os
from pathlib import Path
import pty
import select
import struct
import subprocess
import sys
import termios
import time


binary = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else Path(__file__).resolve().parents[1] / "target/debug/gfs"
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
environment = {key: value for key, value in os.environ.items() if key not in {
    "GEMINI_FILE_SEARCH_API_KEY", "GEMINI_UPLOAD_ROOT", "GEMINI_FILE_SEARCH_MODEL"
}}
environment.update(TERM="xterm-256color", LANG="en_US.UTF-8")


def controlling_terminal():
    os.setsid()
    fcntl.ioctl(0, termios.TIOCSCTTY, 0)


process = subprocess.Popen([str(binary), "--demo"], stdin=slave, stdout=slave,
                           stderr=slave, env=environment, preexec_fn=controlling_terminal,
                           close_fds=True)
os.close(slave)
captured = bytearray()


def expect(text, timeout=8):
    target = text.encode()
    end = time.monotonic() + timeout
    while target not in captured:
        remaining = end - time.monotonic()
        if remaining <= 0:
            raise AssertionError(f"Did not see {text!r}. Terminal output tail: {bytes(captured[-4000:])!r}")
        if select.select([master], [], [], min(remaining, 0.1))[0]:
            try:
                data = os.read(master, 65536)
            except OSError:
                data = b""
            if not data:
                raise AssertionError(f"App exited before {text!r}: {process.poll()}")
            captured.extend(data)


try:
    expect("OFFLINE DEMO")
    expect("guide.md")
    os.write(master, b"a")
    expect("Enter: Submit")
    os.write(master, b"What does the guide specify?\r")
    # The terminal can omit unchanged characters from its output.
    # Use the new source citation to identify the answer.
    expect("This is a sample source citation.")
    expect("SOURCES")
    os.write(master, b"?")
    expect("KEYBOARD SHORTCUTS")
    # Use Enter to close the key list.
    os.write(master, b"\rq")
    # Read the output until the app closes.
    # A full buffer can prevent terminal recovery.
    expect("\x1b[?1049l", timeout=5)
    end = time.monotonic() + 5
    while process.poll() is None and time.monotonic() < end:
        if select.select([master], [], [], 0.1)[0]:
            try:
                data = os.read(master, 65536)
            except OSError:
                break
            if not data:
                break
            captured.extend(data)
    assert process.wait(timeout=5) == 0
    # Read the last terminal recovery commands.
    while select.select([master], [], [], 0.1)[0]:
        try:
            data = os.read(master, 65536)
        except OSError:
            break
        if not data:
            break
        captured.extend(data)
    assert b"\x1b[?1049l" in captured, "The app did not restore the terminal screen."
    assert b"\x1b[?2004l" in captured, "The app did not disable bracketed paste."
    print("The demo and terminal recovery checks passed.")
finally:
    os.close(master)
    if process.poll() is None:
        process.kill()
        process.wait(timeout=5)
