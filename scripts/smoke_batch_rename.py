#!/usr/bin/env python3
"""Bulk rename in an isolated terminal, including minimum-size mouse controls."""
import fcntl,json,os,pty,select,struct,subprocess,sys,tempfile,termios,time
from pathlib import Path

sys.dont_write_bytecode=True
from pty_helpers import quit_when_ready
import batch_rename_fixture


def main():
    binary=Path(sys.argv[1] if len(sys.argv)>1 else 'target/debug/basalgrim-tui').resolve()
    with tempfile.TemporaryDirectory(prefix='basalgrim-batch-') as tmp:
        base=Path(tmp);source=batch_rename_fixture.prepare(base)
        settings=base/'config/molten-zharr/basalgrim/settings.json';settings.parent.mkdir(parents=True)
        settings.write_text(json.dumps({'preview':False,'grid':False}))
        env=dict(os.environ,TERM='xterm-256color',HOME=str(base),XDG_CONFIG_HOME=str(base/'config'),XDG_DATA_HOME=str(base/'data'),XDG_STATE_HOME=str(base/'state'))
        for name in ('DISPLAY','WAYLAND_DISPLAY','WAYLAND_SOCKET','TERM_PROGRAM','LC_TERMINAL','TMUX','WEZTERM_EXECUTABLE','KONSOLE_VERSION'):env.pop(name,None)
        master,slave=pty.openpty()
        def resize(w,h):fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',h,w,0,0))
        resize(170,53)
        child=subprocess.Popen([str(binary),'--no-session',str(source)],env=env,stdin=slave,stdout=slave,stderr=slave)
        output=bytearray()
        def pump(seconds=.1):
            end=time.monotonic()+seconds
            while time.monotonic()<end:
                if select.select([master],[],[],.01)[0]:output.extend(os.read(master,1<<20))
        def send(value):os.write(master,value.encode() if isinstance(value,str) else value);pump()
        def key(name,*mods):
            if mods==('ctrl',):send(chr(ord(name)-96))
            else:send({'F2':'\x1bOQ','Return':'\r','Escape':'\x1b'}[name])
        def click(x,y):send(f'\x1b[<0;{x+1};{y+1}M');send(f'\x1b[<0;{x+1};{y+1}m')
        def wait_for(predicate,label):
            end=time.monotonic()+8
            while not predicate():
                assert child.poll() is None and time.monotonic()<end,label;pump(.02)
        try:
            wait_for(lambda:b'file-000.jpg' in output,'initial directory')
            batch_rename_fixture.exercise(source,key,lambda value:send('\x1b[200~'+value+'\x1b[201~'),click,lambda name:None,wait_for,resize)
            quit_when_ready(child,send)
        finally:
            if child.poll() is None:child.terminate();child.wait(timeout=5)
            os.close(master);os.close(slave)


if __name__=='__main__':main()
