#!/usr/bin/env python3
"""Exercise the embedded shell through an isolated outer Crossterm PTY."""
import fcntl,json,os,pty,select,signal,struct,subprocess,sys,tempfile,termios,time
from pathlib import Path
sys.dont_write_bytecode=True
from smoke_watch import screen_text

def main():
    binary=Path(sys.argv[1] if len(sys.argv)>1 else 'target/release/basalgrim-tui').resolve()
    with tempfile.TemporaryDirectory(prefix='basalgrim-pty-shell-') as temp:
        base=Path(temp);source=base/'files';source.mkdir()
        selected=source/"a space' имя.txt";selected.write_text('keep')
        (base/'.bashrc').write_text("PS1='BASALGRIM> '\nbind 'set enable-bracketed-paste on'\n")
        config=base/'config/molten-zharr/basalgrim/settings.json';config.parent.mkdir(parents=True)
        config.write_text(json.dumps({'grid':False,'preview':False,'theme':'light','icon_mode':'symbols'}))
        env=dict(os.environ,TERM='xterm-256color',SHELL='/bin/bash',HOME=str(base),XDG_CONFIG_HOME=str(base/'config'),XDG_DATA_HOME=str(base/'data'),XDG_STATE_HOME=str(base/'state'))
        for name in ('DISPLAY','WAYLAND_DISPLAY','WAYLAND_SOCKET','TERM_PROGRAM','LC_TERMINAL','TMUX','KITTY_WINDOW_ID','WEZTERM_EXECUTABLE','KONSOLE_VERSION'):env.pop(name,None)
        master,slave=pty.openpty();width,height=120,40
        def resize(w,h):
            nonlocal width,height
            width,height=w,h;fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',h,w,0,0))
        resize(width,height)
        process=subprocess.Popen([str(binary),'--no-session',str(source)],stdin=slave,stdout=slave,stderr=slave,env=env)
        output=bytearray()
        def pump(seconds=.08):
            end=time.monotonic()+seconds
            while time.monotonic()<end:
                if select.select([master],[],[],.01)[0]:output.extend(os.read(master,1<<20))
        def send(data):os.write(master,data);pump()
        def screen():return screen_text(output,width,height)
        def wait(check,label):
            end=time.monotonic()+8
            while not check():
                assert process.poll() is None and time.monotonic()<end,label+'\n'+screen()
                pump()
        def click(x,y):
            send(f'\x1b[<0;{x+1};{y+1}M'.encode());send(f'\x1b[<0;{x+1};{y+1}m'.encode())
        def button(text):
            rows=screen().splitlines()
            top=next((i for i,r in enumerate(rows) if "Скрыть" in r),0)
            for y,row in enumerate(rows):
                if y>=top and text in row:click(row.index(text)+1,y);return
            raise AssertionError('button missing: '+text+'\n'+screen())
        def paste(text):send(b'\x1b[200~'+text.encode()+b'\x1b[201~')
        def command(text):paste(text);send(b'\r')
        try:
            wait(lambda:"имя.txt" in screen(),'files loaded')
            send(b'\x1bOS');wait(lambda:'BASALGRIM>' in screen(),'F4 opens shell')
            command('pwd > cwd; echo $$ > shellpid; stty size > size')
            wait(lambda:(source/'size').exists(),'initial terminal size')
            assert (source/'cwd').read_text().strip()==str(source)
            shell=(source/'shellpid').read_text().strip()
            send(b'touch typed\r');wait(lambda:(source/'typed').exists(),'raw terminal typing')
            # Tab completes a command, arrows/editing act on readline rather than file selection.
            send(b'touc\t tabbed\r');wait(lambda:(source/'tabbed').exists(),'shell completion')
            paste('printf %s ');button('пути');send(b' > quoted\r')
            wait(lambda:(source/'quoted').exists(),'mouse path insertion')
            assert (source/'quoted').read_text()==str(selected)
            command('sleep 60');send(b'\x03');command('touch interrupted')
            wait(lambda:(source/'interrupted').exists(),'Ctrl+C interrupts shell job')
            command("printf '\\033[2J\\033[H\\033[48;2;17;199;111mVT_MARKER\\033[0m\\n'")
            wait(lambda:'VT_MARKER' in screen(),'VT color output')
            assert b'17;199;111' in output,'truecolor lost in outer TUI'
            # Actual alternate-screen application, raw input, cursor movement and return to primary screen.
            peer=source/'peer.py';peer.write_text("import os,tty,termios\nf=0;old=termios.tcgetattr(f);tty.setraw(f)\nos.write(1,b'\\x1b[?1049h\\x1b[2J\\x1b[HALT_ACTIVE')\nkey=os.read(0,1)\ntermios.tcsetattr(f,termios.TCSANOW,old)\nos.write(1,b'\\x1b[?1049l')\nopen('peer-key','wb').write(key)\n")
            command('python3 peer.py');wait(lambda:'ALT_ACTIVE' in screen(),'alternate screen')
            send(b'q');wait(lambda:(source/'peer-key').exists(),'raw application key')
            assert (source/'peer-key').read_bytes()==b'q'
            peer.write_text("import os,tty,termios\nf=0;old=termios.tcgetattr(f);tty.setraw(f)\nos.write(1,b'\\x1b[?1049h\\x1b[?1000h\\x1b[?1006h\\x1b[2J\\x1b[HMOUSE_ACTIVE')\nvalue=b''\nwhile not value.endswith(b'm'):value+=os.read(0,32)\ntermios.tcsetattr(f,termios.TCSANOW,old)\nos.write(1,b'\\x1b[?1000l\\x1b[?1006l\\x1b[?1049l')\nopen('peer-mouse','wb').write(value)\n")
            command('python3 peer.py');wait(lambda:'MOUSE_ACTIVE' in screen(),'application mouse mode')
            top=next(i for i,r in enumerate(screen().splitlines()) if 'Старт:' in r)
            click(3,top+2);wait(lambda:(source/'peer-mouse').exists(),'application mouse press/release')
            assert (source/'peer-mouse').read_bytes()==b'\x1b[<0;3;2M\x1b[<0;3;2m'
            command('seq 1 300');wait(lambda:'300' in screen(),'scrollback ready')
            button('↑');wait(lambda:'История:' in screen(),'mouse scroll history')
            # Wheel scrolls the embedded viewport without shell input.
            rows=screen().splitlines();y=next(i for i,r in enumerate(rows) if 'Старт:' in r)+2
            send(f'\x1b[<64;4;{y+1}M'.encode());button('Вниз')
            wait(lambda:'BASALGRIM>' in screen(),'back to live output')
            button('Скрыть');wait(lambda:'BASALGRIM>' not in screen(),'hide')
            assert Path('/proc/'+shell).exists(),'hide killed the shell'
            send(b'\x1bOS');command('echo $$ > resumed')
            wait(lambda:(source/'resumed').exists(),'resume')
            assert (source/'resumed').read_text().strip()==shell
            output.clear();resize(34,14);process.send_signal(signal.SIGWINCH)
            wait(lambda:'Скрыть' in screen(),'compact terminal')
            command('stty size > compact');wait(lambda:(source/'compact').exists(),'SIGWINCH')
            rows,cols=map(int,(source/'compact').read_text().split());assert cols==32 and rows>=4,(rows,cols)
            button('×');wait(lambda:'Завершить терминал?' in screen(),'close confirmation')
            send(b'\x1b');wait(lambda:'Скрыть' in screen(),'cancel close')
            command('exit 9');wait(lambda:'Скрыть' not in screen() and 'Старт:' not in screen(),'exit automatically closes panel')
            assert not Path('/proc/'+shell).exists(),'exited shell not reaped'
            # Navigation keys now belong to files; F4 creates a shell in the new folder.
            other=base/'another-folder';other.mkdir()
            send(b'\x0c');send(b'\x15');paste(str(other));send(b'\r')
            wait(lambda:'another-folder' in screen(),'file focus after exit')
            send(b'\x1bOS');wait(lambda:'BASALGRIM>' in screen(),'F4 creates fresh shell')
            command('pwd > fresh-cwd; echo $$ > fresh-pid')
            wait(lambda:(other/'fresh-cwd').exists() and (other/'fresh-pid').exists(),'fresh shell data')
            assert (other/'fresh-cwd').read_text().strip()==str(other)
            assert (other/'fresh-pid').read_text().strip()!=shell
            send(b'\x04');wait(lambda:'Скрыть' not in screen(),'Ctrl+D automatically closes panel')
            send(b'\x1bOS');wait(lambda:'BASALGRIM>' in screen(),'reopen after EOF')
            button('×');send(b'\r');wait(lambda:'Скрыть' not in screen(),'confirmed close')
            assert not Path('/proc/'+shell).exists(),'old shell not reaped'
            assert selected.read_text()=='keep','terminal input changed file selection data'
            send(b'\x11');process.wait(timeout=5);assert process.returncode==0
            print('Embedded terminal PTY: cwd, keys, completion, paths, interrupt, VT/color/alternate, history, hide/resume, resize, automatic exit/EOF close, fresh cwd and confirmed close passed',flush=True)
        except BaseException:
            Path('target/terminal-failure.ansi').write_bytes(output);Path('target/terminal-failure.txt').write_text(screen());raise
        finally:
            if process.poll() is None:process.terminate();process.wait(timeout=5)
            os.close(master);os.close(slave)
if __name__=='__main__':main()
