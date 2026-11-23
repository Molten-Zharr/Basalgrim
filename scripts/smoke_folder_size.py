#!/usr/bin/env python3
"""Folder measurement through real keyboard/mouse input in a private PTY."""
import fcntl,json,os,pty,select,signal,struct,subprocess,sys,tempfile,termios,time
from pathlib import Path
sys.dont_write_bytecode=True
from folder_size_fixture import prepare
from smoke_watch import screen_text
from pty_helpers import quit_when_ready

def main():
    binary=Path(sys.argv[1] if len(sys.argv)>1 else 'target/release/basalgrim-tui').resolve()
    with tempfile.TemporaryDirectory(prefix='basalgrim-folder-') as tmp:
        base=Path(tmp);source=prepare(base)
        settings=base/'config/molten-zharr/basalgrim/settings.json';settings.parent.mkdir(parents=True)
        settings.write_text(json.dumps({'grid':False,'preview':True,'theme':'light','icon_mode':'symbols'}))
        env=dict(os.environ,TERM='xterm-256color',HOME=str(base),XDG_CONFIG_HOME=str(base/'config'),XDG_DATA_HOME=str(base/'data'),XDG_STATE_HOME=str(base/'state'))
        for key in ('DISPLAY','WAYLAND_DISPLAY','WAYLAND_SOCKET','TERM_PROGRAM','LC_TERMINAL','TMUX','WEZTERM_EXECUTABLE','KONSOLE_VERSION'):env.pop(key,None)
        master,slave=pty.openpty();width,height=170,53
        def resize(w,h):
            nonlocal width,height
            width,height=w,h;fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',h,w,0,0))
        resize(width,height)
        child=subprocess.Popen([str(binary),'--no-session',str(source)],env=env,stdin=slave,stdout=slave,stderr=slave)
        output=bytearray()
        def pump(seconds=.1):
            end=time.monotonic()+seconds
            while time.monotonic()<end:
                if select.select([master],[],[],.01)[0]:output.extend(os.read(master,1<<20))
        def send(data):os.write(master,data);pump()
        def screen():return screen_text(output,width,height)
        def wait(predicate,label):
            end=time.monotonic()+8
            while not predicate():
                assert child.poll() is None and time.monotonic()<end,label
                pump()
        def click(x,y):send(f'\x1b[<0;{x+1};{y+1}M'.encode());send(f'\x1b[<0;{x+1};{y+1}m'.encode())
        try:
            wait(lambda:'a-folder' in screen(),'initial folder')
            if '--auto-folder-sizes-only' in sys.argv[2:]:
                assert '≈12.0 KiB' not in screen()
                send(b'\x1b[21~');click(5,11);click(155,9)
                wait(lambda:json.loads(settings.read_text()).get('auto_folder_sizes') is True,'saved setting')
                send(b'\x1b')
                wait(lambda:'a-folder ≈12.0 KiB' in screen(),'automatic size in file pane')
                output.clear();resize(34,14);child.send_signal(signal.SIGWINCH)
                wait(lambda:'a-folder ≈12.0 KiB' in screen(),'compact folder size')
                (source/'a-folder/plain').write_bytes(bytes(10240))
                wait(lambda:'a-folder ≈16.0 KiB' in screen(),'external write to visible folder')
                quit_when_ready(child,send)
                output.clear()
                child=subprocess.Popen([str(binary),'--no-session',str(source)],env=env,stdin=slave,stdout=slave,stderr=slave)
                wait(lambda:'a-folder ≈16.0 KiB' in screen(),'enabled after restart at 34x14')
                send(b'\x1b[21~');send(b'\x1b[F');send(b'\x1b[A');send(b'\x1b[A')
                send(b'\r');send(b'\x1b[F')
                wait(lambda:'Размеры папок' in screen(),'compact settings row')
                send(b'\r')
                wait(lambda:json.loads(settings.read_text()).get('auto_folder_sizes') is False,'disabled setting')
                send(b'\x1b')
                wait(lambda:'a-folder' in screen() and '≈16.0 KiB' not in screen(),'disabled compact label')
                quit_when_ready(child,send)
                print('Automatic folder sizes PTY: setting, visible rows, 34x14, external write, restart and disabling passed',flush=True)
                return
            send(b'\x1b[<2;36;11M')
            def size_command():
                for y,line in enumerate(screen().splitlines()):
                    x=line.find('Размер папки',35,110)
                    if x>=0:return x,y
            wait(size_command,'folder size context command')
            click(*size_command())
            wait(lambda:'Размер файлов: 12.0 KiB' in screen() and 'Подсчёт завершён' in screen(),'exact full result')
            assert 'Файлов: 3 · папок: 1' in screen()
            click(60,34);send(b'\x1b[1;2S') # Shift+F4 (F3's legacy encoding conflicts with cursor reports).
            wait(lambda:'Кэш · предыдущий подсчёт' in screen(),'cached result')
            (source/'a-folder/plain').write_bytes(bytes(10240))
            click(60,33)
            wait(lambda:'Размер файлов: 16.0 KiB' in screen() and 'Подсчёт завершён' in screen(),'forced recount after write')
            output.clear();resize(34,14);child.send_signal(signal.SIGWINCH)
            wait(lambda:'Размер файлов: 16.0 KiB' in screen(),'34x14 result')
            # Keep the worker busy long enough for actual mouse Stop at minimum size.
            large=source/'a-folder/large';large.mkdir()
            for group in range(250):
                folder=large/str(group);folder.mkdir();seed=folder/'0';seed.write_bytes(b'x')
                for i in range(1,1000):os.link(seed,folder/str(i))
            click(8,11);click(10,10)
            wait(lambda:'Остановлено · частично' in screen(),'minimum-size mouse cancellation')
            assert 'Пропуски:' in screen()
            click(8,12);wait(lambda:'Размер папки' not in screen(),'close without click-through')
            quit_when_ready(child,send)
            print('Folder size PTY: exact totals, cache, mouse recount/stop/close, Shift+F4 and 34x14 passed',flush=True)
        except BaseException:
            Path('target/folder-failure.ansi').write_bytes(output);Path('target/folder-failure.txt').write_text(screen());raise
        finally:
            if child.poll() is None:child.terminate();child.wait(timeout=5)
            os.close(master);os.close(slave)
if __name__=='__main__':main()
