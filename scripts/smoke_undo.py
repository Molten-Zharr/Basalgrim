#!/usr/bin/env python3
"""Real PTY undo/redo, isolated trash, compact mouse, restart and crash recovery."""
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
import time
sys.dont_write_bytecode = True
from smoke_watch import screen_text


def main():
    binary=Path(sys.argv[1] if len(sys.argv)>1 else 'target/debug/basalgrim-tui').resolve()
    with tempfile.TemporaryDirectory(prefix='basalgrim-undo-') as tmp:
        base=Path(tmp);files=base/'files';files.mkdir();(files/'a.txt').write_text('original content')
        settings=base/'config/molten-zharr/basalgrim/settings.json';settings.parent.mkdir(parents=True)
        settings.write_text(json.dumps({'grid':False,'preview':False,'icon_mode':'symbols'}))
        db=base/'state/molten-zharr/basalgrim/operations.sqlite3'
        env=dict(os.environ,HOME=str(base),TERM='xterm-256color',XDG_CONFIG_HOME=str(base/'config'),XDG_STATE_HOME=str(base/'state'),XDG_DATA_HOME=str(base/'data'))
        for name in ('TERM_PROGRAM','LC_TERMINAL','TMUX','WEZTERM_EXECUTABLE','KONSOLE_VERSION'):env.pop(name,None)
        child=None;master=slave=None;output=bytearray()
        def pump(seconds=.1):
            end=time.monotonic()+seconds
            while time.monotonic()<end:
                if select.select([master],[],[],min(.01,max(0,end-time.monotonic())))[0]:output.extend(os.read(master,1<<20))
        def send(value):os.write(master,value);pump()
        def rows(sql,args=()):
            if not db.exists():return []
            try:
                with sqlite3.connect(db.as_uri()+'?mode=ro',uri=True,timeout=.2) as connection:return connection.execute(sql,args).fetchall()
            except sqlite3.Error:return []
        def wait(predicate,label,seconds=15):
            end=time.monotonic()+seconds
            while not predicate():
                assert child.poll() is None and time.monotonic()<end,label
                pump(.01)
            pump(.16)
        def launch(path=files):
            nonlocal child,master,slave,output
            master,slave=pty.openpty();output=bytearray()
            fcntl.ioctl(slave,termios_size,struct.pack('HHHH',40,170,0,0))
            child=subprocess.Popen([str(binary),'--no-session',str(path)],env=env,stdin=slave,stdout=slave,stderr=slave)
            wait(lambda:'BASALGRIM' in screen_text(output,170,40) and bool(rows('SELECT id FROM owners')),'startup')
        def stop():
            send(b'\x11');child.wait(timeout=5);assert child.returncode==0;os.close(master);os.close(slave)
        def click(x,y):send(f'\x1b[<0;{x};{y}M\x1b[<0;{x};{y}m'.encode())
        def resize(width,height):
            fcntl.ioctl(slave,termios_size,struct.pack('HHHH',height,width,0,0));child.send_signal(signal.SIGWINCH);pump(.25)
        import termios
        termios_size=termios.TIOCSWINSZ
        copied=files/'a (1).txt'
        try:
            launch();send(b'\x1b[15;5~');wait(copied.exists,'duplicate');send(b'\x1a');wait(lambda:not copied.exists(),'Ctrl+Z')
            send(b'\x19');wait(copied.exists,'Ctrl+Y');assert copied.read_text()=='original content'
            send(b'\x1b[17~');click(5,38);wait(lambda:not copied.exists(),'F6 mouse undo');click(23,38);wait(copied.exists,'F6 mouse redo')
            resize(34,14);click(5,12);wait(lambda:not copied.exists(),'compact undo');click(13,12);wait(copied.exists,'compact redo')
            send(b'\x1b');resize(170,40)
            send(b'\x1a');wait(lambda:not copied.exists(),'undo before restart');stop();launch()
            copied.write_text('external destination');pump(.2);send(b'\x19');pump(.3);assert copied.read_text()=='external destination','redo overwrote conflict'
            copied.unlink();pump(.2);send(b'\x19');wait(copied.exists,'redo after restart');assert copied.read_text()=='original content'
            copied.write_text('external edit');pump(.2);send(b'\x1a');pump(.3);assert copied.read_text()=='external edit','undo hid modified data'
            # Explicitly select the original after reload; no user trash is touched.
            send(b'/\x15a.txt\r');send(b'\x1b[19~');send(b'\r');wait(lambda:not (files/'a.txt').exists(),'gio trash')
            send(b'\x1a');wait(lambda:(files/'a.txt').exists(),'undo trash');send(b'\x19');wait(lambda:not (files/'a.txt').exists(),'redo trash')
            stop();launch();send(b'\x1a');wait(lambda:(files/'a.txt').exists(),'restore trash after restart');stop()
            print('Undo PTY: shortcuts, compact mouse, restart, conflicts, changes and isolated trash passed',flush=True)
            crash=base/'crash';crash.mkdir();large=crash/'large.bin'
            with large.open('wb') as file:file.truncate(512*1024*1024)
            launch(crash);send(b'\x1b[15;5~');result=crash/'large (1).bin'
            wait(lambda:bool(rows("SELECT id FROM operations WHERE state='done' AND title='Дублирование' AND id=(SELECT MAX(id) FROM operations)")),'large duplicate completed',30)
            assert result.exists()
            os.write(master,b'\x1a')
            end=time.monotonic()+15
            while result.exists():
                assert child.poll() is None and time.monotonic()<end,'undo publication before crash'
                pump(.001)
            child.send_signal(signal.SIGSTOP)
            pending=rows("SELECT operation_id,sequence,data FROM undo_items WHERE state='busy'")
            assert pending,'missed pending undo crash window'
            plan=json.loads(pending[0][2]);retained=Path(os.fsdecode(bytes(plan['parking'])))
            assert retained.exists() and retained.stat().st_size==512*1024*1024
            child.kill();child.wait(timeout=5);os.close(master);os.close(slave)
            launch(crash)
            wait(lambda:rows('SELECT state FROM undo_items WHERE operation_id=? AND sequence=?',(pending[0][0],pending[0][1]))==[('uncertain',)],'interrupted undo recovery')
            assert retained.exists() and large.exists() and not result.exists(),'startup replayed or deleted retained data'
            stop();print('Undo PTY: SIGKILL after relocation preserves retained data and blocks replay passed',flush=True)
        except BaseException:
            print(screen_text(output,170,40)[-2500:],file=sys.stderr);raise
        finally:
            if child and child.poll() is None:child.kill();child.wait(timeout=5)


if __name__=='__main__':main()
