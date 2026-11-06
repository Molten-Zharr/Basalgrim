#!/usr/bin/env python3
"""Real Shift key and SGR mouse input, without access to a desktop clipboard."""
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
from pty_helpers import quit_when_ready
import range_selection_fixture


def main():
    binary = Path(sys.argv[1] if len(sys.argv)>1 else 'target/debug/basalgrim-tui').resolve()
    with tempfile.TemporaryDirectory(prefix='basalgrim-range-') as tmp:
        base = Path(tmp)
        source = range_selection_fixture.prepare(base)
        config = base / 'config/molten-zharr/basalgrim/settings.json'
        config.parent.mkdir(parents=True)
        config.write_text(json.dumps({'grid':False, 'preview':False}))
        env = dict(os.environ, TERM='xterm-256color', HOME=str(base),
                   XDG_CONFIG_HOME=str(base/'config'), XDG_DATA_HOME=str(base/'data'), XDG_STATE_HOME=str(base/'state'))
        for name in ('DISPLAY','WAYLAND_DISPLAY','WAYLAND_SOCKET','TERM_PROGRAM','LC_TERMINAL','TMUX','WEZTERM_EXECUTABLE','KONSOLE_VERSION'):
            env.pop(name, None)
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 53, 170, 0, 0))
        child = subprocess.Popen([str(binary), '--no-session', str(source)], env=env, stdin=slave, stdout=slave, stderr=slave)
        output = bytearray()

        def pump(seconds=.10):
            end = time.monotonic()+seconds
            while time.monotonic()<end:
                if select.select([master], [], [], .01)[0]:
                    output.extend(os.read(master, 1<<20))

        def send(value):
            os.write(master, value.encode());pump()

        def wait_for(predicate, label):
            end = time.monotonic()+6
            while not predicate():
                assert child.poll() is None and time.monotonic()<end, label
                pump(.02)

        def key(name, *mods):
            if name in ('Home','End','Up','Down'):
                final={'Home':'H','End':'F','Up':'A','Down':'B'}[name]
                modifier=1 + (1 if 'shift' in mods else 0) + (4 if 'ctrl' in mods else 0)
                send(f'\x1b[1;{modifier}{final}')
            elif name == 'Return':
                send('\r')
            else:
                assert mods == ('ctrl',), (name, mods)
                send(chr(ord(name)-96))

        def navigate(path):
            key('l','ctrl');key('u','ctrl');send(str(path));key('Return');pump(.15)

        def click(x, y, *mods):
            code = (4 if 'shift' in mods else 0) + (16 if 'ctrl' in mods else 0)
            send(f'\x1b[<{code};{x+1};{y+1}M')
            send(f'\x1b[<0;{x+1};{y+1}m')  # Release the modifiers before the button.

        try:
            wait_for(lambda:b'file-000.txt' in output, 'initial directory')
            range_selection_fixture.exercise(base, source, key, navigate, click,
                lambda x,y:send(f'\x1b[<65;{x+1};{y+1}M'), lambda name:None, wait_for)
            quit_when_ready(child, lambda value:send(value.decode() if isinstance(value,bytes) else value))
        finally:
            if child.poll() is None:
                child.terminate();child.wait(timeout=5)
            os.close(master);os.close(slave)


if __name__ == '__main__':
    main()
