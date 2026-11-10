#!/usr/bin/env python3
"""Close overflowing tabs with real SGR mouse input at 34x14, isolated from the desktop."""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time

from pty_helpers import quit_when_ready
from smoke_watch import screen_text


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/release/basalgrim-tui").resolve()
    for grid in (False, True):
        with tempfile.TemporaryDirectory(prefix="basalgrim-tabs-") as tmp:
            base = Path(tmp)
            source = base / "long-directory-name-for-overflow"
            source.mkdir()
            (source / "proof.txt").write_text("unchanged")
            settings = base / "config/molten-zharr/basalgrim/settings.json"
            settings.parent.mkdir(parents=True)
            settings.write_text(json.dumps({"grid": grid, "preview": False, "icon_mode": "symbols"}))
            session = base / "state/molten-zharr/basalgrim/session.json"
            env = dict(os.environ, HOME=str(base), TERM="xterm-256color", XDG_CONFIG_HOME=str(base / "config"),
                       XDG_DATA_HOME=str(base / "data"), XDG_STATE_HOME=str(base / "state"))
            for name in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "TERM_PROGRAM", "LC_TERMINAL", "TMUX", "WEZTERM_EXECUTABLE", "KONSOLE_VERSION"):
                env.pop(name, None)
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 14, 34, 0, 0))
            child = subprocess.Popen([str(binary), str(source)], env=env, stdin=slave, stdout=slave, stderr=slave)
            output = bytearray()

            def pump(seconds=.1):
                end = time.monotonic() + seconds
                while time.monotonic() < end:
                    if select.select([master], [], [], .01)[0]:
                        output.extend(os.read(master, 65536))

            def send(data):
                os.write(master, data)
                pump()

            def screen():
                return screen_text(output, 34, 14)

            def count():
                return len(json.loads(session.read_text())["panes"][0]["tabs"]) if session.exists() else 0

            def wait(predicate, label):
                end = time.monotonic() + 8
                while not predicate():
                    assert child.poll() is None and time.monotonic() < end, label + "\n" + screen()
                    pump()

            try:
                wait(lambda: count() == 1, "initial tab")
                for _ in range(12):
                    send(b"\x14")
                wait(lambda: count() == 13, "create overflowing tabs")
                for remaining in range(13, 1, -1):
                    def tab_row():
                        return next(((y, line) for y, line in enumerate(screen().splitlines())
                                     if f"{remaining}:" in line and "×" in line), None)
                    wait(lambda: tab_row() is not None, "visible active tab and close cross")
                    y, line = tab_row()
                    assert "+" in line, "new-tab button lost on overflow"
                    x = line.index("×")
                    send(f"\x1b[<0;{x+1};{y+1}M".encode())
                    send(f"\x1b[<0;{x+1};{y+1}m".encode())
                    wait(lambda: count() == remaining - 1, "mouse closed one tab")
                wait(lambda: "1:long" in screen(), "remaining tab visible")
                row = next(line for line in screen().splitlines() if "1:long" in line)
                assert "×" not in row and "+" in row
                assert (source / "proof.txt").read_text() == "unchanged"
                quit_when_ready(child, send)
                print(f"Tab close PTY grid={grid}: 12 mouse closes at 34x14, overflow, visible plus and preserved last tab passed", flush=True)
            finally:
                if child.poll() is None:
                    child.terminate()
                    child.wait(timeout=5)
                os.close(master)
                os.close(slave)


if __name__ == "__main__":
    main()
