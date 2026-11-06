#!/usr/bin/env python3
"""Real PTY inline editing, Unicode ranges, mouse controls and OSC 52 text copy."""
import base64
import fcntl
import json
import os
from pathlib import Path
import pty
import re
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
from pty_helpers import quit_when_ready


def main():
    binary=Path(sys.argv[1] if len(sys.argv)>1 else 'target/debug/basalgrim-tui').resolve()
    for grid in (False,True):
        with tempfile.TemporaryDirectory(prefix='basalgrim-text-pty-') as tmp:
            base=Path(tmp);folder=base/'files';folder.mkdir();(folder/'alpha.txt').write_text('PTY editor data')
            dest=base/'destination';dest.mkdir()
            settings=base/'config/molten-zharr/basalgrim/settings.json';settings.parent.mkdir(parents=True)
            settings.write_text(json.dumps({'grid':grid,'preview':False,'icon_mode':'symbols'}))
            env=dict(os.environ,TERM='xterm-256color',HOME=str(base),XDG_CONFIG_HOME=str(base/'config'),XDG_STATE_HOME=str(base/'state'),XDG_DATA_HOME=str(base/'data'))
            for name in ('DISPLAY','WAYLAND_DISPLAY','WAYLAND_SOCKET','SSH_CONNECTION','SSH_TTY','TMUX','TERM_PROGRAM','LC_TERMINAL'):
                env.pop(name,None)
            width,height=34,14
            master,slave=pty.openpty();fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',height,width,0,0))
            child=subprocess.Popen([str(binary),'--no-session',str(folder)],stdin=slave,stdout=slave,stderr=slave,env=env)
            output=bytearray()
            def pump(seconds=.12):
                end=time.monotonic()+seconds
                while time.monotonic()<end:
                    if select.select([master],[],[],max(0,min(.02,end-time.monotonic())))[0]:output.extend(os.read(master,1<<20))
            def send(value):os.write(master,value.encode() if isinstance(value,str) else value);pump()
            def screen():
                clean=re.sub(rb'\x1b\][^\x07]*?(?:\x07|\x1b\\)',b'',output)
                return screen_text(clean,width,height)
            def wait(predicate,label):
                end=time.monotonic()+8
                while not predicate():
                    assert child.poll() is None and time.monotonic()<end,label
                    pump(.02)
            def click_label(label,button=0):
                rows=screen().splitlines();y=next(i for i,row in enumerate(rows) if label in row);x=rows[y].index(label)
                send(f'\x1b[<{button};{x+1};{y+1}M\x1b[<{button};{x+1};{y+1}m')
            def copied(start,expected):
                wait(lambda:b'\x1b]52;c;' in output[start:],'text clipboard result')
                payload=re.findall(rb'\x1b\]52;c;([^\x07\x1b]*)(?:\x07|\x1b\\)',output[start:])[-1]
                assert base64.b64decode(payload).decode()==expected
            try:
                # A minimal grid may initially have room only for its symbol icon.
                wait(lambda:'элементов' in screen() or '1 / 1' in screen(),'initial compact frame')
                send(b'\x1bOQ');wait(lambda:'alpha.txt' in screen(),'inline rename at 34x14')
                send(b'\x1b[H\x1b[C\x1b[1;2C\x1b[1;2C')
                start=len(output);send(b'\x03');copied(start,'lp')
                send(b'\x7f');send('\x1b[200~e\u0301文件\x1b[201~')
                send(b'\x01');send('saved.txt')
                click_label('Ввод');wait(lambda:(folder/'saved.txt').exists(),'mouse inline confirmation')
                assert (folder/'saved.txt').read_text()=='PTY editor data'
                pump(.25);send(b'\x03');send(b'\x1bOQ')
                click_label('saved.txt',2);wait(lambda:'Выделить всё' in screen(),'small text context menu')
                start=len(output);click_label('Вырезать');copied(start,'saved')
                send(b'\x1b');assert (folder/'saved.txt').exists(),'Escape changed the source'
                send(b'\x0c');send(str(dest));send(b'\r');pump(.25);send(b'\x16')
                wait(lambda:(dest/'saved.txt').exists(),'text copy retained file clipboard')
                assert (dest/'saved.txt').read_text()=='PTY editor data'
                # Exercise a directory rename too, inspecting all terminal
                # output during the refresh instead of only the final frame.
                original=dest/'a-folder';original.mkdir()
                (original/'child.txt').write_text('keep folder contents')
                for index in range(1000):(dest/f'file-{index:04}.txt').touch()
                pump(.8);send(b'\x1b[H');send(b'\x1bOQ')
                wait(lambda:'a-folder' in screen(),'folder inline editor')
                send('a-renamed');start=len(output);send(b'\r')
                wait(lambda:(dest/'a-renamed').is_dir(),'directory renamed')
                pump(.5)
                refresh=re.sub(rb'\x1b\[[0-?]*[ -/]*[@-~]',b'',output[start:]).decode(errors='replace')
                assert 'Чтение каталога' not in refresh,'directory rename blanked the file pane'
                assert not original.exists()
                assert (dest/'a-renamed/child.txt').read_text()=='keep folder contents'
                quit_when_ready(child,send)
                print(f'Text editing PTY grid={grid}: compact inline rename, Shift ranges, Unicode paste, mouse menus/confirm, OSC 52, retained file clipboard and directory rename without blank frames passed')
            except BaseException:
                print(screen(),file=sys.stderr);raise
            finally:
                if child.poll() is None:child.kill();child.wait(timeout=5)
                os.close(master);os.close(slave)


if __name__=='__main__':main()
