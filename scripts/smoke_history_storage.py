#!/usr/bin/env python3
"""Private PTY storage controls, restart and SIGKILL during permanent removal."""
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
import termios
import time
sys.dont_write_bytecode = True
from smoke_watch import screen_text


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/basalgrim-tui').resolve()
    with tempfile.TemporaryDirectory(prefix='basalgrim-history-storage-') as tmp:
        base = Path(tmp); files = base/'files'; files.mkdir()
        original = files/'a.txt'; original.write_text('keep original')
        config = base/'config/molten-zharr/basalgrim/settings.json'; config.parent.mkdir(parents=True)
        config.write_text(json.dumps({'grid': False, 'preview': False, 'icon_mode': 'symbols'}))
        db = base/'state/molten-zharr/basalgrim/operations.sqlite3'
        env = dict(os.environ, HOME=str(base), TERM='xterm-256color', XDG_CONFIG_HOME=str(base/'config'), XDG_STATE_HOME=str(base/'state'), XDG_DATA_HOME=str(base/'data'))
        for name in ('DISPLAY', 'WAYLAND_DISPLAY', 'WAYLAND_SOCKET', 'TERM_PROGRAM', 'LC_TERMINAL', 'TMUX', 'WEZTERM_EXECUTABLE', 'KONSOLE_VERSION'): env.pop(name, None)
        child = None; master = slave = None; output = bytearray(); width = 100; height = 28

        def pump(seconds=.08):
            end = time.monotonic()+seconds
            while time.monotonic() < end:
                if select.select([master], [], [], min(.01, max(0, end-time.monotonic())))[0]:
                    try: output.extend(os.read(master, 1 << 20))
                    except OSError:
                        assert child.poll() is not None
                        return

        def send(data):
            os.write(master, data.encode() if isinstance(data, str) else data); pump()

        def text(): return screen_text(output, width, height)

        def rows(sql, args=()):
            if not db.exists(): return []
            try:
                with sqlite3.connect(db.as_uri()+'?mode=ro', uri=True, timeout=.2) as connection: return connection.execute(sql, args).fetchall()
            except sqlite3.Error: return []

        def wait(predicate, label, seconds=30):
            end = time.monotonic()+seconds
            while not predicate():
                assert child.poll() is None and time.monotonic() < end, label
                pump(.01)

        def idle():
            wait(lambda: rows("SELECT COUNT(*) FROM operations WHERE state='running'") == [(0,)] and not any(marker in text() for marker in ('Стоп', 'Отмена…')), 'worker completion')

        def launch(path=files):
            nonlocal child, master, slave, output, width, height
            master, slave = pty.openpty(); output = bytearray(); width = 100; height = 28
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', height, width, 0, 0))
            child = subprocess.Popen([str(binary), '--no-session', '--trash-home-only', str(path)], env=env, stdin=slave, stdout=slave, stderr=slave)
            wait(lambda: 'BASALGRIM' in text() and bool(rows('SELECT id FROM owners')), 'startup')

        def stop():
            idle(); end = time.monotonic()+8
            while child.poll() is None:
                send(b'\x11')
                try: child.wait(timeout=.15)
                except subprocess.TimeoutExpired: assert time.monotonic() < end, 'Quit after storage operation'
            assert child.returncode == 0
            os.close(master); os.close(slave)

        def click_label(label):
            lines = text().splitlines(); y = next(i for i, line in enumerate(lines) if label in line); x = lines[y].index(label)
            send(f'\x1b[<0;{x+1};{y+1}M\x1b[<0;{x+1};{y+1}m')

        def resize(w, h):
            nonlocal width, height
            width, height = w, h
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', h, w, 0, 0)); child.send_signal(signal.SIGWINCH); pump(.2)

        try:
            launch(); send(b'\x1b[15;5~'); copied = files/'a (1).txt'
            wait(copied.exists, 'duplicate'); idle(); send(b'\x1a'); wait(lambda: not copied.exists(), 'undo'); idle()
            data = rows("SELECT operation_id,data FROM undo_items WHERE state='undone'")[0]
            parked = Path(os.fsdecode(bytes(json.loads(data[1])['parking'])))
            send(b'\x1b[17~'); resize(34, 14); click_label('Хранилище')
            wait(lambda: 'Выбрано: 1' in text(), 'compact storage catalog')
            click_label('К очистке:'); wait(lambda: 'Выбрано: 0' in text(), '7 day retention')
            for _ in range(3): click_label('К очистке:')
            wait(lambda: 'Выбрано: 1' in text(), 'all records')
            click_label('Очистить'); wait(lambda: 'Очистить данные undo/redo?' in text(), 'confirmation')
            send(b'\x1b'); assert parked.read_text() == original.read_text(), 'cancel removed retained data'
            click_label('Очистить'); send(b'\r'); wait(lambda: not parked.exists(), 'explicit disposal'); idle()
            assert rows('SELECT state FROM undo_items WHERE operation_id=?', (data[0],)) == [('expired',)]
            assert original.read_text() == 'keep original' and not copied.exists()
            stop(); launch(); send(b'\x19'); pump(.2); assert not copied.exists(), 'redo survived explicit disposal'
            send(b'\x1b[17;6~'); wait(lambda: 'Выбрано: 0' in text(), 'storage shortcut after restart')
            send(b'\x1b'); stop()
            print('History storage PTY: compact mouse, retention, cancel, disposal, intact originals and restart passed', flush=True)

            crash = base/'crash'; crash.mkdir(); tree = crash/'tree'; tree.mkdir(); count = 6000
            for i in range(count): (tree/f'{i:05}.txt').write_text('retained tree data')
            launch(crash); send(b'\x1b[15;5~'); result = crash/'tree (1)'
            wait(lambda: bool(rows("SELECT id FROM operations WHERE id=(SELECT MAX(id) FROM operations) AND state='done' AND title='Дублирование'")), 'tree duplicate', 60)
            idle(); send(b'\x1a'); wait(lambda: not result.exists(), 'tree undo'); idle()
            data = rows("SELECT operation_id,data FROM undo_items WHERE state='undone' ORDER BY operation_id DESC")[0]
            parked = Path(os.fsdecode(bytes(json.loads(data[1])['parking'])))
            send(b'\x1b[17;6~'); wait(lambda: 'Выбрано: 1' in text(), 'tree storage catalog')
            send(b'\r'); wait(lambda: 'Очистить данные undo/redo?' in text(), 'tree disposal confirmation')
            os.write(master, b'\r'); end = time.monotonic()+30
            while sum(1 for _ in parked.iterdir()) == count:
                assert child.poll() is None and time.monotonic() < end, 'first irreversible removal'
                pump(.001)
            child.send_signal(signal.SIGSTOP)
            stopped, status = os.waitpid(child.pid, os.WUNTRACED)
            assert stopped == child.pid and os.WIFSTOPPED(status), 'process did not acknowledge SIGSTOP'
            remaining = sorted(p.name for p in parked.iterdir())
            assert 0 < len(remaining) < count, 'missed partial deletion crash window'
            assert rows('SELECT state FROM undo_items WHERE operation_id=?', (data[0],)) == [('discarding',)]
            child.kill(); child.wait(timeout=5); os.close(master); os.close(slave)
            launch(crash)
            wait(lambda: rows('SELECT state FROM undo_items WHERE operation_id=?', (data[0],)) == [('discard_failed',)], 'interrupted cleanup recovery')
            assert sorted(p.name for p in parked.iterdir()) == remaining, 'startup resumed destructive cleanup'
            assert sum(1 for _ in tree.iterdir()) == count and not result.exists(), 'original tree changed'
            send(b'\x1b[17;6~'); wait(lambda: 'защищено: 1' in text(), 'protected interrupted storage')
            send(b'\x1b'); stop()
            print('History storage PTY: SIGKILL during removal preserves the remainder and blocks automatic retry passed', flush=True)
        except BaseException:
            print(text(), file=sys.stderr); raise
        finally:
            if child and child.poll() is None: child.kill(); child.wait(timeout=5)


if __name__ == '__main__': main()
