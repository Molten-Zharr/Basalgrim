#!/usr/bin/env python3
"""Kitty OSC 72 negotiation, streaming and native gestures over an isolated real PTY."""
import base64
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time
from urllib.parse import quote_from_bytes
sys.dont_write_bytecode=True
from smoke_watch import screen_text
from pty_helpers import quit_when_ready

def main():
    binary=Path(sys.argv[1] if len(sys.argv)>1 else 'target/debug/basalgrim-tui').resolve()
    with tempfile.TemporaryDirectory(prefix='basalgrim-external-dnd-') as tmp:
        root=Path(tmp);local=root/'local';foreign=root/'foreign';local.mkdir();foreign.mkdir();(local/'folder').mkdir();(local/'keep.txt').write_text('clipboard stays')
        raw=foreign/os.fsdecode(b'b-\xff.txt')
        for path in (foreign/'a.txt',raw,foreign/'copy.txt',foreign/'untouched.txt'):path.write_bytes(b'dnd bytes')
        env=dict(os.environ,HOME=tmp,XDG_CONFIG_HOME=str(root/'config'),XDG_DATA_HOME=str(root/'data'),XDG_STATE_HOME=str(root/'state'),TERM='xterm-256color')
        for name in ('DISPLAY','WAYLAND_DISPLAY','WAYLAND_SOCKET','SSH_TTY','SSH_CONNECTION','TMUX','TERM_PROGRAM','LC_TERMINAL','WEZTERM_EXECUTABLE','KONSOLE_VERSION'):env.pop(name,None)
        settings=root/'config/molten-zharr/basalgrim/settings.json';settings.parent.mkdir(parents=True);settings.write_text(json.dumps({'grid':False,'preview':False,'icon_mode':'symbols'}))
        master,slave=pty.openpty();fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',53,170,0,0))
        process=subprocess.Popen([str(binary),'--no-session','--trash-home-only',str(local)],env=env,stdin=slave,stdout=slave,stderr=slave);os.close(slave)
        output=bytearray();answered=set()
        def pump(seconds=.1):
            end=time.monotonic()+seconds
            while time.monotonic()<end:
                if select.select([master],[],[],.01)[0]:
                    try:data=os.read(master,65536)
                    except OSError as error:
                        if error.errno==errno.EIO:return
                        raise
                    if not data:return
                    output.extend(data)
                    for query in re.findall(rb'\x1b\]72;t=q:i=(\d+);\x1b\\',output):
                        if query not in answered:
                            answered.add(query);os.write(master,b'\x1b]72;t=q:i='+query+b';\x1b\\')
        def send(data):os.write(master,data.encode() if isinstance(data,str) else data);pump()
        def wire():
            values=re.findall(rb'\x1b\]72;t=a:i=(\d+);',output)
            return int(values[-1]) if values else None
        def wait(predicate,label):
            end=time.monotonic()+8
            while not predicate():
                assert process.poll() is None and time.monotonic()<end,(label,screen_text(output)[-1200:]);pump(.02)
            pump(.2)
        def settled(predicate,label):
            wait(lambda:predicate() and 'Стоп' not in screen_text(output).splitlines()[-1],label)
        def osc(meta,payload=b'',session=None,fragment=False):
            session=wire() if session is None else session
            packet=b'\x1b]72;'+meta.encode()+f':i={session};'.encode()+payload+b'\x1b\\'
            if fragment:
                os.write(master,packet[:1]);time.sleep(.001);send(packet[1:13]);send(packet[13:])
            else:send(packet)
        def mouse(point,ctrl=False):
            code=16 if ctrl else 0
            send(f'\x1b[<{code};{point[0]+1};{point[1]+1}M');send(f'\x1b[<{code};{point[0]+1};{point[1]+1}m')
        def navigate(path):send(b'\x0c\x15\x1b[200~'+str(path).encode()+b'\x1b[201~\r');pump(.2)
        def uri(paths):return ''.join('file://'+quote_from_bytes(os.fsencode(p),safe='/')+'\r\n' for p in paths).encode()
        def start_drop(point=(34,11),action=3):
            session=wire();osc(f't=m:x={point[0]}:y={point[1]}:o={action}',b'text/uri-list')
            osc(f't=M:x={point[0]}:y={point[1]}:o={action}',b'text/uri-list')
            wait(lambda:f't=r:x=1:i={session};'.encode() in output,'request URI-list after drop');return session
        def data(payload,session):
            encoded=base64.b64encode(payload);middle=len(encoded)//2
            osc('t=r:x=1:m=1',encoded[:middle],session,True)
            osc('m=1',encoded[middle:],session,True);osc('m=0',b'',session,True)
        def acknowledged(session,operation):
            wait(lambda:f't=r:o={operation}:i={session};'.encode() in output,'protocol completion');wait(lambda:wire()!=session,'new session identity')
        try:
            wait(lambda:wire() is not None,'Kitty DnD negotiation')
            mouse((34,12));send(b'\x03')
            session=start_drop();data(uri([foreign/'a.txt',raw]),session)
            settled(lambda:(local/'folder/a.txt').exists() and (local/'folder'/raw.name).exists(),'URI batch move with raw Unix name');acknowledged(session,2)
            assert not (foreign/'a.txt').exists() and not raw.exists()
            send(b'\x1a');settled(lambda:(foreign/'a.txt').exists() and raw.exists(),'external move undo')
            send(b'\x19');settled(lambda:not (foreign/'a.txt').exists() and not raw.exists(),'external move redo')
            session=start_drop(action=1);data(uri([foreign/'copy.txt']),session)
            settled(lambda:(local/'folder/copy.txt').exists(),'copy-only drop');assert (foreign/'copy.txt').exists();acknowledged(session,1)
            # Read responses belong to the destination captured before a navigation request.
            session=start_drop(point=(34,25));other=root/'other';other.mkdir();navigate(other)
            data(uri([foreign/'untouched.txt']),session);acknowledged(session,0)
            assert (foreign/'untouched.txt').exists() and not (other/'untouched.txt').exists() and not (local/'untouched.txt').exists()
            navigate(local);session=start_drop();data(b'file:///tmp/valid\r\nfile://remote/invalid\r\n',session);acknowledged(session,0)
            session=start_drop();send(b'\x1b');wait(lambda:wire()!=session,'Escape revokes incoming drop')
            data(uri([foreign/'untouched.txt']),session);assert (foreign/'untouched.txt').exists()
            session=start_drop();mouse((166,52));wait(lambda:wire()!=session,'mouse Stop revokes incoming drop')
            data(uri([foreign/'untouched.txt']),session);assert (foreign/'untouched.txt').exists()
            # Oversized packets must be discarded through ST, including embedded command keys.
            send(b'\x1b]72;t=r:i=1;'+b'\x11'*10000+b'\x1b\\');assert process.poll() is None
            # The terminal's native offer gesture captures the same marked group as an internal drag.
            outgoing=root/'outgoing';outgoing.mkdir();(outgoing/'folder').mkdir()
            for name in ('one.txt','two.txt'):(outgoing/name).write_text(name)
            navigate(outgoing);mouse((34,12),True);mouse((34,13),True)
            session=wire();offset=len(output);osc('t=o:x=34:y=13')
            wait(lambda:b't=P:x=-1:' in output[offset:],'terminal native source offer')
            packets=re.findall(rb'\x1b\]72;t=p:x=0:m=1:i=\d+;([^\x1b]*)\x1b\\',output[offset:])
            offered=base64.b64decode(b''.join(packets));assert uri([outgoing/'one.txt',outgoing/'two.txt'])==offered,offered
            osc('t=E',b'OK')
            osc('t=e:x=5:y=0');wait(lambda:b't=e:y=0:m=1:' in output[offset:],'terminal source data request')
            osc('t=e:x=4:y=0');wait(lambda:wire()!=session,'terminal source completed')
            assert (outgoing/'one.txt').exists() and (outgoing/'two.txt').exists(),'sender deleted native source data'
            send(b'\x1b]')  # A standalone Alt+] must remain a key, never an unterminated OSC.
            proof=root/'proof';proof.mkdir();navigate(proof);send(b'\x16')
            settled(lambda:(proof/'keep.txt').exists(),'native DnD preserves original internal clipboard')
            quit_when_ready(process,send)
            print('External DnD PTY: negotiated Kitty protocol, fragmented URI batches/raw names, move/copy, undo/redo, stale reads, Escape, malformed/oversized data, native source and retained clipboard passed',flush=True)
        finally:
            out=Path('target/preview');out.mkdir(parents=True,exist_ok=True);(out/'external-dnd-pty.bin').write_bytes(output);(out/'external-dnd-pty.txt').write_text(screen_text(output))
            if process.poll() is None:process.kill();process.wait(timeout=5)
            os.close(master)

if __name__=='__main__':main()
