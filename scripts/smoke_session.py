#!/usr/bin/env python3
"""Session restoration through real PTY launches, mouse settings and independent state storage."""
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
sys.dont_write_bytecode = True
from smoke_watch import screen_text


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/basalgrim-tui').resolve()
    with tempfile.TemporaryDirectory(prefix='basalgrim-session-') as tmp:
        base = Path(tmp); dirs = [base / f'tab-{i}' for i in range(4)]
        for directory in dirs:
            directory.mkdir(); (directory / 'marker.txt').write_text(directory.name)
        config = base / 'config/molten-zharr/basalgrim/settings.json'
        config.parent.mkdir(parents=True); config.write_text(json.dumps({'dual':True,'preview':False,'grid':False}))
        state = base / 'state/molten-zharr/basalgrim/session.json'
        env = dict(os.environ, TERM='xterm-256color', XDG_CONFIG_HOME=str(base/'config'), XDG_STATE_HOME=str(base/'state'))
        for name in ('TERM_PROGRAM','LC_TERMINAL','TMUX','WEZTERM_EXECUTABLE','KONSOLE_VERSION'):
            env.pop(name, None)
        child = None; master = slave = None; output = bytearray()

        def pump(seconds=.15):
            end = time.monotonic() + seconds
            while time.monotonic() < end:
                if select.select([master], [], [], .01)[0]: output.extend(os.read(master, 1 << 20))

        def send(value):
            os.write(master, value.encode() if isinstance(value, str) else value); pump()

        def text(): return screen_text(output, 170, 40)

        def wait_for(predicate, label):
            end = time.monotonic() + 8
            while not predicate():
                assert child.poll() is None and time.monotonic() < end, label
                pump(.02)

        def launch(*args):
            nonlocal child, master, slave, output
            master, slave = pty.openpty(); output = bytearray()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH',40,170,0,0))
            child = subprocess.Popen([str(binary), *map(str,args)],cwd=base,env=env,stdin=slave,stdout=slave,stderr=slave)
            wait_for(lambda:'marker.txt' in text(),'initial files')

        def stop():
            send(b'\x11'); child.wait(timeout=5); assert child.returncode == 0
            os.close(master); os.close(slave)

        def navigate(path): send(b'\x0c\x15'); send(str(path)); send(b'\r')

        def click(x,y): send(f'\x1b[<0;{x+1};{y+1}M'); send(f'\x1b[<0;{x+1};{y+1}m')

        try:
            launch(dirs[0]); send(b'\x14'); navigate(dirs[1]); send(b'\t'); navigate(dirs[2]); send(b'\x14'); navigate(dirs[3])
            stop()
            first = json.loads(state.read_text())
            assert first['active']==1 and [p['active'] for p in first['panes']]==[1,1]
            assert [p['tabs'] for p in first['panes']]==[[str(dirs[0]),str(dirs[1])],[str(dirs[2]),str(dirs[3])]]
            launch()
            assert str(dirs[1]) in text() and str(dirs[3]) in text()
            send(b'\x03'); navigate(dirs[0]); send(b'\x16')
            wait_for(lambda:'Совпадение имён' in text(),'restored active pane and focused entry')
            send(b'\r')
            wait_for(lambda:(dirs[0]/'marker (1).txt').exists(),'copy from restored pane')
            assert (dirs[0]/'marker (1).txt').read_text()=='tab-3'
            navigate(dirs[3]); send(b'\x17') # Close restored right tab.
            stop()
            second = json.loads(state.read_text())
            assert second['panes'][1]['tabs']==[str(dirs[2])]
            before = state.read_bytes(); launch('--no-session',dirs[0]); send(b'\x14'); stop()
            assert state.read_bytes()==before,'--no-session overwrote saved state'
            launch(dirs[0]) # An explicit path starts fresh, even when a session exists.
            assert '2:tab-' not in text()
            wait_for(lambda:len(json.loads(state.read_text())['panes'][1]['tabs'])==1 and json.loads(state.read_text())['active']==0,'explicit path saved fresh tabs')
            pump(.4); before=state.read_bytes()
            send(b'\x1b[21~') # F10 settings.
            click(4,7) # Third section: Session.
            wait_for(lambda:'Восстанавливать сессию' in text(),'mouse Session section')
            click(24,3) # First parameter row.
            wait_for(lambda:json.loads(config.read_text()).get('restore_session') is False,'disable restore with mouse')
            send(b'\x1b'); stop()
            assert state.read_bytes()==before,'disabled autosave replaced stored state'
            # Preserve a malformed file, retain normal navigation and leave it unchanged on exit.
            preferences=json.loads(config.read_text());preferences['restore_session']=True;config.write_text(json.dumps(preferences))
            state.write_text('{broken'); launch(dirs[0]); send(b'\x14'); stop()
            assert state.read_text()=='{broken'
            print('Session PTY passed: both panes/tabs, active pane, restart, tab close, explicit path, --no-session, mouse toggle and corrupt-state preservation')
        except BaseException:
            if child and child.poll() is None: print(text(), file=sys.stderr)
            raise
        finally:
            if child and child.poll() is None:
                child.terminate();child.wait(timeout=5)
                os.close(master);os.close(slave)


if __name__ == '__main__': main()
