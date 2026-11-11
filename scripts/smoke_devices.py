#!/usr/bin/env python3
"""Real minimum-size TUI mouse against an independent private UDisks service."""
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
from devices_fixture import Fixture
from pty_helpers import quit_when_ready
from smoke_watch import screen_text


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/release/basalgrim-tui").resolve()
    for grid in (False, True):
        with tempfile.TemporaryDirectory(prefix="basalgrim-devices-") as tmp, Fixture(Path(tmp)) as fixture:
            base = Path(tmp)
            settings = base / "config/molten-zharr/basalgrim/settings.json"
            settings.parent.mkdir(parents=True)
            settings.write_text(json.dumps({"grid": grid, "preview": False, "icon_mode": "symbols"}))
            env = dict(os.environ, HOME=str(base), TERM="xterm-256color", XDG_CONFIG_HOME=str(base / "config"),
                       XDG_DATA_HOME=str(base / "data"), XDG_STATE_HOME=str(base / "state"),
                       DBUS_SYSTEM_BUS_ADDRESS=fixture.address)
            for name in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "TERM_PROGRAM", "LC_TERMINAL", "TMUX", "WEZTERM_EXECUTABLE", "KONSOLE_VERSION"):
                env.pop(name, None)
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 14, 34, 0, 0))
            child = subprocess.Popen([str(binary), "--no-session", str(fixture.root)], env=env, stdin=slave, stdout=slave, stderr=slave)
            output = bytearray()

            def pump(seconds=.1):
                end = time.monotonic() + seconds
                while time.monotonic() < end:
                    if select.select([master], [], [], .01)[0]:
                        output.extend(os.read(master, 65536))

            def send(data):
                os.write(master, data.encode() if isinstance(data, str) else data)
                pump()

            def screen(): return screen_text(output, 34, 14)

            def wait(predicate, label):
                end = time.monotonic() + 8
                while not predicate():
                    assert child.poll() is None and time.monotonic() < end, label + "\n" + screen()
                    pump()
                pump(.15)

            def click(x, y):
                send(f"\x1b[<0;{x+1};{y+1}M")
                send(f"\x1b[<0;{x+1};{y+1}m")

            def open_devices(mouse=False):
                if mouse:
                    click(4, 2)  # Compact Actions menu, including its mouse paging.
                    for _ in range(20):
                        lines = screen().splitlines()
                        match = next((y for y, line in enumerate(lines) if "Накопители" in line), None)
                        if match is not None:
                            click(5, match)
                            break
                        more = next(y for y, line in enumerate(lines) if "↓ Ещё команды" in line)
                        click(4, more)
                    else:
                        raise AssertionError("Devices is not mouse-accessible")
                else:
                    # CSI-u Ctrl+Alt+M: unlike legacy CR it retains exact modifiers.
                    send(b"\x1b[109;7u")
                wait(lambda: "USB 1" in screen(), "device list")

            try:
                wait(lambda: "local.txt" in screen(), "initial files")
                open_devices(mouse=True)
                assert all(label in screen() for label in ("Открыть", "Подключить", "Отключить", "Извлечь")), screen()
                click(3, 10)
                wait(lambda: "device-1.txt" in screen(), "mount then ordinary pane")
                assert [c["method"] for c in fixture.calls()] == ["Mount"]
                open_devices()
                fixture.update(error="DeviceBusy")
                click(24, 10)
                wait(lambda: "занят" in screen(), "busy device explanation")
                assert json.loads(fixture.state.read_text())["mounted"] == [True, True]
                fixture.update(error="")
                click(24, 10)
                wait(lambda: not json.loads(fixture.state.read_text())["mounted"][0], "unmount")
                send(b"\x1b")
                wait(lambda: "config" in screen() and "ФАЙЛЫ" in screen(), "tab leaves disconnected mount")
                assert "device-1.txt" not in screen()
                open_devices()
                click(3, 11)
                assert "Подключено разделов: 1" in screen() and "Затронуто накопителей: 1" in screen(), screen()
                count = len(fixture.calls())
                click(17, 11)
                assert len(fixture.calls()) == count
                click(3, 11)
                # Confirm captures original device identity, even if unplugged/replaced.
                fixture.update(detected=43)
                click(3, 11)
                wait(lambda: "изменились" in screen(), "stale confirmation rejected")
                assert len(fixture.calls()) == count
                pump(2.3)
                click(3, 11); click(3, 11)
                wait(lambda: not json.loads(fixture.state.read_text())["present"], "eject")
                wait(lambda: "Нет доступных" in screen(), "ejected list refreshed")
                assert [c["method"] for c in fixture.calls()][-2:] == ["Unmount", "PowerOff"]
                fixture.update(present=True, detected=44, mounted=[False, False])
                wait(lambda: "USB 1" in screen(), "automatic hotplug")
                click(13, 10)
                wait(lambda: json.loads(fixture.state.read_text())["mounted"][0], "explicit mount")
                assert "Накопители" in screen(), screen()
                fixture.update(error="NotAuthorized")
                click(24, 10)
                wait(lambda: "прав" in screen(), "authorization error")
                send(b"\x1b"); quit_when_ready(child, send)
                assert (base / "USB1/device-1.txt").exists() and (base / "USB2/device-2.txt").exists()
                print(f"Devices PTY grid={grid}: 34x14 mouse, mount/open, unmount, busy/authorization errors, stale confirm, whole-device eject, hotplug passed", flush=True)
            except BaseException:
                Path("target/devices-pty-failure.txt").write_text(screen())
                raise
            finally:
                if child.poll() is None:
                    child.terminate(); child.wait(timeout=5)
                os.close(master); os.close(slave)


if __name__ == "__main__": main()
