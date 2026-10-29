#!/usr/bin/env python3
"""Real PTY conflict decisions, batch scope, cancellation, rename and compact mouse controls."""
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
    binary=Path(sys.argv[1] if len(sys.argv)>1 else "target/debug/basalgrim-tui").resolve()
    with tempfile.TemporaryDirectory(prefix="basalgrim-conflicts-") as tmp:
        base=Path(tmp); source=base/"source"; dest=base/"destination"; source.mkdir();dest.mkdir()
        for name in ("a.txt","b.txt"):
            (source/name).write_text("incoming "+name);(dest/name).write_text("existing "+name)
        config=base/"config/molten-zharr/basalgrim/settings.json";config.parent.mkdir(parents=True);config.write_text(json.dumps({"grid":False,"preview":False}))
        env=dict(os.environ,TERM="xterm-256color",XDG_CONFIG_HOME=str(base/"config"))
        for name in ("TERM_PROGRAM","LC_TERMINAL","TMUX","WEZTERM_EXECUTABLE","KONSOLE_VERSION"):env.pop(name,None)
        master,slave=pty.openpty(); width,height=170,40
        fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack("HHHH",height,width,0,0))
        child=subprocess.Popen([str(binary),str(source)],env=env,stdin=slave,stdout=slave,stderr=slave);output=bytearray()
        def pump(seconds=.15):
            end=time.monotonic()+seconds
            while time.monotonic()<end:
                if select.select([master],[],[],.01)[0]:output.extend(os.read(master,1<<20))
        def send(value):os.write(master,value.encode() if isinstance(value,str) else value);pump()
        def text():return screen_text(output,width,height)
        def wait_for(predicate,message):
            end=time.monotonic()+8
            while not predicate():
                assert child.poll() is None and time.monotonic()<end,message
                pump(.02)
        def navigate(path):send(b"\x0c\x15");send(str(path));send(b"\r")
        def click(x,y):send(f"\x1b[<0;{x+1};{y+1}M");send(f"\x1b[<0;{x+1};{y+1}m")
        def prompt():wait_for(lambda:"Совпадение имён" in text(),"conflict dialog")
        def closed():wait_for(lambda:"Совпадение имён" not in text(),"decision resumed worker")
        try:
            wait_for(lambda:b"a.txt" in output,"initial files");send(b"\x01\x03");navigate(dest);send(b"\x16");prompt()
            click(55,20);click(55,24) # Scope and Keep Both.
            wait_for(lambda:(dest/"a (1).txt").exists() and (dest/"b (1).txt").exists(),"keep both for remaining conflicts")
            for name in ("a.txt","b.txt"):assert (dest/name).read_text()=="existing "+name
            send(b"\x16");prompt();send(b" ");click(55,22) # Replace all, mouse decision.
            wait_for(lambda:all((dest/name).read_text()=="incoming "+name for name in ("a.txt","b.txt")),"replace all")
            for name in ("a.txt","b.txt"):(dest/name).write_text("kept "+name);(source/name).write_text("newer "+name)
            send(b"\x16");prompt();send(b" \x1b[A\r");closed() # Scope + Skip.
            for name in ("a.txt","b.txt"):assert (dest/name).read_text()=="kept "+name
            assert not (dest/"a (2).txt").exists()
            for cancel in (b"\x1b",b"\x1b[17;5~"):
                send(b"\x16");prompt();send(cancel);closed()
            navigate(source);send(b"\x1bOQ\x15b.txt\r");prompt();send(b"\r")
            wait_for(lambda:(source/"b (1).txt").exists() and not (source/"a.txt").exists(),"rename keep both")
            assert (source/"b.txt").read_text()=="newer b.txt"
            send(b"\x01\x03\x16");prompt()
            width,height=34,14;fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack("HHHH",height,width,0,0));child.send_signal(signal.SIGWINCH);pump(.3)
            assert "Сохранить оба" in text()
            click(5,8);assert "Совпадение имён" in text(),"disabled Replace resolved a self-copy"
            click(5,11);closed()
            assert sorted(p.name for p in source.iterdir())==["b (1).txt","b.txt"]
            assert not list(base.rglob(".basalgrim-*"))
            send(b"\x11");child.wait(timeout=5);assert child.returncode==0
            print("Conflict PTY passed: mouse/keyboard decisions, batch scope, keep-both, replace, skip, Esc/Stop, rename and compact disabled controls")
        except BaseException:
            print(text(),file=sys.stderr);raise
        finally:
            if child.poll() is None:child.terminate();child.wait(timeout=5)
            os.close(master);os.close(slave)


if __name__=="__main__":main()
