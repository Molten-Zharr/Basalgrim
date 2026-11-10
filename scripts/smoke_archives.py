#!/usr/bin/env python3
"""Archive workflows in an isolated real PTY, including minimum-size mouse controls."""
import fcntl,json,os,pty,select,signal,struct,subprocess,sys,tempfile,termios,time,tarfile,zipfile
from pathlib import Path
sys.dont_write_bytecode=True
from archive_fixture import prepare
from smoke_watch import screen_text
from pty_helpers import quit_when_ready

def main():
    binary=Path(sys.argv[1] if len(sys.argv)>1 else 'target/release/basalgrim-tui').resolve()
    with tempfile.TemporaryDirectory(prefix='basalgrim-archives-') as tmp:
        base=Path(tmp);root=prepare(base)
        settings=base/'config/molten-zharr/basalgrim/settings.json';settings.parent.mkdir(parents=True)
        settings.write_text(json.dumps({'grid':False,'preview':True,'theme':'molten-zharr','icon_mode':'symbols'}))
        env=dict(os.environ,TERM='xterm-256color',HOME=str(base),XDG_CONFIG_HOME=str(base/'config'),XDG_DATA_HOME=str(base/'data'),XDG_STATE_HOME=str(base/'state'))
        for k in ('DISPLAY','WAYLAND_DISPLAY','WAYLAND_SOCKET','TERM_PROGRAM','LC_TERMINAL','TMUX','WEZTERM_EXECUTABLE','KONSOLE_VERSION'):env.pop(k,None)
        master,slave=pty.openpty();width,height=170,53
        def resize(w,h):
            nonlocal width,height
            width,height=w,h;fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',h,w,0,0))
        resize(width,height)
        child=subprocess.Popen([str(binary),'--no-session',str(root)],env=env,stdin=slave,stdout=slave,stderr=slave)
        output=bytearray()
        def pump(seconds=.1):
            end=time.monotonic()+seconds
            while time.monotonic()<end:
                if select.select([master],[],[],.01)[0]:output.extend(os.read(master,1<<20))
        def send(data):os.write(master,data);pump()
        def screen():return screen_text(output,width,height)
        def wait(predicate,label):
            end=time.monotonic()+10
            while not predicate():
                assert child.poll() is None and time.monotonic()<end,label+'\n'+screen()
                pump()
        def click(x,y,button=0):send(f'\x1b[<{button};{x+1};{y+1}M'.encode());send(f'\x1b[<{button};{x+1};{y+1}m'.encode())
        def navigate(path):send(b'\x0c');send(b'\x15');send(str(path).encode());send(b'\r')
        try:
            wait(lambda:'a-folder' in screen(),'initial files')
            send(b'\x1b\x01');wait(lambda:'Создать архив' in screen(),'create shortcut')
            output.clear();resize(34,14);child.send_signal(signal.SIGWINCH);pump(.3)
            wait(lambda:'TAR.GZ' in screen(),'compact format buttons')
            click(19,8);click(7,10)
            wait(lambda:(root/'a-folder.tar.gz').is_file(),'compact create button')
            with tarfile.open(root/'a-folder.tar.gz') as a:assert a.extractfile('a-folder/nested/hello.txt').read()==b'Archive content: hello\n'
            output.clear();resize(170,53);child.send_signal(signal.SIGWINCH);pump(.3)
            navigate(root/'z-sample.zip')
            wait(lambda:'Архив: только чтение' in screen(),'virtual ZIP root')
            send(b'\r');wait(lambda:'second.txt' in screen(),'first archive folder')
            send(b'\r');wait(lambda:'hello.txt' in screen(),'nested archive folder')
            wait(lambda:'Archive content: hello' in screen(),'preview of compressed member')
            click(40,11,2);wait(lambda:'Распаковать' in screen(),'member context menu')
            click(43,13);wait(lambda:'Распаковать в папку' in screen(),'selected extraction dialog')
            dest=root/'selection';send(b'\x15');send(str(dest).encode())
            output.clear();resize(34,14);child.send_signal(signal.SIGWINCH);pump(.3);click(7,10)
            wait(lambda:(dest/'hello.txt').is_file(),'compact extraction confirm')
            assert sorted(p.name for p in dest.iterdir())==['hello.txt']
            pump(.3);send(b'\x1a');wait(lambda:not dest.exists(),'undo extraction')
            send(b'\x19');wait(lambda:(dest/'hello.txt').is_file(),'redo extraction')
            quit_when_ready(child,send)
            print('Archive PTY: independent ZIP/TAR interoperability, native shortcuts, 34x14 format/confirm, navigation, preview, selected context extraction and undo/redo passed',flush=True)
        except BaseException:
            Path('target/archive-failure.ansi').write_bytes(output);Path('target/archive-failure.txt').write_text(screen());raise
        finally:
            if child.poll() is None:child.terminate();child.wait(timeout=5)
            os.close(master);os.close(slave)
if __name__=='__main__':main()
