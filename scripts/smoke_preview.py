#!/usr/bin/env python3
"""Image previews in a real PTY; stdlib only, icons deliberately forced to symbols."""
import binascii
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
import zlib


def png(path, color):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", binascii.crc32(kind + data))
    raw = (b"\0" + bytes(color) * 64) * 32
    path.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 64, 32, 8, 6, 0, 0, 0))
                     + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/basalgrim-tui").resolve()
    for protocol in ("timeout", "none", "kitty", "sixel", "iterm2"):
        with tempfile.TemporaryDirectory(prefix="basalgrim-preview-") as tmp:
            root = Path(tmp)
            files = root / "files"
            files.mkdir()
            png(files / "a.png", [224, 40, 30, 255])
            png(files / "b.png", [30, 210, 60, 128])
            (files / "c.txt").write_text("preview text after image\n")
            settings = root / "config/molten-zharr/basalgrim/settings.json"
            settings.parent.mkdir(parents=True)
            settings.write_text(json.dumps({"preview": True, "grid": False, "icon_mode": "symbols"}))
            env = dict(os.environ, TERM="xterm-256color", XDG_CONFIG_HOME=str(root / "config"),
                       XDG_STATE_HOME=str(root / "state"), XDG_DATA_HOME=str(root / "data"))
            for name in ("TERM_PROGRAM", "LC_TERMINAL", "TMUX", "WEZTERM_EXECUTABLE", "KONSOLE_VERSION"):
                env.pop(name, None)
            if protocol == "iterm2":
                env["TERM_PROGRAM"] = "iTerm.app"
            master, slave = pty.openpty()
            original = termios.tcgetattr(slave)
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 150, 1200, 640))
            process = subprocess.Popen([str(binary), "--no-session", str(files)], stdin=slave, stdout=slave, stderr=slave, env=env)
            output = bytearray()
            replied = False

            def pump(seconds=.1):
                nonlocal replied
                end = time.monotonic() + seconds
                while time.monotonic() < end:
                    if select.select([master], [], [], .01)[0]:
                        output.extend(os.read(master, 1 << 20))
                    if not replied and protocol != "timeout" and b"\x1b[5n" in output:
                        reply = b"\x1b_Gi=31;OK\x1b\\" if protocol == "kitty" else b""
                        reply += b"\x1b[?62;4c" if protocol == "sixel" else b"\x1b[?62c"
                        os.write(master, reply + b"\x1b[6;16;8t\x1b[0n")
                        replied = True

            def wait(predicate, description):
                end = time.monotonic() + 8
                while not predicate():
                    assert process.poll() is None, f"{protocol}: exited: {description}"
                    assert time.monotonic() < end, f"{protocol}: {description}"
                    pump()

            def send(data):
                os.write(master, data)
                pump()

            def image():
                return b",a=T,U=1,f=32," in output or b"\x1bP" in output or b"\x1b]1337;File=" in output

            def text():
                return re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", output.decode(errors="replace")).replace(" ", "")

            try:
                wait(lambda: b"BASALGRIM" in output and "64×32" in text(), "image metadata")
                graphical = protocol in ("kitty", "sixel", "iterm2")
                if graphical:
                    wait(image, "actual preview payload with symbol icons")
                    pump(.3)
                    output.clear()
                    png(files / "a.png", [30, 70, 240, 255])
                    wait(image, "same-path external rewrite")
                    send(b"\x1b[<35;126;23M")
                    send(b"\x1b[21;2~")  # Shift+F10: Actions at pointer
                    pump(.3)
                    output.clear()
                    send(b"\x0c")
                    assert not image(), f"{protocol}: preview covered its menu"
                    send(b"\x1b")
                    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 70, 560, 384))
                    process.send_signal(signal.SIGWINCH)
                    pump(.3)
                    output.clear()
                    send(b"\x1b[B")
                    assert not image(), f"{protocol}: image in hidden pane"
                    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 150, 1200, 640))
                    process.send_signal(signal.SIGWINCH)
                    wait(image, "preview after resize")
                    send(b"\x1b[B")  # c.txt
                else:
                    wait(lambda: "не поддерживает графику".encode() in output, "text fallback")
                    assert not image(), f"{protocol}: unexpected graphics"
                    send(b"\x1b[B\x1b[B")
                wait(lambda: "previewtextafterimage" in text(), "text after image")
                pump(.2)
                output.clear()
                send(b"\x1b[13~")  # F3 off
                send(b"\x1b[H")
                assert not image(), f"{protocol}: graphics after disabling preview"
                send(b"\x11")
                process.wait(timeout=5)
                assert process.returncode == 0
                restored = termios.tcgetattr(slave)
                assert restored[3] & (termios.ECHO | termios.ICANON) == original[3] & (termios.ECHO | termios.ICANON)
                assert json.loads(settings.read_text())["icon_mode"] == "symbols"
                print(f"Preview PTY passed: {protocol}, rewrite, modal, resize, text and disabled preview", flush=True)
            except BaseException:
                Path("target/preview").mkdir(parents=True, exist_ok=True)
                Path(f"target/preview/pty-preview-{protocol}.ansi").write_bytes(output)
                print(text()[-2000:], file=sys.stderr)
                raise
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)
                os.close(master)
                os.close(slave)


if __name__ == "__main__":
    main()
