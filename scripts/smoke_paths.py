#!/usr/bin/env python3
"""Real PTY breadcrumb navigation, compact parents and exact text clipboard."""
import base64
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import shlex
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


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/basalgrim-tui").resolve()
    with tempfile.TemporaryDirectory(prefix="basalgrim-path-") as tmp:
        base = Path(tmp)
        folder = base / "path parent" / "Кириллица spaces"
        folder.mkdir(parents=True)
        (folder / "a.txt").write_text("clipboard survives path copy")
        destination = base / "destination"; destination.mkdir()
        settings = base / "config/molten-zharr/basalgrim/settings.json"
        settings.parent.mkdir(parents=True)
        settings.write_text(json.dumps({"grid": False,"preview": False,"icon_mode": "symbols"}))
        env = dict(os.environ, TERM="xterm-256color", HOME=str(base), XDG_CONFIG_HOME=str(base/"config"), XDG_STATE_HOME=str(base/"state"), XDG_DATA_HOME=str(base/"data"))
        # Always exercise OSC 52 in a private PTY; never touch user clipboards.
        for name in ("DISPLAY","WAYLAND_DISPLAY","WAYLAND_SOCKET","TERM_PROGRAM","LC_TERMINAL","TMUX","WEZTERM_EXECUTABLE","KONSOLE_VERSION"):
            env.pop(name,None)
        master, slave = pty.openpty(); width,height=170,40
        fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack("HHHH",height,width,0,0))
        child = subprocess.Popen([str(binary),"--no-session",str(folder)],stdin=slave,stdout=slave,stderr=slave,env=env)
        output = bytearray()
        def pump(seconds=.13):
            end=time.monotonic()+seconds
            while time.monotonic()<end:
                if select.select([master],[],[],max(0,min(.02,end-time.monotonic())))[0]: output.extend(os.read(master,1<<20))
        def send(value): os.write(master,value.encode() if isinstance(value,str) else value);pump()
        def text():
            # Strip OSC strings before replaying cell differences.
            clean=re.sub(rb"\x1b\][^\x07]*?(?:\x07|\x1b\\)",b"",output)
            return screen_text(clean,width,height)
        def wait(predicate,label):
            end=time.monotonic()+8
            while not predicate():
                assert child.poll() is None and time.monotonic()<end,label
                pump(.02)
        def click_label(label, row_filter=lambda row:True):
            rows=text().splitlines(); y=next(y for y,row in enumerate(rows) if label in row and row_filter(row))
            x=rows[y].index(label)
            send(f"\x1b[<0;{x+1};{y+1}M\x1b[<0;{x+1};{y+1}m")
        def copy(expected):
            start=len(output);click_label("Коп.",lambda row:"Коп." in row)
            wait(lambda:b"\x1b]52;c;" in output[start:],"OSC 52 clipboard output")
            payload=re.findall(rb"\x1b\]52;c;([^\x07\x1b]*)(?:\x07|\x1b\\)",output[start:])[-1]
            assert base64.b64decode(payload).decode()==str(expected),(payload,expected)
        def navigate(path): send(b"\x0c");send(str(path));send(b"\r")
        try:
            wait(lambda:"a.txt" in text(),"initial directory")
            send(b"\x03");copy(folder)
            click_label("Кириллица spaces",lambda row:"Коп." in row)
            assert "Ввод" in text(),"current breadcrumb must edit inline"
            send(b"\x03");send(b"\x1b")
            click_label("path parent",lambda row:"Коп." in row);pump(.2);copy(folder.parent)
            send(b"\x1b[1;3D");pump(.2);copy(folder)
            send(b"\x0c\x03")
            start=len(output);send(b"\x03");wait(lambda:b"\x1b]52;c;" in output[start:],"Ctrl+L Ctrl+C")
            send(b"\x1b");navigate(destination);send(b"\x16")
            wait(lambda:(destination/"a.txt").exists(),"text copying must preserve file clipboard")
            pump(.3)
            long=base
            for i in range(12):long=long/f"long-parent-{i}"
            long.mkdir(parents=True);navigate(long)
            width,height=34,14
            fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack("HHHH",height,width,0,0));child.send_signal(signal.SIGWINCH);pump(.3)
            copy(long);click_label("…",lambda row:"Коп." in row)
            wait(lambda:"Папки пути" in text(),"compact hidden parents")
            send(b"\x1b[H");pump(.1)
            click_label(" / ",lambda row:row.startswith("│ / ") and "›" not in row)
            pump(.2);copy(Path("/"))
            quit_when_ready(child, send)
            print("Paths PTY: breadcrumbs, history, Unicode clipboard, editor, file clipboard and compact parents passed")
            width,height=170,40
            fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack("HHHH",height,width,0,0))
            helpers=base/"helpers";helpers.mkdir();captured=base/"copied.txt"
            helper=helpers/"wl-copy"
            helper.write_text("#!/bin/sh\nexec /bin/cat > "+shlex.quote(str(captured))+"\n");helper.chmod(0o700)
            local=dict(env,WAYLAND_DISPLAY="private-test",PATH=str(helpers))
            output.clear()
            child=subprocess.Popen([str(binary),"--no-session",str(folder)],stdin=slave,stdout=slave,stderr=slave,env=local)
            wait(lambda:"a.txt" in text(),"local-helper startup");click_label("Коп.",lambda row:"Коп." in row)
            wait(lambda:captured.exists() and captured.read_text()==str(folder),"bounded local helper")
            wait(lambda:"Текст скопирован" in text(),"local-helper acknowledgement")
            assert b"\x1b]52;c;" not in output,"local clipboard success also sent OSC 52"
            quit_when_ready(child, send)
            output.clear()
            child=subprocess.Popen([str(binary),"--no-session",str(folder)],stdin=slave,stdout=slave,stderr=slave,env=dict(env,TERM="linux"))
            wait(lambda:"a.txt" in text(),"console startup");click_label("Коп.",lambda row:"Коп." in row)
            wait(lambda:"Не удалось скопировать текст" in text(),"unsupported clipboard error")
            assert b"\x1b]52;c;" not in output,"Linux console received unsupported clipboard controls"
            quit_when_ready(child, send)
            print("Paths PTY: local clipboard helper and unsupported console passed")
        except BaseException:
            print(text(),file=sys.stderr);raise
        finally:
            if child.poll() is None:child.kill();child.wait(timeout=5)
            os.close(master);os.close(slave)


if __name__ == "__main__": main()
