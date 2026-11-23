#!/usr/bin/env python3
"""Verify real wheel scrolling and selection preservation through a PTY."""
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

from pty_helpers import quit_when_ready
from smoke_watch import screen_text


def main():
    binary=Path(sys.argv[1] if len(sys.argv)>1 else "target/debug/basalgrim-tui").resolve()
    for grid in (False,True):
        with tempfile.TemporaryDirectory(prefix="basalgrim-scroll-") as tmp:
            base=Path(tmp); source=base/"source"; source.mkdir()
            for i in range(100): (source/f"file-{i:03}.txt").write_text(str(i))
            settings=base/"config/molten-zharr/basalgrim/settings.json"; settings.parent.mkdir(parents=True)
            settings.write_text(json.dumps({"grid":grid,"preview":False}))
            env=dict(os.environ,TERM="xterm-256color",HOME=str(base),XDG_DATA_HOME=str(base/"data"),XDG_STATE_HOME=str(base/"state"),XDG_CONFIG_HOME=str(base/"config"))
            for key in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "TERM_PROGRAM","LC_TERMINAL","TMUX","WEZTERM_EXECUTABLE","KONSOLE_VERSION"): env.pop(key,None)
            master,slave=pty.openpty(); fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack("HHHH",53,170,0,0))
            child=subprocess.Popen([str(binary),str(source)],env=env,stdin=slave,stdout=slave,stderr=slave)
            output=bytearray()
            def pump(seconds=.12):
                end=time.monotonic()+seconds
                while time.monotonic()<end:
                    if select.select([master],[],[],.01)[0]: output.extend(os.read(master,1<<20))
            def send(data): os.write(master,data.encode() if isinstance(data,str) else data); pump()
            def wait_for(predicate,message):
                end=time.monotonic()+6
                while not predicate():
                    assert child.poll() is None and time.monotonic()<end, f"{message}\n{screen_text(output)}"
                    pump(.02)
            def navigate(path): send(b"\x0c\x15"); send(str(path)); send(b"\r")
            def click(x,y,ctrl=False):
                code=16 if ctrl else 0
                send(f"\x1b[<{code};{x+1};{y+1}M"); send(f"\x1b[<{code};{x+1};{y+1}m")
            def scroll():
                for _ in range(1 if grid else 3): send(b"\x1b[<65;51;21M")
            def scroll_source():
                wait_for(lambda: "file-002.txt" in screen_text(output), "source directory loaded")
                scroll()
                row = 12 if grid else 10
                expected = f"file-{8 if grid else 9:03}.txt"
                wait_for(lambda: expected in screen_text(output).splitlines()[row], "scrolled entry visible")
            try:
                wait_for(lambda:b"file-000.txt" in output,"initial directory")
                def spacing():
                    lines = screen_text(output).splitlines()
                    coords = [next(((x, y) for y, line in enumerate(lines)
                                    if (x := line.find(name)) >= 0), None)
                              for name in ("file-000.txt", "file-001.txt")]
                    if None in coords:
                        return 0
                    axis = 0 if grid else 1
                    return coords[1][axis] - coords[0][axis]
                initial = spacing()
                assert initial > 0
                send(b"\x1b[<80;51;21M")
                wait_for(lambda:spacing()>initial, "Ctrl+wheel up enlarges file cells")
                assert json.loads(settings.read_text())["file_zoom"] == [1, 0]
                enlarged = spacing()
                assert b"\x1b[>1u" in output, "TUI requested unambiguous modified keys"
                send(b"\x1b[45;5u")
                wait_for(lambda:spacing()==initial, "Ctrl+minus directly restores canvas")
                send(b"\x1b[61;6u")
                wait_for(lambda:spacing()==enlarged, "Ctrl+Shift+equals yields the same zoom as wheel up")
                send(b"\x1b[<81;51;21M")
                wait_for(lambda:spacing()==initial, "Ctrl+wheel down restores layout")
                assert json.loads(settings.read_text())["file_zoom"] == [0, 0]
                send(b"\x1b=")
                wait_for(lambda:spacing()==enlarged, "legacy Alt+equals works directly on canvas")
                send(b"\x1b-")
                wait_for(lambda:spacing()==initial, "legacy Alt+minus restores canvas")
                click(34,12 if grid else 10,True); click(52 if grid else 34,12 if grid else 11,True)
                scroll(); send(b"\x03")
                protected=base/"protected"; protected.mkdir(); navigate(protected); send(b"\x16")
                wait_for(lambda:all((protected/n).exists() for n in ("file-000.txt","file-001.txt")),"wheel must preserve Ctrl-selected files")
                assert sorted(p.name for p in protected.iterdir())==["file-000.txt","file-001.txt"]
                navigate(source); scroll_source(); click(34,12 if grid else 10)
                expected=f"file-{8 if grid else 9:03}.txt"
                clicked=base/"clicked"; clicked.mkdir(); send(b"\x03"); navigate(clicked); send(b"\x16")
                wait_for(lambda:(clicked/expected).exists(),"first visible file must reflect viewport scrolling")
                navigate(source); scroll_source(); send(b"\x1b[B\x03")
                expected=f"file-{8 if grid else 1:03}.txt"
                keyboard=base/"keyboard"; keyboard.mkdir(); navigate(keyboard); send(b"\x16")
                wait_for(lambda:(keyboard/expected).exists(),"keyboard navigation must use original focus")
                quit_when_ready(child, send)
                pump()
                assert output.count(b"\x1b[<1u") + output.count(b"\x1b[<u") == 1
                print(f"Scroll PTY passed: {'grid' if grid else 'list'}, preserved group, correct visible click and keyboard recovery")
            finally:
                if child.poll() is None: child.terminate(); child.wait(timeout=5)
                os.close(master); os.close(slave)


if __name__=="__main__": main()
