#!/usr/bin/env python3
"""Move real files between /tmp and /dev/shm through the TUI's cut clipboard."""
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

sys.dont_write_bytecode = True
from smoke_watch import screen_text


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/basalgrim-tui").resolve()
    with tempfile.TemporaryDirectory(prefix="basalgrim-move-") as tmp, tempfile.TemporaryDirectory(prefix="basalgrim-move-target-", dir="/dev/shm") as remote:
        base, destination = Path(tmp), Path(remote)
        assert base.stat().st_dev != destination.stat().st_dev, "This test requires two filesystems"
        source = base / "source"; (source / "folder").mkdir(parents=True)
        (source / "payload.txt").write_text("move payload")
        (source / "folder/data").write_bytes(bytes(range(256)) * 8192)
        (source / "folder/broken").symlink_to("missing")
        config = base / "config/molten-zharr/basalgrim/settings.json"; config.parent.mkdir(parents=True)
        config.write_text(json.dumps({"grid": False, "preview": False}))
        env = dict(os.environ, TERM="xterm-256color", XDG_STATE_HOME=str(base / "state"), XDG_CONFIG_HOME=str(base / "config"))
        for name in ("TERM_PROGRAM", "LC_TERMINAL", "TMUX", "WEZTERM_EXECUTABLE", "KONSOLE_VERSION"):
            env.pop(name, None)
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 170, 0, 0))
        child = subprocess.Popen([str(binary), str(source)], env=env, stdin=slave, stdout=slave, stderr=slave)
        output = bytearray()
        locked = None; copied_locked = None

        def pump(seconds=.15):
            end = time.monotonic() + seconds
            while time.monotonic() < end:
                if select.select([master], [], [], .005)[0]: output.extend(os.read(master, 1 << 20))

        def send(value): os.write(master, value.encode() if isinstance(value, str) else value); pump()

        def wait_for(predicate, description):
            end = time.monotonic() + 8
            while not predicate():
                assert child.poll() is None and time.monotonic() < end, description
                pump(.01)

        def navigate(path): send(b"\x0c\x15"); send(str(path)); send(b"\r")

        try:
            wait_for(lambda: b"payload.txt" in output, "initial files")
            send(b"\x01\x18"); navigate(destination); send(b"\x16")
            wait_for(lambda: not list(source.iterdir()) and (destination / "payload.txt").exists(), "cross-device file and directory move")
            assert (destination / "folder/data").read_bytes() == bytes(range(256)) * 8192
            assert os.readlink(destination / "folder/broken") == "missing"
            (source / "a").write_text("first"); (source / "z").write_text("last")
            (destination / "z").write_text("precious")
            navigate(source); send(b"\x01\x18"); navigate(destination); send(b"\x16")
            wait_for(lambda: not (source / "a").exists() and (destination / "a").exists(), "first item before collision")
            assert (source / "z").read_text() == "last" and (destination / "z").read_text() == "precious"
            wait_for(lambda: "Совпадение имён" in screen_text(output, 170, 40), "collision decision")
            send(b"\x1b[A\r")  # Default Keep Both -> Skip, retaining the cut source.
            retry = destination / "retry"; retry.mkdir(); navigate(retry); send(b"\x16")
            wait_for(lambda: not (source / "z").exists() and (retry / "z").exists(), "clipboard retains only the unfinished cut item")
            send(b"\x18"); navigate(source); send(b"\x16")
            wait_for(lambda: (source / "z").exists() and not (retry / "z").exists(), "reverse move from tmpfs to disk")
            assert (source / "z").read_text() == "last"
            # An undeletable source keeps the published copy and produces an
            # explicit partial result rather than being counted as a move.
            ro_source = base / "readonly"; locked = ro_source / "locked"; locked.mkdir(parents=True)
            (locked / "data").write_text("keep original"); locked.chmod(0o555)
            ro_destination = destination / "readonly"; ro_destination.mkdir(); copied_locked = ro_destination / "locked"
            navigate(ro_source); send(b"\x18"); navigate(ro_destination); send(b"\x16\x1b[17~")
            wait_for(lambda: "Копия создана; перенос не завершён" in screen_text(output, 170, 40), "partial move must be explicit in operations")
            assert (locked / "data").read_text() == (copied_locked / "data").read_text() == "keep original"
            assert locked.stat().st_mode & 0o777 == 0o555
            send(b"\x11"); child.wait(timeout=5); assert child.returncode == 0
            for parent in (base, destination):
                assert not list(parent.rglob(".basalgrim-*")), "Temporary move data survived"
            print("Cross-device PTY passed: file/tree/symlink, reverse move, collision, exact cut clipboard and retained-copy report")
        except BaseException:
            print(screen_text(output, 170, 40), file=sys.stderr)
            raise
        finally:
            if child.poll() is None: child.terminate(); child.wait(timeout=5)
            for path in (locked, copied_locked):
                if path and path.exists(): path.chmod(0o755)
            os.close(master); os.close(slave)


if __name__ == "__main__": main()
