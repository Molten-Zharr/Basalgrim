#!/usr/bin/env python3
"""Exercise the folder editor and restart a real 34×14 terminal."""
import fcntl
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
from folder_style_fixture import Fixture, exercise, reset, wait
from pty_helpers import quit_when_ready
from smoke_media import frames
from smoke_watch import screen_text


def session(binary, fixture, env, restarted=False, protocol=None):
    master, slave = pty.openpty()
    width, height = 34, 14
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', height, width, width*8, height*16))
    if protocol == 'iterm2':
        env = dict(env, TERM_PROGRAM='iTerm.app')
    process = subprocess.Popen([binary, '--no-session', '--trash-home-only', fixture.root],
                               env=env, stdin=slave, stdout=slave, stderr=slave)
    output = bytearray()
    replied = False

    def pump(seconds=.12):
        nonlocal replied
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            if select.select([master], [], [], .01)[0]:
                output.extend(os.read(master, 65536))
            if protocol and not replied and b'\x1b[5n' in output:
                reply = b'\x1b_Gi=31;OK\x1b\\' if protocol == 'kitty' else b''
                reply += b'\x1b[?62;4c' if protocol == 'sixel' else b'\x1b[?62c'
                os.write(master, reply + b'\x1b[6;16;8t\x1b[0n')
                replied = True

    def send(data):
        os.write(master, data)
        pump()

    def key(name, modifier=None):
        send({('Return', 'alt'): b'\x1b[13;3u', ('Return', None): b'\r',
              ('F2', None): b'\x1bOQ'}[(name, modifier)])

    def text(value):
        send(f'\x1b[200~{value}\x1b[201~'.encode())

    def click(x, y):
        for event in 'Mm':
            send(f'\x1b[<0;{x+1};{y+1}{event}'.encode())

    def shot(_):
        pump()
        return screen_text(output, width, height)

    def icon_pixels():
        pump()
        return any(all(abs(actual - expected) < 5 for actual, expected in zip(rgb, (21, 223, 103)))
                   for _, _, colors in frames(output, protocol) for rgb in colors)

    try:
        if protocol:
            wait(icon_pixels, f'{protocol} saved icon pixels')
            print(f'{protocol} folder icon verified at 34×14')
        else:
            wait(lambda: 'Проекты' in shot('start'), 'folder listing')
            if restarted:
                reset(fixture, key, click, shot, (width, height))
            else:
                exercise(fixture, key, text, click, shot, (width, height))
        quit_when_ready(process, send)
    except BaseException:
        print(shot('failure'), file=sys.stderr)
        raise
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=5)
        os.close(master)
        os.close(slave)


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/release/basalgrim-tui').resolve()
    with tempfile.TemporaryDirectory(prefix='basalgrim-style-pty-') as tmp:
        fixture = Fixture(Path(tmp), view='list')
        env = dict(os.environ, **fixture.env, TERM='xterm-256color')
        for name in ('DISPLAY', 'WAYLAND_DISPLAY', 'WAYLAND_SOCKET', 'TMUX', 'TERM_PROGRAM',
                     'LC_TERMINAL', 'WEZTERM_EXECUTABLE', 'KONSOLE_VERSION'):
            env.pop(name, None)
        try:
            session(binary, fixture, env)
            for protocol in ('kitty', 'sixel', 'iterm2'):
                session(binary, fixture, env, protocol=protocol)
            session(binary, fixture, env, restarted=True)
        finally:
            fixture.close()


if __name__ == '__main__':
    main()
