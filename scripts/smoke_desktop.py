#!/usr/bin/env python3
"""Real 34x14 PTY input plus an independent D-Bus client; all state is private."""
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
from desktop_fixture import Fixture,ID
from smoke_watch import screen_text
from pty_helpers import quit_when_ready
binary=Path(sys.argv[1] if len(sys.argv)>1 else 'target/release/basalgrim-tui').resolve()
with tempfile.TemporaryDirectory(prefix='bg-desktop-pty-') as tmp:
    f=Fixture(Path(tmp));env=dict(f.env,TERM='xterm-256color')
    for n in ('DISPLAY','WAYLAND_DISPLAY','WAYLAND_SOCKET','TMUX','TERM_PROGRAM','LC_TERMINAL','WEZTERM_EXECUTABLE','KONSOLE_VERSION'):env.pop(n,None)
    settings=f.config/'molten-zharr/basalgrim/settings.json';settings.parent.mkdir(parents=True);settings.write_text(json.dumps({'grid':False,'preview':False,'icon_mode':'symbols'}))
    master,slave=pty.openpty();fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',14,34,0,0))
    child=subprocess.Popen([str(binary),'--file-manager','--no-session','--trash-home-only',str(f.root)],env=env,stdin=slave,stdout=slave,stderr=slave)
    out=bytearray()
    def pump(seconds=.1):
        end=time.monotonic()+seconds
        while time.monotonic()<end:
            if select.select([master],[],[],.01)[0]:out.extend(os.read(master,65536))
    def send(text):os.write(master,text.encode() if isinstance(text,str) else text);pump()
    def screen():return screen_text(out,34,14)
    def wait(predicate,label):
        end=time.monotonic()+8
        while not predicate():
            assert child.poll() is None and time.monotonic()<end,label+'\n'+screen()
            pump()
    def click(x,y):send(f'\x1b[<0;{x+1};{y+1}M');send(f'\x1b[<0;{x+1};{y+1}m')
    try:
        wait(lambda:'sample.txt' in screen(),'initial pane');pump(.4)
        f.call('ShowItems',[f.hidden,f.file]);wait(lambda:'.hidden' in screen(),'hidden reveal');pump(.3)
        send(b'\x03');pump(.3);send(b'\x0c');send(b'\x15');send(str(f.folder));send(b'\r');pump(.4);send(b'\x16')
        wait(lambda:(f.folder/f.file.name).exists() and (f.folder/f.hidden.name).exists(),'exact grouped selection copy')
        f.call('ShowItemProperties',[f.file]);wait(lambda:'Свойства' in screen(),'properties request');assert 'sample.txt' in screen(),screen();send(b'\x1b')
        f.call('ShowFolders',[f.folder]);pump(.4);send(b'\x0e');send('from-desktop');send(b'\r');wait(lambda:(f.folder/'from-desktop').exists(),'folder request')
        send(b'\x1b[18~');send('draft');f.call('ShowItems',[f.file]);pump(.4);assert 'draft' in screen(),screen();send(b'\x1b');pump(.4)
        r=subprocess.run([str(binary),'--desktop','--select',str(f.hidden)],env=env,capture_output=True,timeout=8);assert r.returncode==0,r.stderr
        wait(lambda:'.hidden' in screen(),'forwarded request')
        send(b'\x1b[21~');click(4,6);wait(lambda:'РАБОЧИЙ' in screen(),'compact System section');click(16,4)
        wait(lambda:(f.apps/ID).exists(),'compact Install');pump(.3);click(16,7)
        wait(lambda:ID in f.default(),'compact Default');pump(.3);send(b'\x1b[B');click(16,7)
        wait(lambda:'fixture-previous.desktop' in f.default(),'compact Restore')
        send(b'\x1b');quit_when_ready(child,send)
        print('Desktop PTY: 34x14 mouse install/default/restore, hidden grouped selection copied, property/folder requests, modal deferral and launcher forwarding passed')
    except BaseException:
        Path('target/desktop-pty-failure.txt').write_text(screen());raise
    finally:
        if child.poll() is None:child.terminate();child.wait(timeout=5)
        os.close(master);os.close(slave);f.close()
