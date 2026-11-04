#!/usr/bin/env python3
"""File MIME exchange through real PTY input, on a caller-owned private display."""
import json
import os
from pathlib import Path
import pty
import select
import subprocess
import sys
import tempfile
import time
from pty_helpers import quit_when_ready


def main():
    assert os.environ.get("BASALGRIM_CLIPBOARD_TEST_ISOLATED") == "1", "Use a private display; never the user's clipboard"
    binary = Path(sys.argv[1]).resolve()
    wayland = bool(os.environ.get("WAYLAND_DISPLAY"))
    xclip = os.environ.get("XCLIP", "xclip")

    def offer(path, cut):
        payload = ("cut" if cut else "copy") + "\n" + path.as_uri()
        args = ["wl-copy", "--foreground", "--type", "x-special/gnome-copied-files", payload] if wayland else [xclip, "-selection", "clipboard", "-target", "x-special/gnome-copied-files", "-quiet"]
        child = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if not wayland:
            child.stdin.write(payload.encode())
        child.stdin.close()
        time.sleep(.2)
        return child

    def get(mime):
        args = ["wl-paste", "--no-newline", "--type", mime] if wayland else [xclip, "-selection", "clipboard", "-out", "-target", mime]
        return subprocess.check_output(args, timeout=3)

    with tempfile.TemporaryDirectory(prefix="basalgrim-file-clipboard-pty-") as tmp:
        base = Path(tmp)
        source, dest = base / "source", base / "dest"
        source.mkdir(); dest.mkdir()
        item = source / "Картинка #1.txt"
        item.write_text("file clipboard through PTY")
        moved = source / "cut.txt"
        moved.write_text("cut through PTY")
        env = dict(os.environ, HOME=tmp, XDG_DATA_HOME=str(base / "data"), XDG_STATE_HOME=str(base / "state"), XDG_CONFIG_HOME=str(base / "config"), TERM="xterm-256color")
        for key in ("SSH_TTY", "SSH_CONNECTION"):
            env.pop(key, None)
        settings = base / "config/molten-zharr/basalgrim/settings.json"
        settings.parent.mkdir(parents=True)
        settings.write_text(json.dumps({"preview": False, "grid": False, "restore_session": False}))
        master, slave = pty.openpty()
        process = subprocess.Popen([str(binary), "--no-session", "--trash-home-only", str(dest)], env=env, stdin=slave, stdout=slave, stderr=slave)
        os.close(slave)
        output = bytearray()

        def drain():
            while select.select([master], [], [], 0)[0]:
                try:
                    output.extend(os.read(master, 65536))
                except OSError:
                    break

        def send(data):
            drain(); os.write(master, data); time.sleep(.1); drain()

        def wait(predicate, message):
            deadline = time.monotonic() + 8
            while not predicate():
                drain()
                assert process.poll() is None, output[-2000:]
                assert time.monotonic() < deadline, message
                time.sleep(.02)

        owner = None
        try:
            time.sleep(.5); drain()
            for path, cut in ((item, False), (moved, True)):
                owner = offer(path, cut)
                send(b"\x16")
                wait(lambda: (dest / path.name).exists(), "External file paste in TUI")
                assert path.exists() != cut
                owner.terminate(); owner.wait(timeout=3); owner = None
                time.sleep(.3)
            send(b"\x0c\x15\x1b[200~" + str(source).encode() + b"\x1b[201~\r")
            time.sleep(.3); send(b"\x03")
            wait(lambda: get("x-special/gnome-copied-files") == ("copy\n" + item.as_uri()).encode(), "TUI copy export")
            assert get("application/x-kde-cutselection") == b"0"
            send(b"\x18")
            wait(lambda: get("application/x-kde-cutselection") == b"1", "TUI cut export")
            quit_when_ready(process, send)
        finally:
            if owner:
                owner.terminate(); owner.wait(timeout=3)
            if process.poll() is None:
                process.kill(); process.wait(timeout=3)
            os.close(master)
        print("Native PTY: external copy/cut import and GNOME/KDE export passed", flush=True)


if __name__ == "__main__":
    main()
