#!/usr/bin/env python3
"""Real PTY other-pane transfers, duplicate, clipboard preservation and compact mouse menus."""
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
sys.dont_write_bytecode = True
from smoke_watch import screen_text


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/basalgrim-tui").resolve()
    with tempfile.TemporaryDirectory(prefix="basalgrim-panes-") as tmp:
        base = Path(tmp); source = base / "source"; dest = base / "destination"
        source.mkdir(); dest.mkdir(); clipboard_check = base / "clipboard"; clipboard_check.mkdir()
        for name in ("a.txt", "b.txt", "untouched"):
            (source / name).write_text("incoming " + name)
        settings = base / "config/molten-zharr/basalgrim/settings.json"
        settings.parent.mkdir(parents=True)
        settings.write_text(json.dumps({"dual": True, "grid": False, "preview": False}))
        env = dict(os.environ, TERM="xterm-256color", XDG_CONFIG_HOME=str(base / "config"))
        for name in ("TERM_PROGRAM", "LC_TERMINAL", "TMUX", "WEZTERM_EXECUTABLE", "KONSOLE_VERSION"):
            env.pop(name, None)
        master, slave = pty.openpty(); width, height = 170, 40
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
        child = subprocess.Popen([str(binary), str(source)], env=env, stdin=slave, stdout=slave, stderr=slave)
        output = bytearray()

        def pump(seconds=.18):
            end = time.monotonic() + seconds
            while time.monotonic() < end:
                if select.select([master], [], [], .01)[0]: output.extend(os.read(master, 1 << 20))

        def send(value):
            os.write(master, value.encode() if isinstance(value, str) else value); pump()

        def text(): return screen_text(output, width, height)

        def wait_for(predicate, message):
            end = time.monotonic() + 8
            while not predicate():
                assert child.poll() is None and time.monotonic() < end, message
                pump(.02)

        def navigate(path): send(b"\x0c\x15"); send(str(path)); send(b"\r")

        def click(x, y, button=0):
            send(f"\x1b[<{button};{x+1};{y+1}M"); send(f"\x1b[<{button};{x+1};{y+1}m")

        def row_of(name): return next(i for i, row in enumerate(text().splitlines()) if name in row[:98])

        def choose(label):
            wait_for(lambda: label in text(), "mouse command: " + label)
            rows = text().splitlines(); row = next(i for i, value in enumerate(rows) if label in value)
            click(rows[row].index(label) + 1, row)

        try:
            wait_for(lambda: "a.txt" in text(), "initial files")
            send(b"\t"); navigate(dest); send(b"\t")
            send(b"\x1b[F\x03\x1b[H")  # End -> copy untouched -> Home.
            click(34, row_of("a.txt"), 16); click(34, row_of("b.txt"), 16)
            send(b"\x1b[15;2~")  # Shift+F5: copy to other pane.
            wait_for(lambda: all((dest / n).exists() for n in ("a.txt", "b.txt")), "other-pane group copy")
            assert not (dest / "untouched").exists()
            pump(.3)
            click(34, row_of("a.txt"), 2); choose("Дублировать")
            wait_for(lambda: all((source / n).exists() for n in ("a (1).txt", "b (1).txt")), "mouse group duplicate")
            pump(.3); send(b"\x1b[17;2~")  # Shift+F6: move -> conflicts.
            wait_for(lambda: "Совпадение имён" in text(), "direct move conflict")
            send(b" \x1b[A\r")  # Remaining + Skip.
            wait_for(lambda: "Совпадение имён" not in text(), "skip remaining conflicts")
            for name in ("a.txt", "b.txt"):
                assert (source / name).exists(); (dest / name).unlink()
            pump(.4)
            click(34, row_of("a.txt"), 2); choose("Переместить в соседнюю панель")
            wait_for(lambda: all(not (source / n).exists() and (dest / n).exists() for n in ("a.txt", "b.txt")), "mouse group move")
            pump(.3); send(b"\x1b[H\x1b[15;5~")  # Home + Ctrl+F5 duplicate.
            wait_for(lambda: (source / "a (1) (1).txt").exists(), "duplicate shortcut")
            pump(.3)
            width, height = 34, 14
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
            child.send_signal(signal.SIGWINCH); pump(.3)
            send(b"\x1b[21;2~")  # Shift+F10: global mouse menu.
            for _ in range(15):
                if "Дублировать" in text(): break
                send(b"\x1b[<65;5;10M")
                assert "Выход" not in text(), "duplicate command was skipped"
            choose("Дублировать")
            wait_for(lambda: (source / "a (1) (2).txt").exists(), "compact mouse duplicate")
            navigate(clipboard_check); send(b"\x16")
            wait_for(lambda: (clipboard_check / "untouched").exists(), "original clipboard preserved")
            assert sorted(p.name for p in clipboard_check.iterdir()) == ["untouched"]
            assert not list(base.rglob(".basalgrim-*"))
            pump(.3); send(b"\x11"); child.wait(timeout=5); assert child.returncode == 0
            print("Pane PTY passed: Shift+F5/F6, Ctrl+F5, group mouse commands, conflict scope, compact menu and original clipboard")
        except BaseException:
            print(text(), file=sys.stderr); raise
        finally:
            if child.poll() is None: child.terminate(); child.wait(timeout=5)
            os.close(master); os.close(slave)


if __name__ == "__main__": main()
