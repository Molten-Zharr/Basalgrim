#!/usr/bin/env python3
"""Command palette through real SGR mouse and keyboard input at 34x14."""
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
from smoke_watch import screen_text
from pty_helpers import quit_when_ready


def main():
    binary=Path(sys.argv[1] if len(sys.argv)>1 else 'target/release/basalgrim-tui').resolve()
    with tempfile.TemporaryDirectory(prefix='bg-palette-') as tmp:
        f=Fixture(Path(tmp));env=dict(os.environ,**f.env,TERM='xterm-256color')
        for n in ('DISPLAY','WAYLAND_DISPLAY','WAYLAND_SOCKET','TMUX','TERM_PROGRAM','LC_TERMINAL','WEZTERM_EXECUTABLE','KONSOLE_VERSION'):env.pop(n,None)
        settings=f.base/'config/molten-zharr/basalgrim/settings.json';settings.parent.mkdir(parents=True)
        settings.write_text(json.dumps({'grid':False,'preview':False,'icon_mode':'symbols'}))
        master,slave=pty.openpty();fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',14,34,0,0))
        child=subprocess.Popen([str(binary),'--no-session','--trash-home-only',str(f.root)],env=env,stdin=slave,stdout=slave,stderr=slave)
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
        def palette(query):
            send(b'\x1b[112;6u');wait(lambda:'Поиск команды' in screen(),'palette shortcut')
            send(query)
        try:
            wait(lambda:'sample.txt' in screen(),'pane')
            click(5,2);wait(lambda:'Поиск команды' in screen(),'mouse Actions entry')
            row=next(i for i,l in enumerate(screen().splitlines()) if 'Поиск команды' in l);click(4,row)
            wait(lambda:'Найдено:' in screen(),'palette opened by mouse')
            send('new folder');wait(lambda:'Новая папка' in screen(),'English alias')
            click(4,3);pump(.45);assert 'Найдено:' in screen(),'single click ran command'
            click(4,3);click(4,3);wait(lambda:'Найдено:' not in screen(),'double click command')
            send('Создано');send(b'\r');wait(lambda:(f.root/'Создано').is_dir(),'folder operation');pump(.5)
            palette('redo');click(4,12);assert 'Нет доступной операции' in screen(),screen()
            # Query selection and Cyrillic bracketed paste remain ordinary text editing.
            send(b'\x01');send('\x1b[200~Новый файл\x1b[201~');wait(lambda:'Новый файл' in screen(),'Russian query')
            click(4,12);send('Новый.txt');send(b'\r');wait(lambda:(f.root/'Новый.txt').exists(),'compact mouse Run');pump(.5)
            palette('F2');send(b'\r');send(b'\x01');send('Переименовано.txt');send(b'\r')
            wait(lambda:(f.root/'Переименовано.txt').exists(),'shortcut lookup Rename');pump(.5)
            palette('В корзину');send(b'\r');wait(lambda:'Найдено:' not in screen(),'confirmation shown')
            send(b'\x1b');assert (f.root/'Переименовано.txt').exists(),'confirmation bypassed'
            palette('xxxx_unknown');wait(lambda:'Ничего не найдено' in screen(),'no matches')
            click(20,12);wait(lambda:'Найдено: 0' not in screen(),'mouse clear')
            send(b'\x1b');quit_when_ready(child,send)
            print('Command palette PTY: 34x14 mouse entry, Cyrillic/alias/shortcut search, single/double click, disabled reasons, Create/Rename, confirmation and Clear passed',flush=True)
        except BaseException:
            Path('target/palette-pty-failure.txt').write_text(screen());raise
        finally:
            if child.poll() is None:child.terminate();child.wait(timeout=5)
            os.close(master);os.close(slave);f.close()


if __name__=='__main__':main()
