#!/usr/bin/env python3
"""Network login and cancellation through the actual 34x14 TUI on private GVfs."""
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
from network_fixture import Fixture, USER, PASSWORD
from smoke_watch import screen_text
from pty_helpers import quit_when_ready


def main():
    binary=Path(sys.argv[1] if len(sys.argv)>1 else "target/release/basalgrim-tui").resolve()
    for protocol in ("ftp","sftp"):
        with tempfile.TemporaryDirectory(prefix="bgnet-") as tmp, Fixture(Path(tmp),protocol) as fixture:
            base=Path(tmp);settings=base/"config/molten-zharr/basalgrim/settings.json";settings.parent.mkdir(parents=True)
            settings.write_text(json.dumps({"grid":False,"preview":False,"icon_mode":"symbols"}))
            env=dict(fixture.env,TERM="xterm-256color")
            for name in ("DISPLAY","WAYLAND_DISPLAY","WAYLAND_SOCKET","TMUX","TERM_PROGRAM","LC_TERMINAL","WEZTERM_EXECUTABLE","KONSOLE_VERSION"):env.pop(name,None)
            master,slave=pty.openpty();fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack("HHHH",14,34,0,0))
            child=subprocess.Popen([str(binary),"--no-session",str(fixture.local)],env=env,stdin=slave,stdout=slave,stderr=slave)
            output=bytearray()
            def pump(seconds=.1):
                end=time.monotonic()+seconds
                while time.monotonic()<end:
                    if select.select([master],[],[],.01)[0]:output.extend(os.read(master,65536))
            def send(data):os.write(master,data.encode() if isinstance(data,str) else data);pump()
            def screen():return screen_text(output,34,14)
            def wait(predicate,label):
                end=time.monotonic()+12
                while not predicate():
                    assert child.poll() is None and time.monotonic()<end,label+"\n"+screen()
                    pump()
                pump(.1)
            def click(x,y):send(f"\x1b[<0;{x+1};{y+1}M");send(f"\x1b[<0;{x+1};{y+1}m")
            def field(label,text):
                row=next(i for i,line in enumerate(screen().splitlines()) if label in line)
                click(3,row+1);send(b"\x15");send(text)
            def connect():
                send(b"\x1b[107;7u");wait(lambda:"Сеть" in screen(),"network shortcut")
                click(3,11);wait(lambda:"Адрес сервера" in screen(),"address field")
                if protocol=="ftp":
                    click(22,6);send(fixture.uri.removeprefix("ftp://"))
                else:field("Адрес сервера",fixture.uri)
                click(3,12)
                if protocol=="sftp" and not (base/"known_hosts").exists():
                    wait(lambda:"Log In Anyway" in screen(),"host-key question")
                    row=next(i for i,line in enumerate(screen().splitlines()) if "Log In Anyway" in line)
                    click(4,row)
                wait(lambda:"Пароль" in screen(),"credential prompt")
            try:
                wait(lambda:"ФАЙЛЫ" in screen(),"initial pane")
                connect();send(b"\x1b");wait(lambda:"ФАЙЛЫ" in screen() and "Пароль" not in screen(),"cancel auth")
                pump(.4);connect()
                if "Пользователь" in screen():field("Пользователь",USER)
                if "Домен" in screen():field("Домен","WORKGROUP")
                field("Пароль",PASSWORD);assert PASSWORD not in screen() and "********" in screen(),screen()
                click(3,12);wait(lambda:"hello.txt" in screen(),"ordinary remote file pane")
                assert "/run/user" not in screen() and "gvfs" not in screen(),screen()
                # Filter/select a file, copy through the common file engine, then navigate locally.
                send("/");send("hello.txt");send(b"\r");send(b"\x03")
                send(b"\x0c");send(b"\x15");send(str(fixture.local));send(b"\r");pump(.4);send(b"\x16")
                wait(lambda:(fixture.local/"hello.txt").exists(),"download via file clipboard")
                assert (fixture.local/"hello.txt").read_text()=="independent network server\n"
                send(b"\x1b[107;7u");wait(lambda:"Отключить" in screen(),"mounted connection list")
                click(25,11);wait(lambda:"Отключить сетевую папку" in screen(),"disconnect confirmation")
                click(3,12);wait(lambda:"Нет сетевых подключений" in screen(),"disconnect")
                send(b"\x1b");quit_when_ready(child,send)
                print(f"Network PTY {protocol}: 34x14 mouse credentials/cancel, masked secret, ordinary pane, clipboard download, confirmed disconnect passed",flush=True)
            except BaseException:
                Path("target/network-pty-failure.txt").write_text(screen());raise
            finally:
                if child.poll() is None:child.terminate();child.wait(timeout=5)
                os.close(master);os.close(slave)


if __name__=="__main__":main()
