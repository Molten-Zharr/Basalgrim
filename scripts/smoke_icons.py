#!/usr/bin/env python3
"""Check real TUI negotiation, fallback, preferences and input after probe timeout."""
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


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/basalgrim-tui").resolve()
    for protocol in ("timeout", "none", "kitty", "sixel", "iterm2", "kitty-symbols"):
        with tempfile.TemporaryDirectory(prefix="basalgrim-icons-") as tmp:
            root = Path(tmp)
            files, config = root / "files", root / "config"
            files.mkdir()
            for name in ("main.rs", "archive.zip", "notes.txt"):
                (files / name).write_text("icon test")
            settings = config / "molten-zharr/basalgrim/settings.json"
            settings.parent.mkdir(parents=True)
            mode = "symbols" if protocol == "kitty-symbols" else "images"
            settings.write_text(json.dumps({"preview": False, "icon_mode": mode}))
            env = dict(os.environ, TERM="xterm-256color", XDG_STATE_HOME=str(config.parent / "state"), XDG_CONFIG_HOME=str(config))
            for name in ("TERM_PROGRAM", "LC_TERMINAL", "TMUX", "WEZTERM_EXECUTABLE", "KONSOLE_VERSION", "NO_COLOR"):
                env.pop(name, None)
            if protocol == "iterm2":
                env["TERM_PROGRAM"] = "iTerm.app"
            master, slave = pty.openpty()
            original = termios.tcgetattr(slave)
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 800, 480))
            child = subprocess.Popen([str(binary), str(files)], stdin=slave, stdout=slave, stderr=slave, env=env)
            output = bytearray()
            replied = False

            def pump(seconds=0.08):
                nonlocal replied
                end = time.monotonic() + seconds
                while time.monotonic() < end:
                    if select.select([master], [], [], max(0, min(0.01, end - time.monotonic())))[0]:
                        output.extend(os.read(master, 1 << 20))
                    if not replied and protocol != "timeout" and b"\x1b[5n" in output:
                        reply = b""
                        if protocol.startswith("kitty"):
                            reply += b"\x1b_Gi=31;OK\x1b\\"
                        reply += b"\x1b[?62;4c" if protocol == "sixel" else b"\x1b[?62c"
                        reply += b"\x1b[6;16;8t\x1b[0n"
                        os.write(master, reply)
                        replied = True

            def wait_for(predicate, description):
                deadline = time.monotonic() + 5
                while not predicate():
                    assert child.poll() is None, f"{protocol}: exited during {description}"
                    assert time.monotonic() < deadline, f"{protocol}: timeout: {description}"
                    pump()

            def send(value):
                os.write(master, value)
                pump()

            def has_image():
                return b",a=T,U=1,f=32," in output or b"\x1bP" in output or b"\x1b]1337;File=" in output

            try:
                wait_for(lambda: b"BASALGRIM" in output, "first frame")
                images = protocol in ("kitty", "sixel", "iterm2")
                if images:
                    wait_for(has_image, "image payload")
                else:
                    wait_for(lambda: "≡≡".encode() in output, "symbol fallback")
                    assert not has_image(), f"{protocol}: unexpected image payload"
                assert json.loads(settings.read_text())["icon_mode"] == mode
                send(b"\x1b[21~")  # F10 must work even after a probe timeout.
                wait_for(lambda: "Настройки".encode() in output, "settings after negotiation")
                send(b"\t\x1b[C\x1b[C\x1b[C\t\x1b[C")  # Themes -> Icons -> toggle
                expected = "images" if mode == "symbols" else "symbols"
                wait_for(lambda: json.loads(settings.read_text())["icon_mode"] == expected, "saved icon preference")
                output.clear()
                send(b"\x1b")
                if expected == "symbols" or not images and protocol != "kitty-symbols":
                    wait_for(lambda: "≡≡".encode() in output, "symbols after settings")
                    assert not has_image(), f"{protocol}: images remained after switching to symbols"
                else:
                    wait_for(has_image, "images after settings")
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 640, 384))
                child.send_signal(__import__("signal").SIGWINCH)
                pump()
                send(b"\x11")
                child.wait(timeout=5)
                assert child.returncode == 0
                restored = termios.tcgetattr(slave)
                assert (restored[3] & (termios.ECHO | termios.ICANON)) == (original[3] & (termios.ECHO | termios.ICANON))
                print(f"Icon PTY passed: {protocol}, saved toggle, resize, input, terminal restore")
            except BaseException:
                print(output[-2000:].decode(errors="replace"), file=sys.stderr)
                raise
            finally:
                if child.poll() is None:
                    child.terminate()
                    child.wait(timeout=5)
                os.close(master)
                os.close(slave)


if __name__ == "__main__":
    main()
