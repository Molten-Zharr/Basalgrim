#!/usr/bin/env python3
"""Actual 34x14 terminal properties, permissions and application chooser."""
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
from details_fixture import Fixture
from smoke_watch import screen_text
from pty_helpers import quit_when_ready


def main():
    binary=Path(sys.argv[1] if len(sys.argv)>1 else 'target/release/basalgrim-tui').resolve()
    with tempfile.TemporaryDirectory(prefix='bg-details-') as tmp:
        f=Fixture(Path(tmp));env=dict(os.environ,**f.env,TERM='xterm-256color')
        for name in ('DISPLAY','WAYLAND_DISPLAY','WAYLAND_SOCKET','TMUX','TERM_PROGRAM','LC_TERMINAL','WEZTERM_EXECUTABLE','KONSOLE_VERSION'):env.pop(name,None)
        settings=f.base/'config/molten-zharr/basalgrim/settings.json';settings.parent.mkdir(parents=True)
        settings.write_text(json.dumps({'grid':False,'preview':False,'icon_mode':'symbols'}))
        master,slave=pty.openpty();fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',14,34,0,0))
        child=subprocess.Popen([str(binary),'--no-session',str(f.root)],env=env,stdin=slave,stdout=slave,stderr=slave)
        output=bytearray()
        def pump(seconds=.12):
            end=time.monotonic()+seconds
            while time.monotonic()<end:
                if select.select([master],[],[],.01)[0]:output.extend(os.read(master,65536))
        def send(data):os.write(master,data.encode() if isinstance(data,str) else data);pump()
        def screen():return screen_text(output,34,14)
        def wait(predicate,label):
            end=time.monotonic()+8
            while not predicate():
                assert child.poll() is None and time.monotonic()<end,label+'\n'+screen()
                pump()
        def click(x,y,button=0):send(f'\x1b[<{button};{x+1};{y+1}M');send(f'\x1b[<{button};{x+1};{y+1}m')
        try:
            wait(lambda:'sample.txt' in screen(),'pane loaded')
            # Entry context menu, scroll to Properties if needed, with no keyboard shortcut.
            row=next(i for i,l in enumerate(screen().splitlines()) if 'sample.txt' in l)
            click(10,row,2);wait(lambda:'Свойства' in screen(),'contextual Properties')
            row=next(i for i,l in enumerate(screen().splitlines()) if 'Свойства' in l);click(5,row)
            wait(lambda:'Сведения' in screen(),'properties modal')
            click(16,2);wait(lambda:'Владелец' in screen(),'permissions tab')
            click(19,6);assert f.file.stat().st_mode&0o777==0o640
            click(3,11);wait(lambda:f.file.stat().st_mode&0o777==0o660,'save permissions');pump(.5)
            send(b'\x1b[13;3u');wait(lambda:'Сведения' in screen(),'Alt+Enter')
            click(16,2);click(19,6);click(25,12);assert f.file.stat().st_mode&0o777==0o660
            send(b'\x1b[13;5u');wait(lambda:'Fixture Alpha' in screen(),'Ctrl+Enter MIME chooser')
            send('Gamma');wait(lambda:'Нет программ' in screen(),'filtered recommended list')
            click(16,11);wait(lambda:'Fixture Gamma' in screen(),'all installed applications')
            click(3,4);pump(.45);assert not f.log.exists(),'single click launched an application'
            click(3,4);click(3,4);wait(f.log.exists,'double click launched chosen program')
            pump(.3);assert len((f.base/'launches.jsonl').read_text().splitlines())==1,'double click launched twice'
            result=json.loads(f.log.read_text());assert result['app']=='Gamma' and bytes.fromhex(result['path'])==os.fsencode(f.file),result
            # Same controls in the grid, keyboard Tab/Space, close without click-through.
            send(b'\x07');send(b'\x1b[13;3u');wait(lambda:'Сведения' in screen(),'grid properties')
            click(16,2);send(b'\t');send(' ');click(25,12)
            assert f.file.stat().st_mode&0o777==0o660
            quit_when_ready(child,send)
            print('File details PTY: 34x14 contextual mouse properties, Save/Cancel, Alt+Enter/Ctrl+Enter, filtered/all apps, single/double click, exact argv and grid passed',flush=True)
        except BaseException:
            Path('target/details-pty-failure.txt').write_text(screen());raise
        finally:
            if child.poll() is None:child.terminate();child.wait(timeout=5)
            os.close(master);os.close(slave);f.close()


if __name__=='__main__':main()
