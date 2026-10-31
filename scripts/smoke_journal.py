#!/usr/bin/env python3
"""Real PTY persistent history, raw SQLite records, crash recovery and mouse refresh."""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
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
    with tempfile.TemporaryDirectory(prefix='basalgrim-journal-') as tmp:
        base=Path(tmp);files=base/'files';files.mkdir();dest=base/'dest';dest.mkdir()
        (files/'seed.txt').write_text('seed')
        settings=base/'config/molten-zharr/basalgrim/settings.json';settings.parent.mkdir(parents=True)
        settings.write_text(json.dumps({'grid':False,'preview':False}))
        db=base/'state/molten-zharr/basalgrim/operations.sqlite3'
        env=dict(os.environ,TERM='xterm-256color',XDG_CONFIG_HOME=str(base/'config'),XDG_STATE_HOME=str(base/'state'))
        for name in ('TERM_PROGRAM','LC_TERMINAL','TMUX','WEZTERM_EXECUTABLE','KONSOLE_VERSION'):env.pop(name,None)
        child=None;master=slave=None;output=bytearray()
        def pump(seconds=.12):
            end=time.monotonic()+seconds
            while time.monotonic()<end:
                if select.select([master],[],[],.01)[0]:output.extend(os.read(master,1<<20))
        def send(value):os.write(master,value.encode() if isinstance(value,str) else value);pump()
        def text():return screen_text(output,170,40)
        def wait_for(predicate,label):
            end=time.monotonic()+10
            while not predicate():
                assert child.poll() is None and time.monotonic()<end,label
                pump(.01)
        def rows(sql,args=()):
            if not db.exists():return []
            try:
                with sqlite3.connect(db.as_uri()+'?mode=ro',uri=True,timeout=.2) as connection:return connection.execute(sql,args).fetchall()
            except sqlite3.Error:return []
        def launch():
            nonlocal child,master,slave,output
            master,slave=pty.openpty();output=bytearray()
            fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',40,170,0,0))
            child=subprocess.Popen([str(binary),'--no-session',str(files)],env=env,stdin=slave,stdout=slave,stderr=slave)
            wait_for(lambda:'seed.txt' in text() and bool(rows('SELECT id FROM owners')),'startup journal')
        def stop():
            send(b'\x11');child.wait(timeout=5);assert child.returncode==0
            os.close(master);os.close(slave)
        def navigate(path):send(b'\x0c\x15');send(str(path));send(b'\r')
        def search(name):send(b'/\x15');send(name);send(b'\r')
        def completed(count):wait_for(lambda:rows("SELECT COUNT(*) FROM operations WHERE state='done'")==[(count,)],'durable operation result')
        try:
            launch();send(b'\x0e');send('created.txt');send(b'\r');completed(1)
            search('created.txt');send(b'\x1b[15;5~');completed(2)
            assert (files/'created (1).txt').exists()
            actual=rows("SELECT destination FROM items WHERE kind='duplicate'")[0][0]
            assert os.fsdecode(actual)==str(files/'created (1).txt')
            send(b'\x1bOQ\x15');send('renamed.txt');send(b'\r');completed(3)
            search('renamed.txt');send(b'\x18');navigate(dest);send(b'\x16');completed(4)
            assert not (files/'renamed.txt').exists() and (dest/'renamed.txt').exists()
            stop();launch();send(b'\x1b[17~')
            wait_for(lambda:'Запись #4' in text() and 'Перемещение' in text(),'history restored after restart')
            send(b'\x1b[<0;5;2M\x1b[<0;5;2m')
            wait_for(lambda:'Последние 20 операций' in text(),'mouse journal refresh')
            send(b'\x1b');navigate(files)
            big=files/'big.bin';big.touch();os.truncate(big,512*1024*1024)
            send(b'\x12');search('big.bin');send(b'\x03');navigate(dest)
            os.write(master,b'\x16') # Poll before a large copy can finish.
            wait_for(lambda:bool(rows("SELECT destination FROM steps WHERE kind='staging' AND state='planned' ORDER BY id DESC LIMIT 1")),'staging intent committed before copying')
            # Keep finalization behind a write lock while simulating process death.
            blocker=sqlite3.connect(db,timeout=2);blocker.execute('BEGIN IMMEDIATE')
            active=blocker.execute("SELECT id FROM operations WHERE state='running' ORDER BY id DESC LIMIT 1").fetchone()
            assert active is not None
            candidates=blocker.execute("SELECT destination FROM steps WHERE operation_id=? AND kind='staging'",active).fetchall()
            child.kill();child.wait(timeout=5);os.close(master);os.close(slave);blocker.rollback();blocker.close()
            tracked=[big,dest/'big.bin',*[Path(os.fsdecode(row[0])) for row in candidates]]
            before={str(path):path.exists() for path in tracked}
            launch();send(b'\x1b[17~')
            wait_for(lambda:'Прервано: нужна проверка' in text(),'crashed owner recovered')
            assert rows('SELECT state FROM operations WHERE id=?',active)==[('interrupted',)]
            assert rows('SELECT state FROM items WHERE operation_id=?',active)==[('uncertain',)]
            assert {str(path):path.exists() for path in tracked}==before,'startup changed user files or staged data'
            assert 'Проверить временный путь' in text()
            stop()
            assert not (base/'state/molten-zharr/basalgrim/session.json').exists(),'--no-session unexpectedly saved tabs'
            assert db.stat().st_mode&0o777==0o600
            print('Journal PTY passed: create/duplicate/rename/move, actual targets, restart, mouse refresh, SIGKILL recovery and unchanged files')
        except BaseException:
            if child and child.poll() is None:print(text(),file=sys.stderr)
            raise
        finally:
            if child and child.poll() is None:child.terminate();child.wait(timeout=5);os.close(master);os.close(slave)


if __name__=='__main__':main()
