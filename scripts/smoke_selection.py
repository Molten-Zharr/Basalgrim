#!/usr/bin/env python3
"""Real SGR mouse press/drag/release and copy of the selected set in a PTY."""
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
    binary = Path(sys.argv[1] if len(sys.argv)>1 else "target/debug/basalgrim-tui").resolve()
    for grid in (False,True):
        with tempfile.TemporaryDirectory(prefix="basalgrim-selection-") as tmp:
            base=Path(tmp); source=base/"source"; source.mkdir(); destination=source/"destination"; destination.mkdir()
            for name in ("alpha.txt","beta.txt","gamma.txt"): (source/name).write_text(name)
            config=base/"config/molten-zharr/basalgrim/settings.json"; config.parent.mkdir(parents=True)
            config.write_text(json.dumps({"grid":grid,"preview":False}))
            env=dict(os.environ,TERM="xterm-256color",XDG_STATE_HOME=str(base/"state"),XDG_CONFIG_HOME=str(base/"config"))
            for key in ("TERM_PROGRAM","LC_TERMINAL","TMUX","WEZTERM_EXECUTABLE","KONSOLE_VERSION"): env.pop(key,None)
            master,slave=pty.openpty()
            fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack("HHHH",53,170,0,0))
            child=subprocess.Popen([str(binary),str(source)],stdin=slave,stdout=slave,stderr=slave,env=env)
            output=bytearray()
            def pump(seconds=.10):
                end=time.monotonic()+seconds
                while time.monotonic()<end:
                    if select.select([master],[],[],.01)[0]: output.extend(os.read(master,1<<20))
            def send(data): os.write(master,data.encode() if isinstance(data,str) else data); pump()
            def wait_for(predicate,message):
                end=time.monotonic()+6
                while not predicate():
                    assert child.poll() is None and time.monotonic()<end,message
                    pump(.02)
            def mouse(code,point,up=False): send(f"\x1b[<{code};{point[0]+1};{point[1]+1}{'m' if up else 'M'}")
            def band(start,end,ctrl=False,cancel=False):
                mouse(16 if ctrl else 0,start); mouse(48 if ctrl else 32,end)
                if cancel: send(b"\x1b")
                mouse(16 if ctrl else 0,end,True)
            def navigate(path): send(b"\x0c\x15"); send(str(path)); send(b"\r")
            try:
                wait_for(lambda:b"alpha.txt" in output,"initial directory")
                origin,point=((78,15),(44,10)) if grid else ((76,13),(26,12))
                band(origin,point); send(b"\x03"); navigate(destination); send(b"\x16")
                wait_for(lambda:all((destination/n).exists() for n in ("alpha.txt","beta.txt")),"rectangle copy")
                assert not (destination/"gamma.txt").exists()
                navigate(source); band(origin,point)
                gamma_origin,gamma_point=((96,15),(80,10)) if grid else ((76,14),(26,14))
                band(gamma_origin,gamma_point,ctrl=True)
                all_files=base/"all"; all_files.mkdir()
                send(b"\x03"); navigate(all_files); send(b"\x16")
                wait_for(lambda:all((all_files/n).exists() for n in ("alpha.txt","beta.txt","gamma.txt")),"Ctrl additive rectangle")
                navigate(source); band(origin,point); band(gamma_origin,gamma_point,cancel=True)
                cancelled=base/"cancelled"; cancelled.mkdir()
                send(b"\x03"); navigate(cancelled); send(b"\x16")
                wait_for(lambda:all((cancelled/n).exists() for n in ("alpha.txt","beta.txt")),"Escape selection restore")
                assert not (cancelled/"gamma.txt").exists()
                send(b"\x11"); child.wait(timeout=5); assert child.returncode==0
                print(f"Selection PTY passed: {'grid' if grid else 'list'}, exact copied set, Ctrl addition, Escape restore")
            finally:
                if child.poll() is None: child.terminate(); child.wait(timeout=5)
                os.close(master); os.close(slave)


if __name__=="__main__": main()
