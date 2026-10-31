#!/usr/bin/env python3
"""Verify automatic directory updates in the real TUI without refresh input."""
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time
import unicodedata


def screen_text(output, width=170, height=53):
    """Reconstruct Crossterm's changed cells, including unchanged name prefixes."""
    cells = [[" "] * width for _ in range(height)]
    row = col = 0
    for token in re.findall(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b[^\[]|[^\x1b]+", output.decode(errors="replace")):
        if token.startswith("\x1b["):
            command = token[-1]
            params = token[2:-1]
            if command in "Hf":
                parts = params.split(";")
                row = min(height - 1, max(0, int(parts[0] or 1) - 1))
                col = min(width - 1, max(0, int(parts[1] or 1) - 1)) if len(parts) > 1 else 0
            elif command == "J" and params in ("2", "3"):
                cells = [[" "] * width for _ in range(height)]
            elif command == "K":
                for x in range(col if params in ("", "0") else 0, width): cells[row][x] = " "
            continue
        if token.startswith("\x1b"): continue
        for char in token:
            if char == "\r": col = 0
            elif char == "\n": row = min(height - 1, row + 1)
            elif not unicodedata.category(char).startswith("C"):
                if col < width: cells[row][col] = char
                col += 2 if unicodedata.east_asian_width(char) in "WF" else 1
    return "\n".join("".join(line) for line in cells)


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/basalgrim-tui").resolve()
    for grid in (False, True):
        with tempfile.TemporaryDirectory(prefix="basalgrim-watch-") as tmp:
            base = Path(tmp); source = base / "source"; source.mkdir()
            for i in range(100): (source / f"file-{i:03}.txt").write_text(str(i))
            settings = base / "config/molten-zharr/basalgrim/settings.json"
            settings.parent.mkdir(parents=True)
            settings.write_text(json.dumps({"grid": grid, "preview": False}))
            env = dict(os.environ, TERM="xterm-256color", XDG_STATE_HOME=str(base / "state"), XDG_CONFIG_HOME=str(base / "config"))
            for key in ("TERM_PROGRAM", "LC_TERMINAL", "TMUX", "WEZTERM_EXECUTABLE", "KONSOLE_VERSION"):
                env.pop(key, None)
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 53, 170, 0, 0))
            child = subprocess.Popen([str(binary), str(source)], env=env, stdin=slave, stdout=slave, stderr=slave)
            output = bytearray()

            def pump(seconds=.15):
                end = time.monotonic() + seconds
                while time.monotonic() < end:
                    if select.select([master], [], [], .01)[0]: output.extend(os.read(master, 1 << 20))

            def send(data):
                os.write(master, data.encode() if isinstance(data, str) else data); pump()

            def wait_for(predicate, message):
                end = time.monotonic() + 6
                while not predicate():
                    assert child.poll() is None and time.monotonic() < end, message
                    pump(.02)

            def navigate(path): send(b"\x0c\x15"); send(str(path)); send(b"\r")

            def click(x, y, ctrl=False):
                code = 16 if ctrl else 0
                send(f"\x1b[<{code};{x+1};{y+1}M"); send(f"\x1b[<{code};{x+1};{y+1}m")

            def scroll():
                for _ in range(1 if grid else 3): send(b"\x1b[<65;51;21M")

            try:
                wait_for(lambda: b"file-000.txt" in output, "initial directory")
                click(34, 12 if grid else 11, True); click(52 if grid else 34, 12, True)
                scroll()
                created = source / "file-020-new.txt"; created.write_text("external")
                wait_for(lambda: "file-020-new.txt" in screen_text(output), "external create must repaint without any input")
                created.write_text("x" * 384)
                wait_for(lambda: "384 B" in screen_text(output), "external writes must update file sizes")
                created = created.rename(source / "file-020-old.txt")
                wait_for(lambda: "file-020-old.txt" in screen_text(output) and "file-020-new.txt" not in screen_text(output), "external rename must repaint without input")
                created.unlink()
                wait_for(lambda: "file-020-old.txt" not in screen_text(output), "external deletion must remove the visible entry")
                send(b"\x03")
                protected = base / "protected"; protected.mkdir(); navigate(protected); send(b"\x16")
                wait_for(lambda: all((protected / f"file-{i:03}.txt").exists() for i in (0, 1)), "watch updates must preserve the marked group")
                assert sorted(p.name for p in protected.iterdir()) == ["file-000.txt", "file-001.txt"]
                navigate(source); scroll()
                for i in range(8 if grid else 1): (source / f"file--new-{i:03}").write_text("x")
                pump(1)
                click(34, 12 if grid else 11); send(b"\x03")
                clicked = base / "clicked"; clicked.mkdir(); navigate(clicked); send(b"\x16")
                expected = f"file-{8 if grid else 9:03}.txt"
                wait_for(lambda: (clicked / expected).exists(), "inserting before the viewport must preserve its first visible file")
                send(b"\x11"); child.wait(timeout=5); assert child.returncode == 0
                print(f"Watch PTY passed: {'grid' if grid else 'list'}, external create/write/rename/delete, preserved group and viewport")
            except BaseException:
                Path("target/preview").mkdir(parents=True, exist_ok=True)
                Path(f"target/preview/watch-{'grid' if grid else 'list'}-failure.ansi").write_bytes(output)
                raise
            finally:
                if child.poll() is None: child.terminate(); child.wait(timeout=5)
                os.close(master); os.close(slave)


if __name__ == "__main__": main()
