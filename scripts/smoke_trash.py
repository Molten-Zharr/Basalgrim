#!/usr/bin/env python3
"""Real PTY freedesktop trash: gio, groups, compact mouse, conflicts and restart."""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import sqlite3
import struct
import subprocess
import sys
import tempfile
import termios
import time
sys.dont_write_bytecode = True
from smoke_watch import screen_text


def main():
    binary=Path(sys.argv[1] if len(sys.argv)>1 else 'target/debug/basalgrim-tui').resolve()
    with tempfile.TemporaryDirectory(prefix='basalgrim-trash-') as tmp:
        base=Path(tmp);files=base/'files';files.mkdir();destination=base/'destination';destination.mkdir()
        config=base/'config/molten-zharr/basalgrim/settings.json';config.parent.mkdir(parents=True)
        config.write_text(json.dumps({'grid':False,'preview':False,'icon_mode':'symbols'}))
        env=dict(os.environ,HOME=str(base),XDG_CONFIG_HOME=str(base/'config'),XDG_STATE_HOME=str(base/'state'),XDG_DATA_HOME=str(base/'data'),GIO_USE_VFS='local',TERM='xterm-256color')
        for name in ('DISPLAY', 'WAYLAND_DISPLAY', 'WAYLAND_SOCKET', 'TERM_PROGRAM','LC_TERMINAL','TMUX','WEZTERM_EXECUTABLE','KONSOLE_VERSION'):env.pop(name,None)
        alpha=files/'basalgrim-alpha.txt';beta=files/'basalgrim-кириллица.txt';tree=files/'basalgrim-tree'
        alpha.write_text('alpha content');beta.write_text('unicode content');tree.mkdir();(tree/'nested').write_text('tree content')
        subprocess.run(['gio','trash','--',str(alpha),str(beta),str(tree)],env=env,check=True)
        master,slave=pty.openpty();width,height=170,40;child=None;output=bytearray()
        def pump(seconds=.14):
            end=time.monotonic()+seconds
            while time.monotonic()<end:
                if select.select([master],[],[],max(0,min(.02,end-time.monotonic())))[0]:output.extend(os.read(master,1<<20))
        def send(value):os.write(master,value.encode() if isinstance(value,str) else value);pump()
        def text():return screen_text(output,width,height)
        def wait(predicate,label,seconds=12):
            end=time.monotonic()+seconds
            while not predicate():
                assert child.poll() is None and time.monotonic()<end,label;pump(.01)
        def idle():
            # Trash shares the normal status bar now; do not assume a footer row.
            wait(lambda:not any(marker in text() for marker in ('Стоп','Отмена…')),'operation acknowledged by UI')
        def click_label(label,toggle=False,button=0):
            rows=text().splitlines();y=next(i for i,row in enumerate(rows) if label in row);x=rows[y].index(label)+1
            code=16 if toggle else button
            send(f'\x1b[<{code};{x+1};{y+1}M\x1b[<{code};{x+1};{y+1}m')
        def launch(path=files):
            nonlocal child,output
            output=bytearray();fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',height,width,0,0))
            child=subprocess.Popen([str(binary),'--no-session','--trash-home-only',str(path)],env=env,stdin=slave,stdout=slave,stderr=slave)
            wait(lambda:'BASALGRIM' in text(),'startup');pump(.3)
        def open_trash():
            send(b'\x1b[21;2~');click_label('Открыть корзину');wait(lambda:'Корзина' in text() and 'ФАЙЛЫ' in text(),'trash in ordinary file pane')
            send('/');send(str(base));send(b'\r');wait(lambda:'basalgrim-' in text(),'private trash listing')
        def restore_original():click_label('Восстановить');wait(lambda:'Восстановить из корзины?' in text(),'confirmation');send(b'\r')
        def close():
            idle()
            # Filesystem publication can precede App consuming worker completion.
            # Respect the busy Quit guard and retry within a fixed deadline.
            deadline=time.monotonic()+8
            while child.poll() is None:
                send(b'\x11')
                try:child.wait(timeout=.15)
                except subprocess.TimeoutExpired:
                    assert time.monotonic()<deadline,'trash worker did not finish before Quit'
            assert child.returncode==0
        try:
            launch();open_trash()
            wait(lambda:alpha.name in text() and beta.name in text() and tree.name in text(),'gio items including Unicode and directory')
            click_label(alpha.name,True);click_label(beta.name,True);restore_original()
            wait(lambda:alpha.exists() and beta.exists(),'mouse group restoration');idle()
            assert alpha.read_text()=='alpha content' and beta.read_text()=='unicode content'
            send(b'\x1a');wait(lambda:not alpha.exists() and not beta.exists(),'undo restore');idle()
            send(b'\x19');wait(lambda:alpha.exists() and beta.exists(),'redo restore');idle()
            click_label(tree.name,button=2);wait(lambda:'КОРЗИНА' in text(),'context menu')
            click_label('Восстановить из корзины');send(b'\r');wait(tree.exists,'context restore directory');idle()
            assert (tree/'nested').read_text()=='tree content'
            close()
            # New gio trash content remains visible across application restarts.
            subprocess.run(['gio','trash','--',str(alpha)],env=env,check=True);alpha.write_text('existing destination')
            launch(destination);open_trash()
            width,height=34,14;fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',height,width,0,0));child.send_signal(signal.SIGWINCH);pump(.3)
            click_label('Восстановить');send(b'\r');wait(lambda:'Совпадение имён' in text(),'compact restore conflict')
            send(b'\r');restored=files/'basalgrim-alpha (1).txt';wait(restored.exists,'Keep Both preserves occupied destination');idle()
            assert restored.read_text()=='alpha content' and alpha.read_text()=='existing destination'
            close()
            subprocess.run(['gio','trash','--',str(beta)],env=env,check=True)
            width,height=170,40;launch(destination);open_trash();click_label(beta.name)
            send(b'\x1b[21;2~');click_label('Восстановить в текущую папку');send(b'\r');wait(lambda:(destination/beta.name).exists(),'restore into captured current directory');idle()
            assert (destination/beta.name).read_text()=='unicode content' and not beta.exists()
            close();print('Trash PTY passed: gio items, Unicode, groups, directory, context menu, undo/redo, restart, compact Keep Both and current-folder restore',flush=True)
            # Selected permanent deletion and whole-trash empty are separate commands.
            purge_a=files/'basalgrim-delete-a.txt';purge_b=files/'basalgrim-delete-b.txt'
            purge_a.write_text('delete only selected');purge_b.write_text('empty hidden item')
            subprocess.run(['gio','trash','--',str(purge_a),str(purge_b)],env=env,check=True)
            trash_files=base/'data/Trash/files';trash_info=base/'data/Trash/info'
            launch();open_trash();click_label(purge_a.name,button=2);click_label('Удалить из корзины')
            wait(lambda:'Удалить безвозвратно?' in text(),'permanent-delete confirmation');send(b'\x1b')
            assert (trash_files/purge_a.name).exists() and (trash_files/purge_b.name).exists()
            send(b'\x1b[3~');send(b'\r');wait(lambda:not (trash_files/purge_a.name).exists(),'selected deletion');idle()
            assert (trash_files/purge_b.name).exists() and not (trash_info/(purge_a.name+'.trashinfo')).exists()
            send('/');send('no-matching-file');send(b'\r');click_label('Очистить корзину')
            wait(lambda:'Очистить всю корзину?' in text(),'empty full trash while filter hides everything')
            send(b'\x1b');assert (trash_files/purge_b.name).exists()
            click_label('Очистить корзину');wait(lambda:'Очистить всю корзину?' in text(),'empty confirmation again');send(b'\r')
            wait(lambda:not any(trash_files.iterdir()) and not any(trash_info.iterdir()),'whole-trash cleanup');idle()
            assert alpha.read_text()=='existing destination' and (destination/beta.name).read_text()=='unicode content'
            close();print('Trash PTY delete: selected only, cancel, whole-trash empty through an empty filter, intact originals passed',flush=True)
            # Stop after the metadata claim, before publication. A journal write
            # lock keeps the next durable intent behind a bounded hashing pass.
            large=files/'basalgrim-crash.bin'
            with large.open('wb') as file:file.truncate(512*1024*1024)
            subprocess.run(['gio','trash','--',str(large)],env=env,check=True)
            launch();open_trash();restore_original()
            db=base/'state/molten-zharr/basalgrim/operations.sqlite3'
            def claim():
                with sqlite3.connect(db,timeout=.2) as connection:
                    return connection.execute("SELECT s.source,s.destination,i.source FROM steps s JOIN items i ON i.operation_id=s.operation_id AND i.sequence=s.item WHERE s.kind='trash_info_claim' AND s.state='done' AND s.operation_id=(SELECT MAX(id) FROM operations)").fetchone()
            wait(lambda:claim() is not None,'durable metadata claim',30)
            blocker=sqlite3.connect(db,timeout=2);blocker.execute('BEGIN IMMEDIATE');row=claim()
            assert row and not large.exists(),'missed restore claim crash window'
            info,parked,data=(Path(os.fsdecode(value)) for value in row)
            assert parked.exists() and data.exists() and not info.exists()
            child.kill();child.wait(timeout=5);blocker.rollback();blocker.close()
            before={str(p):(p.exists(),p.stat().st_size if p.exists() else None) for p in (large,info,parked,data)}
            launch();send(b'\x1b[17~');wait(lambda:'Прервано: нужна проверка' in text(),'interrupted restoration history')
            assert {str(p):(p.exists(),p.stat().st_size if p.exists() else None) for p in (large,info,parked,data)}==before,'startup replayed or removed claimed trash data'
            send(b'\x1b');close()
            print('Trash PTY crash recovery: SIGKILL preserves claimed metadata and data without automatic replay',flush=True)
        except BaseException:
            print(text(),file=sys.stderr);raise
        finally:
            if child and child.poll() is None:child.kill();child.wait(timeout=5)
            os.close(master);os.close(slave)


if __name__=='__main__':main()
