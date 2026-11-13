#!/usr/bin/env python3
"""Real 34x14 PTY appearance controls, palette files and persistence."""
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
sys.dont_write_bytecode=True
from appearance_fixture import Fixture,exercise,compact
from smoke_watch import screen_text
from pty_helpers import quit_when_ready

binary=Path(sys.argv[1] if len(sys.argv)>1 else 'target/release/basalgrim-tui').resolve()
with tempfile.TemporaryDirectory(prefix='basalgrim-appearance-pty-') as tmp:
    fixture=Fixture(Path(tmp));env=dict(os.environ,**fixture.env,TERM='xterm-256color')
    for name in ('DISPLAY','WAYLAND_DISPLAY','WAYLAND_SOCKET','TMUX','TERM_PROGRAM','LC_TERMINAL','WEZTERM_EXECUTABLE','KONSOLE_VERSION'):env.pop(name,None)
    settings=fixture.base/'config/molten-zharr/basalgrim/settings.json';settings.parent.mkdir(parents=True,exist_ok=True)
    # Fixture supplies private default appearance.
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
        keys={'Escape':b'\x1b','Return':b'\r','F10':b'\x1b[21~','Tab':b'\t','Home':b'\x1b[H','End':b'\x1b[F','Up':b'\x1b[A','Down':b'\x1b[B'}
        code=bytes([ord(name)&31]) if 'ctrl' in mods else keys.get(name,name.encode())
        send((b'\x1b' if 'alt' in mods else b'')+code)
    def click(x,y):send(f'\x1b[<0;{x+1};{y+1}M');send(f'\x1b[<0;{x+1};{y+1}m')
    def screen():pump(.15);return screen_text(output,34,14)
    def shot(name):Path('target/preview/appearance-pty-'+name+'.txt').write_text(screen())
    try:
        import appearance_fixture
        def wait(predicate,label):
            end=time.monotonic()+8
            while not predicate():
                assert time.monotonic()<end,label
                pump(.05)
        appearance_fixture.wait=wait
        pump(1)
        exercise(fixture,key,lambda s:send('\x1b[200~'+s+'\x1b[201~'),click,shot,(34,14))
        quit_when_ready(process,send)
        os.close(master);os.close(slave)
        master,slave,process=launch();output.clear();pump(1)
        compact(fixture,key,lambda s:send('\x1b[200~'+s+'\x1b[201~'),click,shot,(34,14))
        quit_when_ready(process,send)
        print('Appearance PTY: 34x14 mouse lifecycle passed')
    except BaseException:
        shot('failure');raise
    finally:
        if process.poll() is None:process.terminate();process.wait(timeout=5)
        os.close(master);os.close(slave);fixture.close()
