#!/usr/bin/env python3
"""34x14 ordinary create and automatic pkexec routing; root disabled by no_new_privs."""
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
from details_fixture import Fixture
from privileged_fixture import sandbox, last_operation, operation_id
from smoke_watch import screen_text
from pty_helpers import quit_when_ready
binary=Path(sys.argv[1] if len(sys.argv)>1 else 'target/release/basalgrim-tui').resolve()
with tempfile.TemporaryDirectory(prefix='bg-privileged-pty-') as tmp:
    f=Fixture(Path(tmp));env=dict(os.environ,**f.env,TERM='xterm-256color')
    for n in ('DISPLAY','WAYLAND_DISPLAY','WAYLAND_SOCKET','TMUX','TERM_PROGRAM','LC_TERMINAL','WEZTERM_EXECUTABLE','KONSOLE_VERSION'):env.pop(n,None)
    settings=f.base/'config/molten-zharr/basalgrim/settings.json';settings.parent.mkdir(parents=True)
    settings.write_text(json.dumps({'grid':False,'preview':False,'icon_mode':'symbols'}))
    master,slave=pty.openpty();fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',14,34,0,0))
    child=subprocess.Popen([str(binary),'--no-session','--trash-home-only',str(f.root)],env=env,stdin=slave,stdout=slave,stderr=slave,preexec_fn=sandbox)
    out=bytearray()
    def pump(seconds=.12):
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
    def click(x,y,button=0):send(f'\x1b[<{button};{x+1};{y+1}M');send(f'\x1b[<{button};{x+1};{y+1}m')
    try:
        wait(lambda:'sample.txt' in screen(),'pane');pump(.4)
        send(b'\x0e');wait(lambda:'Новый файл' in screen(),'ordinary create dialog')
        send('Проверка.txt');click(5,10)
        wait(lambda:(f.root/'Проверка.txt').exists(),'create should complete without administrator wizard')
        wait(lambda:last_operation(f.root) and last_operation(f.root)[0]=='done','normal history')
        f.root.chmod(0o500)
        try:
            send(b'\x0e');send('protected.txt');click(5,10)
            wait(lambda:last_operation(f.root) and last_operation(f.root)[0]=='failed','denied create should reach pkexec')
            assert 'pkexec' in str(last_operation(f.root)),last_operation(f.root)
            assert not (f.root/'protected.txt').exists()
            assert 'Администратор' not in screen() and 'Далее:' not in screen(),screen()
        finally:f.root.chmod(0o700)
        send(b'\x1b')
        target=f.base/'untouched';target.write_text('keep target')
        link=f.root/'syslog.service';link.symlink_to(target)
        send(b'\x1b[15~');pump(.3);send('/');send(link.name);send(b'\r');pump(.2)
        before=operation_id(f.root);f.root.chmod(0o500)
        try:
            send(b'\x1b[19~');wait(lambda:'В корзину' in screen(),'trash confirmation');click(5,11)
            wait(lambda:operation_id(f.root)>before and last_operation(f.root) and last_operation(f.root)[0]=='failed','protected Trash must reach pkexec')
            assert 'pkexec' in str(last_operation(f.root)),last_operation(f.root)
            assert link.is_symlink() and target.read_text()=='keep target'
            assert 'Администратор' not in screen(),screen()
        finally:f.root.chmod(0o700)
        send(b'\x1b');send('/');send(link.name);send(b'\r');send(b'\x1b[19~');click(5,11)
        wait(lambda:not link.is_symlink(),'ordinary Trash');assert target.read_text()=='keep target'
        assert any(p.is_symlink() and os.readlink(p)==str(target) for p in (f.base/'data/Trash/files').iterdir())
        send(b'\x1b');quit_when_ready(child,send)
        print('Automatic authorization PTY: protected symlink Trash requests pkexec, denial preserves source, ordinary Trash reaches caller catalog; create regression passed')
    except BaseException:
        Path('target/privileged-pty-failure.txt').write_text(screen());raise
    finally:
        if child.poll() is None:child.terminate();child.wait(timeout=5)
        os.close(master);os.close(slave);f.close()
