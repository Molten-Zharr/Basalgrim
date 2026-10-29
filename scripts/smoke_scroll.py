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


def main():
    binary=Path(sys.argv[1] if len(sys.argv)>1 else "target/debug/basalgrim-tui").resolve()
    for grid in (False,True):
        with tempfile.TemporaryDirectory(prefix="basalgrim-scroll-") as tmp:
            base=Path(tmp); source=base/"source"; source.mkdir()
            for i in range(100): (source/f"file-{i:03}.txt").write_text(str(i))
            settings=base/"config/molten-zharr/basalgrim/settings.json"; settings.parent.mkdir(parents=True)
            settings.write_text(json.dumps({"grid":grid,"preview":False}))
            env=dict(os.environ,TERM="xterm-256color",XDG_CONFIG_HOME=str(base/"config"))
            for key in ("TERM_PROGRAM","LC_TERMINAL","TMUX","WEZTERM_EXECUTABLE","KONSOLE_VERSION"): env.pop(key,None)
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
                    assert child.poll() is None and time.monotonic()<end,message
                    pump(.02)
            def navigate(path): send(b"\x0c\x15"); send(str(path)); send(b"\r")
            def click(x,y,ctrl=False):
                code=16 if ctrl else 0
                send(f"\x1b[<{code};{x+1};{y+1}M"); send(f"\x1b[<{code};{x+1};{y+1}m")
            def scroll():
                for _ in range(1 if grid else 3): send(b"\x1b[<65;51;21M")
            try:
                wait_for(lambda:b"file-000.txt" in output,"initial directory")
                click(34,12 if grid else 11,True); click(52 if grid else 34,12,True)
                scroll(); send(b"\x03")
                protected=base/"protected"; protected.mkdir(); navigate(protected); send(b"\x16")
                wait_for(lambda:all((protected/n).exists() for n in ("file-000.txt","file-001.txt")),"wheel must preserve Ctrl-selected files")
                assert sorted(p.name for p in protected.iterdir())==["file-000.txt","file-001.txt"]
                navigate(source); scroll(); click(34,12 if grid else 11)
                expected=f"file-{8 if grid else 9:03}.txt"
                clicked=base/"clicked"; clicked.mkdir(); send(b"\x03"); navigate(clicked); send(b"\x16")
                wait_for(lambda:(clicked/expected).exists(),"first visible file must reflect viewport scrolling")
                navigate(source); scroll(); send(b"\x1b[B\x03")
                expected=f"file-{8 if grid else 1:03}.txt"
                keyboard=base/"keyboard"; keyboard.mkdir(); navigate(keyboard); send(b"\x16")
                wait_for(lambda:(keyboard/expected).exists(),"keyboard navigation must use original focus")
                send(b"\x11"); child.wait(timeout=5); assert child.returncode==0
                print(f"Scroll PTY passed: {'grid' if grid else 'list'}, preserved group, correct visible click and keyboard recovery")
            finally:
                if child.poll() is None: child.terminate(); child.wait(timeout=5)
                os.close(master); os.close(slave)


if __name__=="__main__": main()
