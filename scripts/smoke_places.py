#!/usr/bin/env python3
"""Personal places in an isolated 34x14 PTY with native mouse and external opener."""
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
from places_fixture import prepare, rows, hidden_bookmarks
from pty_helpers import quit_when_ready
from smoke_watch import screen_text


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/release/basalgrim-tui").resolve()
    for grid in (False, True):
        with tempfile.TemporaryDirectory(prefix="basalgrim-places-") as tmp:
            base = Path(tmp)
            root = prepare(base)
            settings = base / "config/molten-zharr/basalgrim/settings.json"
            settings.parent.mkdir(parents=True)
            settings.write_text(json.dumps({"grid": grid, "preview": False, "icon_mode": "symbols"}))
            env = dict(os.environ, HOME=str(base), TERM="xterm-256color", XDG_CONFIG_HOME=str(base / "config"),
                       XDG_DATA_HOME=str(base / "data"), XDG_STATE_HOME=str(base / "state"),
                       PATH=str(base / "bin") + os.pathsep + os.environ["PATH"], BASALGRIM_OPEN_LOG=str(base / "opened.txt"))
            for name in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "TERM_PROGRAM", "LC_TERMINAL", "TMUX", "WEZTERM_EXECUTABLE", "KONSOLE_VERSION"):
                env.pop(name, None)
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 14, 34, 0, 0))
            child = subprocess.Popen([str(binary), "--no-session", str(root)], env=env, stdin=slave, stdout=slave, stderr=slave)
            output = bytearray()

            def pump(seconds=.1):
                end = time.monotonic() + seconds
                while time.monotonic() < end:
                    if select.select([master], [], [], .01)[0]:
                        output.extend(os.read(master, 65536))

            def send(data):
                os.write(master, data.encode() if isinstance(data, str) else data)
                pump()

            def screen(): return screen_text(output, 34, 14)

            def wait(predicate, label):
                end = time.monotonic() + 8
                while not predicate():
                    assert child.poll() is None and time.monotonic() < end, label + "\n" + screen()
                    pump()
                pump(.15)

            def click(x, y):
                send(f"\x1b[<0;{x+1};{y+1}M")
                send(f"\x1b[<0;{x+1};{y+1}m")

            try:
                wait(lambda: "note.txt" in screen(), "initial files")
                pump(.3)
                send(b"\x1b\x06")
                wait(lambda: len(rows(base, 1)) == 1, "favorite shortcut")
                send(b"\x02")
                wait(lambda: "Закладки" in screen(), "places shortcut")
                assert "Imported folder" in screen() and "DUPLICATE" not in screen() and "[GTK]" not in screen(), screen()
                click(5, 11)
                wait(lambda: len(rows(base, 0)) == 1, "compact add bookmark")
                click(4, 2); click(20, 11)
                wait(lambda: "Название места" in screen(), "compact label button")
                send(b"\x15"); send("Рабочая папка"); send(b"\r")
                wait(lambda: rows(base, 0)[0][1] == "Рабочая папка", "Unicode bookmark label")
                click(4, 3); click(23, 10)
                wait(lambda: bytes(base / "Imported") in hidden_bookmarks(base), "remove imported bookmark")
                click(15, 12)
                wait(lambda: "Imported folder" not in screen(), "removed import stays hidden after refresh")
                assert (base / "Imported").is_dir()
                click(14, 1); click(4, 10)
                wait(lambda: (base / "opened.txt").exists(), "open favorite")
                wait(lambda: len(rows(base, 2)) == 1, "record successful open")
                assert (base / "opened.txt").read_text() == str(root / "note.txt")
                send(b"\x02"); click(25, 1)
                wait(lambda: "Old document" in screen(), "GTK recent merged")
                assert "private-hidden" not in screen()
                assert "GTK" not in screen(), screen()
                click(12, 10)
                wait(lambda: "ФАЙЛЫ" in screen(), "reveal file in ordinary pane")
                send(b"\x02"); click(25, 1); click(23, 10)
                wait(lambda: len(rows(base, 2)) == 0, "forget recent only")
                assert (root / "note.txt").read_text() == "personal places proof\n"
                send(b"\x1b"); quit_when_ready(child, send)
                print(f"Places PTY grid={grid}: 34x14 mouse tabs/add/label/open/reveal/forget, Unicode, GTK/private records and persistence passed", flush=True)
            except BaseException:
                Path("target/places-pty-failure.txt").write_text(screen())
                raise
            finally:
                if child.poll() is None:
                    child.terminate(); child.wait(timeout=5)
                os.close(master); os.close(slave)


if __name__ == "__main__": main()
