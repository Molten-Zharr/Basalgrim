#!/usr/bin/env python3
"""Verify code text AND its actual SGR colors in a desktop-free PTY."""
import fcntl, json, os, pty, re, select, signal, struct, subprocess, sys, tempfile, termios, time, unicodedata
from pathlib import Path
sys.dont_write_bytecode = True
from syntax_fixture import prepare, STRING, KEYWORD
from pty_helpers import quit_when_ready


def screen(output, width, height):
    cells = [[(' ', None)] * width for _ in range(height)]
    row = col = 0
    fg = None
    decoded = re.sub(r'\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)', '', output.decode(errors='replace'))
    for token in re.findall(r'\x1b\[[0-?]*[ -/]*[@-~]|\x1b[^\[]|[^\x1b]+', decoded):
        if token.startswith('\x1b['):
            command, params = token[-1], token[2:-1]
            if command in 'Hf':
                values = params.split(';')
                row = min(height-1, max(0, int(values[0] or 1)-1))
                col = min(width-1, max(0, int(values[1] or 1)-1)) if len(values)>1 else 0
            elif command == 'J' and params in ('2','3'):
                cells = [[(' ',None)] * width for _ in range(height)]
            elif command == 'K':
                for x in range(col if params in ('','0') else 0, width): cells[row][x] = (' ',fg)
            elif command == 'm':
                values = [int(v or 0) for v in params.split(';')]
                i = 0
                while i < len(values):
                    value = values[i]
                    if value in (0,39): fg = None
                    elif value in (38,48) and values[i+1:i+2] == [2]:
                        if value == 38: fg = tuple(values[i+2:i+5])
                        i += 4
                    elif value in (38,48) and values[i+1:i+2] == [5]: i += 2
                    i += 1
            continue
        if token.startswith('\x1b'): continue
        for char in token:
            if char == '\r': col = 0
            elif char == '\n': row = min(height-1, row+1)
            elif not unicodedata.category(char).startswith('C'):
                if col < width: cells[row][col] = (char,fg)
                col += 2 if unicodedata.east_asian_width(char) in 'WF' else 1
    return cells


def main():
    binary = Path(sys.argv[1] if len(sys.argv)>1 else 'target/release/basalgrim-tui').resolve()
    with tempfile.TemporaryDirectory(prefix='basalgrim-syntax-') as tmp:
        base = Path(tmp); source = prepare(base)
        settings = base/'config/molten-zharr/basalgrim/settings.json'; settings.parent.mkdir(parents=True)
        settings.write_text(json.dumps({'grid':False,'preview':True,'theme':'light','icon_mode':'symbols'}))
        env = dict(os.environ, TERM='xterm-256color', HOME=str(base), XDG_CONFIG_HOME=str(base/'config'), XDG_DATA_HOME=str(base/'data'), XDG_STATE_HOME=str(base/'state'))
        for name in ('DISPLAY','WAYLAND_DISPLAY','WAYLAND_SOCKET','TERM_PROGRAM','LC_TERMINAL','TMUX','WEZTERM_EXECUTABLE','KONSOLE_VERSION','NO_COLOR'): env.pop(name,None)
        master,slave = pty.openpty()
        width,height = 170,53
        def resize(w,h):
            nonlocal width,height
            width,height = w,h
            fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',h,w,0,0))
        resize(width,height)
        child = subprocess.Popen([str(binary),'--no-session',str(source)],env=env,stdin=slave,stdout=slave,stderr=slave)
        output = bytearray()
        def pump(seconds=.1):
            end=time.monotonic()+seconds
            while time.monotonic()<end:
                if select.select([master],[],[],.01)[0]:output.extend(os.read(master,1<<20))
        def wait(predicate,label):
            end=time.monotonic()+8
            while not predicate():
                assert child.poll() is None and time.monotonic()<end,label
                pump()
        def send(data):os.write(master,data);pump()
        def click(x,y):
            send(f'\x1b[<0;{x+1};{y+1}M'.encode());send(f'\x1b[<0;{x+1};{y+1}m'.encode())
        def colored(color, letter):
            return any(c == (letter,color) for row in screen(output,width,height)[13:-4] for c in row[134:])
        try:
            wait(lambda:colored(STRING,'H') and colored(KEYWORD,'f'),'initial code SGR colors')
            send(b'\x1b[<65;145;25M')
            wait(lambda:colored(STRING,'H'),'syntax colors after preview wheel')
            click(35,12)
            wait(lambda:not colored(STRING,'H') and not colored(KEYWORD,'f'),'plain text clears syntax colors')
            click(35,11)
            wait(lambda:colored(STRING,'H'),'mouse returns to code')
            (source/'a.rs').write_text('// updated comments only\n'*35)
            wait(lambda:not colored(STRING,'H') and not colored(KEYWORD,'f'),'watched code rewrite clears old colors')
            (source/'a.rs').write_text('fn main() {\n    let greeting = "Hello updated";\n}\n'*35)
            wait(lambda:colored(STRING,'H'),'watched code rewrite gains colors')
            output.clear();resize(34,14);child.send_signal(signal.SIGWINCH);pump(.3)
            assert not colored(STRING,'H'),'preview should hide at 34x14'
            output.clear();resize(170,53);child.send_signal(signal.SIGWINCH)
            wait(lambda:colored(STRING,'H'),'colors return after resizing')
            send(b'\x1bOR') # F3
            wait(lambda:not colored(STRING,'H'),'F3 hides code')
            quit_when_ready(child,send)
            print('Syntax PTY: actual SGR colors, mouse selection/wheel, watched rewrites, 34x14 resize and F3 passed',flush=True)
        except BaseException:
            Path('target/syntax-failure.ansi').write_bytes(output)
            raise
        finally:
            if child.poll() is None:child.terminate();child.wait(timeout=5)
            os.close(master);os.close(slave)

if __name__ == '__main__': main()
