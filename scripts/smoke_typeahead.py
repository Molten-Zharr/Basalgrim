#!/usr/bin/env python3
"""Real 34x14 PTY typeahead input, deadlines, Backspace and persistence."""
import fcntl
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
sys.dont_write_bytecode=True
from typeahead_fixture import Fixture,exercise,compact
from smoke_watch import screen_text
from pty_helpers import quit_when_ready

binary=Path(sys.argv[1] if len(sys.argv)>1 else 'target/release/basalgrim-tui').resolve()
Path('target/preview').mkdir(parents=True,exist_ok=True)
with tempfile.TemporaryDirectory(prefix='basalgrim-typeahead-pty-') as tmp:
    fixture=Fixture(Path(tmp));env=dict(os.environ,**fixture.env,TERM='xterm-256color')
    for name in ('DISPLAY','WAYLAND_DISPLAY','WAYLAND_SOCKET','TMUX','TERM_PROGRAM','LC_TERMINAL','WEZTERM_EXECUTABLE','KONSOLE_VERSION'):env.pop(name,None)
    def launch():
        master,slave=pty.openpty();fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',14,34,0,0))
        process=subprocess.Popen([str(binary),'--no-session','--trash-home-only',str(fixture.root)],env=env,stdin=slave,stdout=slave,stderr=slave)
        return master,slave,process
    master,slave,process=launch()
    output=bytearray()
    def pump(seconds=.12):
        end=time.monotonic()+seconds
        while time.monotonic()<end:
            if select.select([master],[],[],.01)[0]:output.extend(os.read(master,65536))
    def send(text):os.write(master,text.encode() if isinstance(text,str) else text);pump()
    def key(name,*mods):
        keys={'Escape':b'\x1b','Return':b'\r','F10':b'\x1b[21~','Tab':b'\t','Home':b'\x1b[H','End':b'\x1b[F','Up':b'\x1b[A','Down':b'\x1b[B','F2':b'\x1bOQ','BackSpace':b'\x7f'}
        code=b'\x1b[127;5u' if name=='BackSpace' and 'ctrl' in mods else b'\x1b[103;6u' if name=='g' and 'shift' in mods else bytes([ord(name)&31]) if 'ctrl' in mods else keys.get(name,name.encode())
        send((b'\x1b' if 'alt' in mods else b'')+code)
    def click(x,y,button=1):
        code=2 if button==3 else 0
        send(f'\x1b[<{code};{x+1};{y+1}M');send(f'\x1b[<{code};{x+1};{y+1}m')
    def typing(value):send(value)
    def screen():pump(.15);return screen_text(output,34,14)
    def shot(name):Path('target/preview/typeahead-pty-'+name+'.txt').write_text(screen())
    try:
        import typeahead_fixture
        def wait(predicate,label):
            end=time.monotonic()+8
            while not predicate():
                assert time.monotonic()<end,label
                pump(.05)
        typeahead_fixture.wait=wait
        pump(1)
        exercise(fixture,key,lambda s:send('\x1b[200~'+s+'\x1b[201~'),click,typing,shot,(34,14))
        quit_when_ready(process,send)
        os.close(master);os.close(slave)
        master,slave,process=launch();output.clear();pump(1)
        compact(fixture,key,lambda s:send('\x1b[200~'+s+'\x1b[201~'),click,typing,shot,(34,14))
        quit_when_ready(process,send)
        print('Typeahead PTY: 34x14 keyboard/mouse lifecycle passed')
    except BaseException:
        shot('failure');raise
    finally:
        if process.poll() is None:process.terminate();process.wait(timeout=5)
        os.close(master);os.close(slave);fixture.close()
