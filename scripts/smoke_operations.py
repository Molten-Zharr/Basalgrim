#!/usr/bin/env python3
"""Exercise a real cancellable TUI copy with isolated sparse input and settings."""
import fcntl
import os
from pathlib import Path
import pty
import select
import subprocess
import struct
import sys
import tempfile
import termios
import time


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/basalgrim-tui").resolve()
    with tempfile.TemporaryDirectory(prefix="basalgrim-operations-") as tmp:
        root = Path(tmp)
        source, destination = root / "source", root / "destination"
        source.mkdir()
        destination.mkdir()
        large = source / "large.bin"
        with large.open("wb") as file:
            file.truncate(1024 * 1024 * 1024)
        config = root / "config"
        config.mkdir()
        env = dict(os.environ, TERM="xterm-256color", XDG_CONFIG_HOME=str(config))
        for name in ("TERM_PROGRAM", "LC_TERMINAL", "TMUX", "WEZTERM_EXECUTABLE", "KONSOLE_VERSION", "NO_COLOR"):
            env.pop(name, None)
        master, slave = pty.openpty()
        original = termios.tcgetattr(slave)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 36, 140, 0, 0))
        child = subprocess.Popen([str(binary), str(source)], stdin=slave, stdout=slave, stderr=slave, env=env)
        output = bytearray()

        def pump(seconds=0.02):
            end = time.monotonic() + seconds
            while time.monotonic() < end:
                if select.select([master], [], [], max(0, min(0.005, end - time.monotonic())))[0]:
                    output.extend(os.read(master, 1 << 20))

        def wait_for(predicate, description):
            deadline = time.monotonic() + 5
            while not predicate():
                assert child.poll() is None, f"TUI exited: {description}"
                assert time.monotonic() < deadline, f"Timeout: {description}"
                pump(0.005)

        def send(data):
            os.write(master, data)
            pump()

        try:
            wait_for(lambda: b"BASALGRIM" in output and b"large.bin" in output, "initial directory")
            send(b"\x03")  # Copy
            send(b"\x0c\x15")  # Path, clear
            send(str(destination).encode())
            send(b"\r")
            output.clear()
            os.write(master, b"\x16\x1b[17~")  # Paste, F6 -> operations
            wait_for(lambda: any(path.stat().st_size > 0 for path in destination.glob(".basalgrim-copy-*/item") if path.exists()), "copy checkpoint")
            send(b"\x1b[17;5~")  # Ctrl+F6 -> cancel
            wait_for(lambda: "Отменено".encode() in output, "cancelled report")
            assert not (destination / "large.bin").exists(), "Partial output was published"
            assert not list(destination.glob(".basalgrim-copy-*")), "Staging survived cancellation"
            assert large.stat().st_size == 1024 * 1024 * 1024
            assert "Операции".encode() in output
            assert "Готово 0/1".encode() in output
            send(b"\x1b")  # Back; do a small real copy afterwards.
            small = source / "small.txt"
            small.write_text("after cancellation")
            send(b"\x0c\x15")
            send(str(source).encode())
            send(b"\r")
            send(b"/small.txt\r\x03")
            send(b"\x0c\x15")
            send(str(destination).encode())
            send(b"\r\x16")
            wait_for(lambda: (destination / "small.txt").exists(), "copy after cancelled operation")
            assert (destination / "small.txt").read_text() == "after cancellation"
            send(b"\x1b[17~")
            wait_for(lambda: "Завершено".encode() in output, "completed history")
            send(b"\x11")
            child.wait(timeout=5)
            assert child.returncode == 0
            restored = termios.tcgetattr(slave)
            assert (restored[3] & (termios.ECHO | termios.ICANON)) == (original[3] & (termios.ECHO | termios.ICANON))
            print("Operations PTY passed: byte copy, cancel, staging cleanup, source preservation, history, next copy, terminal restore")
        except BaseException:
            print(output[-2500:].decode(errors="replace"), file=sys.stderr)
            raise
        finally:
            if child.poll() is None:
                child.terminate()
                child.wait(timeout=5)
            os.close(master)
            os.close(slave)


if __name__ == "__main__":
    main()
