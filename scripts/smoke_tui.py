#!/usr/bin/env python3
"""Exercise the real TUI in a PTY, isolating both settings and system trash."""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/basalgrim-tui").resolve()
    with tempfile.TemporaryDirectory(prefix="basalgrim-smoke-") as tmp:
        sandbox = Path(tmp)
        folder = sandbox / "files"
        folder.mkdir()
        destination = folder / "destination"
        destination.mkdir()
        (folder / "alpha.txt").write_text("Basalgrim smoke test\n")
        config, data = sandbox / "config", sandbox / "data"
        config.mkdir()
        data.mkdir()
        env = dict(os.environ, TERM="xterm-256color", XDG_CONFIG_HOME=str(config), XDG_DATA_HOME=str(data))
        env.pop("NO_COLOR", None)  # Exercise actual color updates in the isolated test terminal.
        for name in ("TERM_PROGRAM", "LC_TERMINAL", "TMUX", "WEZTERM_EXECUTABLE", "KONSOLE_VERSION"):
            env.pop(name, None)
        master, slave = pty.openpty()
        original = termios.tcgetattr(slave)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 140, 0, 0))
        child = subprocess.Popen([str(binary), str(folder)], stdin=slave, stdout=slave, stderr=slave, env=env)
        output = bytearray()

        def pump(seconds=0.15):
            end = time.monotonic() + seconds
            while time.monotonic() < end:
                if select.select([master], [], [], max(0, min(0.02, end - time.monotonic())))[0]:
                    output.extend(os.read(master, 1 << 20))

        def send(value):
            os.write(master, value.encode() if isinstance(value, str) else value)
            pump()

        def wait_for(predicate, description):
            deadline = time.monotonic() + 5
            while not predicate():
                assert child.poll() is None, f"TUI exited during {description}"
                assert time.monotonic() < deadline, f"Timed out: {description}"
                pump(0.02)

        def navigate(path):
            send(b"\x0c\x15")  # Ctrl+L, Ctrl+U
            send(str(path))
            send(b"\r")
            pump()

        def search(name):
            send("/")
            send(b"\x15")
            send(name)
            send(b"\r")

        try:
            wait_for(lambda: b"BASALGRIM" in output, "terminal graphics probe and first frame")
            start = len(output)
            send(b"\x1b[<35;50;13M")  # SGR mouse motion over an unselected file
            assert b"48;2;" in output[start:], "Mouse motion did not repaint the hover background"
            start = len(output)
            send(b"\x1b[O")  # Focus lost clears hover, without changing selection
            assert b"48;2;" in output[start:], "Focus loss did not clear hover"
            start = len(output)
            send(b"\x1b[<2;60;14M")  # SGR secondary click -> shared Actions menu
            assert "Выбрать все".encode() in output[start:], "Right click did not open the command menu"
            send(b"\x1b[<2;60;14M")  # Second right click dismisses without click-through
            search("alpha.txt")
            send(b"\x03")  # Copy
            navigate(destination)
            send(b"\x16")  # Paste
            wait_for(lambda: (destination / "alpha.txt").exists(), "copy")
            assert (folder / "alpha.txt").read_text() == (destination / "alpha.txt").read_text()

            send(b"\x1bOQ")  # F2 rename
            send(b"\x15")
            send("renamed.txt")
            send(b"\r")
            wait_for(lambda: (destination / "renamed.txt").exists(), "rename")
            assert not (destination / "alpha.txt").exists()

            search("renamed.txt")
            send(b"\x18")  # Cut
            navigate(folder)
            send(b"\x16")
            wait_for(lambda: (folder / "renamed.txt").exists(), "move")
            assert not (destination / "renamed.txt").exists()

            search("renamed.txt")
            send(b"\x1b[3~")  # Delete opens confirmation
            assert (folder / "renamed.txt").exists(), "Trash must require confirmation"
            send(b"\r")
            wait_for(lambda: not (folder / "renamed.txt").exists(), "isolated trash")
            trashed = list((data / "Trash/files").glob("renamed*"))
            assert len(trashed) == 1 and trashed[0].read_text() == "Basalgrim smoke test\n"
            assert list((data / "Trash/info").glob("*.trashinfo"))

            send(b"\x0e")  # Ctrl+N
            send("new.txt")
            send(b"\r")
            wait_for(lambda: (folder / "new.txt").exists(), "create file")

            send(b"\x07")  # Ctrl+G saves preference
            wait_for(lambda: (config / "molten-zharr/basalgrim/settings.json").exists(), "settings")
            settings_path = config / "molten-zharr/basalgrim/settings.json"
            send(b"\x1b[21~")  # F10: settings
            send(b"\t\t\x1b[B\r")  # Parameters, Classic Amber
            send(b"\t\t\x1b[C\x1b[C\t\x1b[C")  # Typography, next font
            send(b"\x1b[B\r\x15")  # Font size editor, clear
            send("22")
            send(b"\r")
            wait_for(lambda: json.loads(settings_path.read_text()).get("font_size") == 22, "font settings")
            saved = json.loads(settings_path.read_text())
            assert saved["theme"] == "classic-amber" and saved["font"] == "FiraCodeBold"
            send(b"\t\x1b[B\t\x1b[B\r")  # Menu, shortcuts, Help, capture
            send(b"\x11")  # Current quit key must be captured, not dispatched
            assert child.poll() is None
            send(b"\x1bOS")  # F4 replaces F1
            wait_for(lambda: json.loads(settings_path.read_text())["bindings"][0]["key"]["key"] == {"F": 4}, "shortcut capture")
            send(b"\x1b")  # Settings -> files
            send(b"\x1bOS")  # New help shortcut
            send(b"\x1b")  # Help -> files
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 18, 50, 0, 0))
            child.send_signal(signal.SIGWINCH)
            pump()
            send(b"\x11")  # Ctrl+Q
            child.wait(timeout=5)
            assert child.returncode == 0
            restored = termios.tcgetattr(slave)
            assert (restored[3] & (termios.ECHO | termios.ICANON)) == (original[3] & (termios.ECHO | termios.ICANON))
            child = subprocess.Popen([str(binary), str(folder)], stdin=slave, stdout=slave, stderr=slave, env=env)
            output.clear()
            wait_for(lambda: b"BASALGRIM" in output, "first frame after restart")
            send(b"\x1bOS")  # Persisted F4 opens help after restart
            assert "Управление".encode() in output
            send(b"\x1b")
            send(b"\x11")
            child.wait(timeout=5)
            assert child.returncode == 0
            print("TUI PTY smoke passed: hover/focus loss, files, isolated trash, settings, restart, resize, terminal restore")
        except BaseException:
            print(output[-3000:].decode(errors="replace"), file=sys.stderr)
            raise
        finally:
            if child.poll() is None:
                child.terminate()
                child.wait(timeout=5)
            os.close(master)
            os.close(slave)


if __name__ == "__main__":
    main()
