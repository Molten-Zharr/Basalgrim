#!/usr/bin/env python3
"""Shared internal file drag/drop through genuine SGR mouse input in a private PTY."""
import fcntl
import errno
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
from pty_helpers import quit_when_ready
from smoke_watch import screen_text


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/basalgrim-tui").resolve()
    for grid in (False, True):
        with tempfile.TemporaryDirectory(prefix="basalgrim-drag-") as tmp:
            base = Path(tmp); source = base / "source"; source.mkdir(); folder = source / "folder"; folder.mkdir()
            for name in ("a.txt", "b.txt", "c.txt", "z.txt"):
                (source / name).write_text(name)
            env = dict(os.environ, HOME=tmp, XDG_DATA_HOME=str(base / "data"), XDG_STATE_HOME=str(base / "state"), XDG_CONFIG_HOME=str(base / "config"), TERM="xterm-256color")
            for name in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "TERM_PROGRAM", "LC_TERMINAL", "TMUX", "WEZTERM_EXECUTABLE", "KONSOLE_VERSION"):
                env.pop(name, None)
            settings = base / "config/molten-zharr/basalgrim/settings.json"; settings.parent.mkdir(parents=True)
            settings.write_text(json.dumps({"grid": grid, "preview": False, "icon_mode": "symbols"}))
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 53, 170, 0, 0))
            process = subprocess.Popen([str(binary), "--no-session", "--trash-home-only", str(source)], env=env, stdin=slave, stdout=slave, stderr=slave)
            os.close(slave); output = bytearray()

            def pump(seconds=.12):
                until = time.monotonic() + seconds
                while time.monotonic() < until:
                    if select.select([master], [], [], .01)[0]:
                        try:data=os.read(master,65536)
                        except OSError as error:
                            if error.errno==errno.EIO:return
                            raise
                        if not data:return
                        output.extend(data)

            def send(data):
                os.write(master, data.encode() if isinstance(data, str) else data); pump()

            def wait(predicate, label):
                until = time.monotonic() + 8
                while True:
                    status=screen_text(output,170,53).splitlines()[-1]
                    if predicate() and 'Стоп' not in status and 'Отмена…' not in status:break
                    assert process.poll() is None and time.monotonic() < until, (label, output[-1000:])
                    pump(.02)
                pump(.25)

            def mouse(code, point, up=False):
                send(f"\x1b[<{code};{point[0]+1};{point[1]+1}{'m' if up else 'M'}")

            def click(point, ctrl=False):
                mouse(16 if ctrl else 0, point); mouse(16 if ctrl else 0, point, True)

            def drag(start, end, copy=False, cancel=False):
                mouse(0, start); mouse(48 if copy else 32, end)
                if cancel:
                    send(b"\x1b")
                mouse(16 if copy else 0, end, True)

            def navigate(path):
                send(b"\x0c\x15\x1b[200~" + str(path).encode() + b"\x1b[201~\r"); pump(.25)

            try:
                wait(lambda: b"a.txt" in output, "initial list")
                a, b, c, z, target = ((54,12),(74,12),(94,12),(106,12),(34,12)) if grid else ((34,12),(34,13),(34,14),(34,15),(34,11))
                click(z); send(b"\x03")
                proof=base/'initial-clipboard';proof.mkdir();navigate(proof);send(b'\x16')
                wait(lambda:(proof/'z.txt').exists(),'initial file clipboard setup');navigate(source)
                click(a, True); click(b, True); drag(b, target)
                wait(lambda: (folder / "a.txt").exists() and (folder / "b.txt").exists(), "group move into folder")
                assert not (source / "a.txt").exists() and not (source / "b.txt").exists()
                c = (54,12) if grid else (34,12)
                drag(c, target, copy=True)
                wait(lambda: (folder / "c.txt").exists(), "Ctrl copy into folder")
                assert (source / "c.txt").exists()
                drag(c, target, cancel=True)
                assert (source / "c.txt").exists() and not (folder / "c (1).txt").exists()
                other = base / "other-tab"; other.mkdir()
                send(b"\x14"); navigate(other); click((28,6)); pump(.2)
                drag(c, (39,6))
                wait(lambda: (other / "c.txt").exists(), "drop on inactive tab")
                assert not (source / "c.txt").exists()
                send(b"\x1a"); wait(lambda: (source / "c.txt").exists(), "undo mouse move")
                send(b"\x19"); wait(lambda: (other / "c.txt").exists() and not (source / "c.txt").exists(), "redo mouse move")
                (source / "pane.txt").write_text("pane payload"); pump(.6)
                send(b"\x04"); click((110,25))
                second = base / "second-pane"; second.mkdir(); navigate(second)
                drag((59,12) if grid else (34,12), (110,25))
                wait(lambda: (second / "pane.txt").exists(), "drop into opposite pane")
                clipboard = base / "clipboard"; clipboard.mkdir(); navigate(clipboard); send(b"\x16")
                wait(lambda: (clipboard / "z.txt").exists(), "drag/drop preserves the original clipboard")
                parent=base/'up';inside=parent/'one/inside';inside.mkdir(parents=True)
                (inside/'ancestor.txt').write_text('ancestor');(inside/'peer.txt').write_text('peer');navigate(inside)
                parent_x=25+3+1+5+1+len(base.name)+2+1+1
                drag((34,12) if grid else (34,11),(parent_x,8))
                wait(lambda:(parent/'ancestor.txt').exists(),'breadcrumb move several levels up')
                drag((34,12) if grid else (34,11),(parent_x,8),copy=True)
                wait(lambda:(parent/'peer.txt').exists(),'Ctrl breadcrumb copy');assert (inside/'peer.txt').exists()
                click((115,6));wait(lambda:'2:second-pane' in screen_text(output,170,53).splitlines()[6],'inactive pane plus')
                click((167,5));wait(lambda:json.loads(settings.read_text()).get('dual') is False,'second-pane close')
                click((155,5));wait(lambda:json.loads(settings.read_text()).get('dual') is True,'second-pane reopen')
                assert '2:second-pane' in screen_text(output,170,53).splitlines()[6]
                quit_when_ready(process, send)
                print(f"Drag/drop PTY {'grid' if grid else 'list'}: group move, Ctrl copy, Escape, tabs, panes, undo/redo and preserved clipboard passed", flush=True)
            finally:
                Path('target/preview').mkdir(parents=True,exist_ok=True)
                Path('target/preview/drag-drop-pty-screen.txt').write_text(screen_text(output,170,53))
                Path('target/preview/drag-drop-pty-tree.txt').write_text('\n'.join(str(p.relative_to(base)) for p in base.rglob('*')))
                if process.poll() is None:
                    process.kill(); process.wait(timeout=5)
                os.close(master)


if __name__ == "__main__":
    main()
