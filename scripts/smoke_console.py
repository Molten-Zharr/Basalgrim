#!/usr/bin/env python3
"""Actual TUI/PTY keyboard checks. TERM=linux is deliberately NOT a real VC/GPM test."""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
sys.dont_write_bytecode = True
from smoke_watch import screen_text
from pty_helpers import quit_when_ready


def scenario(binary, term):
    with tempfile.TemporaryDirectory(prefix='basalgrim-console-') as directory:
        root = Path(directory)
        files = root / 'files'
        files.mkdir()
        (files / 'original.txt').write_text('keep')
        destination = files / 'destination'
        destination.mkdir()
        config = root / 'config/molten-zharr/basalgrim/settings.json'
        config.parent.mkdir(parents=True)
        config.write_text(json.dumps({'grid': False, 'preview': False, 'icon_mode': 'symbols'}))
        (root / '.bashrc').write_text("PS1='CONSOLE> '\n")
        env = dict(os.environ, TERM=term, HOME=str(root), SHELL='/bin/bash',
                   XDG_CONFIG_HOME=str(root / 'config'), XDG_DATA_HOME=str(root / 'data'),
                   XDG_STATE_HOME=str(root / 'state'), GIO_USE_VFS='local')
        for key in ('DISPLAY', 'WAYLAND_DISPLAY', 'WAYLAND_SOCKET', 'TERM_PROGRAM', 'LC_TERMINAL',
                    'KITTY_WINDOW_ID', 'TMUX', 'WEZTERM_EXECUTABLE', 'KONSOLE_VERSION', 'DBUS_SESSION_BUS_ADDRESS'):
            env.pop(key, None)
        refused = subprocess.run([binary, '--no-session', str(files)], stdin=subprocess.DEVNULL,
                                 capture_output=True, env=env, timeout=8)
        assert refused.returncode != 0 and 'stdin и stdout'.encode() in refused.stderr
        assert b'\x1b' not in refused.stdout, 'nonterminal startup changed output'
        master, slave = pty.openpty()
        original = termios.tcgetattr(slave)
        width, height = 100, 32
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', height, width, 0, 0))
        child = subprocess.Popen([binary, '--no-session', str(files)], stdin=slave, stdout=slave,
                                 stderr=slave, env=env)
        output = bytearray()

        def pump(seconds=.12):
            end = time.monotonic() + seconds
            while time.monotonic() < end:
                if select.select([master], [], [], .01)[0]:
                    output.extend(os.read(master, 1 << 20))

        def send(data):
            os.write(master, data.encode() if isinstance(data, str) else data)
            pump()

        def screen():
            return screen_text(output, width, height)

        def wait(predicate, label):
            end = time.monotonic() + 8
            while not predicate():
                assert child.poll() is None and time.monotonic() < end, label + '\n' + screen()
                pump(.05)

        try:
            wait(lambda: 'original.txt' in screen(), 'first frame')
            assert 'GPM' not in screen(), 'TERM alone incorrectly selected the GPM backend'
            # Linux-console F1/F2/F4 sequences, also accepted by Crossterm in a PTY.
            send(b'\x1b[[A')
            wait(lambda: 'Управление · basalgrim' in screen(), 'Linux F1')
            send(b'\x1b')
            send(b'\x0e')
            send('Тест_ЁABC.txt')
            send(b'\r')
            wait(lambda: (files / 'Тест_ЁABC.txt').is_file(), 'ordinary Unicode text')
            wait(lambda: 'Стоп' not in screen().splitlines()[-1], 'create completion')
            wait(lambda: 'Тест_ЁABC.txt' in screen(), 'updated file listing')
            send(b'\x1b[F')  # End selects the newly created (last sorted) file.
            send(b'\x1b[[B')
            send(b'\x01')
            send('Имя_ABC.txt')
            send(b'\r')
            wait(lambda: (files / 'Имя_ABC.txt').is_file(), 'Linux F2 rename')
            wait(lambda: 'Стоп' not in screen().splitlines()[-1], 'rename completion')
            # Enhanced-terminal Unicode Ctrl key; plain console Ctrl bytes below.
            if term != 'linux':
                send(f'\x1b[{ord("с")};5u')  # Ctrl+C on Russian layout
            else:
                send(b'\x03')
            send(b'\x0c\x15')
            send(str(destination))
            send(b'\r')
            wait(lambda: 'destination' in screen(), 'path navigation')
            send(b'\x16')
            wait(lambda: (destination / 'Имя_ABC.txt').is_file(), 'copy/paste key input')
            wait(lambda: 'Стоп' not in screen().splitlines()[-1], 'copy completion')
            # A raw program in the embedded shell observes exact Alt+uppercase
            # and UTF-8 bytes. It cannot be mistaken for file-manager shortcuts.
            peer = destination / 'keys.py'
            peer.write_text("import os,tty,termios\nold=termios.tcgetattr(0);tty.setraw(0)\nos.write(1,b'RAW_KEYS_READY')\ndata=b''\nwhile len(data)<5:data+=os.read(0,5-len(data))\ntermios.tcsetattr(0,termios.TCSANOW,old)\nopen('keys.bin','wb').write(data)\n")
            send(b'\x1b[[D')
            wait(lambda: 'CONSOLE>' in screen(), 'Linux F4 terminal')
            send('python3 keys.py\r')
            wait(lambda: 'RAW_KEYS_READY' in screen(), 'raw key peer')
            send(b'\x1bA')
            send('ЁZ')
            wait(lambda: (destination / 'keys.bin').exists(), 'shell bytes')
            assert (destination / 'keys.bin').read_bytes() == b'\x1bA' + 'ЁZ'.encode()
            wait(lambda: 'CONSOLE>' in screen(), 'shell ready')
            send('exit\r')
            wait(lambda: 'Скрыть' not in screen(), 'automatic shell exit')
            width, height = 34, 14
            output.clear()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', height, width, 0, 0))
            child.send_signal(signal.SIGWINCH)
            pump(.3)
            send(b'\x0e')
            send('ещё.txt')
            send(b'\r')
            wait(lambda: (destination / 'ещё.txt').exists(), 'compact Unicode create')
            quit_when_ready(child, send)
            assert termios.tcgetattr(slave) == original, 'terminal flags not restored'
            print(f'PTY {term}: tty validation, console function keys, Unicode/clipboard/Alt case, shell exit, 34x14 and termios passed', flush=True)
        except BaseException:
            Path('target/console-failure.txt').write_text(screen())
            raise
        finally:
            if child.poll() is None:
                child.terminate()
                child.wait(timeout=5)
            os.close(master)
            os.close(slave)


if __name__ == '__main__':
    binary = str(Path(sys.argv[1] if len(sys.argv) > 1 else 'target/release/basalgrim-tui').resolve())
    for term in ('linux', 'xterm-256color'):
        scenario(binary, term)
