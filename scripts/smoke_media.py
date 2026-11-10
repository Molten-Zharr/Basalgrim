#!/usr/bin/env python3
"""Decode actual terminal media payloads in private PTYs, including 34x14."""
import base64,fcntl,io,json,os,pty,re,select,signal,struct,subprocess,sys,tempfile,termios,time,zlib
from pathlib import Path
from PIL import Image
sys.dont_write_bytecode=True
from media_fixture import prepare,video,RED,GREEN,BLUE
from pty_helpers import quit_when_ready


def frames(data,protocol):
    if protocol=='kitty':
        params=None;raw=bytearray()
        for header,chunk in re.findall(rb'\x1b_G([^;]*);([A-Za-z0-9+/=]*)\x1b\\',data):
            current=dict(part.split(b'=',1) for part in header.split(b',') if b'=' in part)
            if current.get(b'f')==b'32':params=current;raw.clear()
            if params is None:continue
            raw.extend(base64.b64decode(chunk))
            if current.get(b'm')==b'0':
                if params.get(b'o')==b'z':raw=bytearray(zlib.decompress(raw))
                w,h=int(params[b's']),int(params[b'v'])
                if len(raw)==w*h*4:
                    yield w,h,[c[1][:3] for c in Image.frombytes('RGBA',(w,h),bytes(raw)).getcolors(w*h)]
                params=None;raw.clear()
    elif protocol=='iterm2':
        for chunk in re.findall(rb'\x1b]1337;File=[^:]*:([A-Za-z0-9+/=]+)\x07',data):
            im=Image.open(io.BytesIO(base64.b64decode(chunk))).convert('RGB')
            yield im.width,im.height,[c[1] for c in im.getcolors(im.width*im.height)]
    elif protocol=='sixel':
        for payload in re.findall(rb'\x1bP[^q]*q(.*?)\x1b\\',data,re.S):
            size=re.search(rb'"\d+;\d+;(\d+);(\d+)',payload)
            if size:
                colors=[tuple(round(int(v)*255/100) for v in p) for p in re.findall(rb'#\d+;2;(\d+);(\d+);(\d+)',payload)]
                yield int(size[1]),int(size[2]),colors


def main():
    binary=Path(sys.argv[1] if len(sys.argv)>1 else 'target/release/basalgrim-tui').resolve()
    for protocol in ['none','kitty','sixel','iterm2']:
        with tempfile.TemporaryDirectory(prefix='basalgrim-media-') as tmp:
            base=Path(tmp);source=prepare(base)
            settings=base/'config/molten-zharr/basalgrim/settings.json';settings.parent.mkdir(parents=True)
            settings.write_text(json.dumps({'grid':True,'preview':True,'thumbnails':True,'theme':'light'}))
            env=dict(os.environ,TERM='xterm-256color',HOME=str(base),XDG_CONFIG_HOME=str(base/'config'),XDG_DATA_HOME=str(base/'data'),XDG_STATE_HOME=str(base/'state'))
            for name in ['DISPLAY','WAYLAND_DISPLAY','WAYLAND_SOCKET','TERM_PROGRAM','LC_TERMINAL','TMUX','WEZTERM_EXECUTABLE','KONSOLE_VERSION']:env.pop(name,None)
            if protocol=='iterm2':env['TERM_PROGRAM']='iTerm.app'
            master,slave=pty.openpty()
            def size(w,h):fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',h,w,w*8,h*16))
            size(150,40)
            child=subprocess.Popen([str(binary),'--no-session',str(source)],env=env,stdin=slave,stdout=slave,stderr=slave)
            output=bytearray();replied=False
            def pump(seconds=.1):
                nonlocal replied
                end=time.monotonic()+seconds
                while time.monotonic()<end:
                    if select.select([master],[],[],.01)[0]:output.extend(os.read(master,1<<20))
                    if not replied and b'\x1b[5n' in output:
                        response=b'\x1b_Gi=31;OK\x1b\\' if protocol=='kitty' else b''
                        response+=b'\x1b[?62;4c' if protocol=='sixel' else b'\x1b[?62c'
                        os.write(master,response+b'\x1b[6;16;8t\x1b[0n');replied=True
            def wait(predicate,label):
                end=time.monotonic()+12
                while not predicate():
                    assert child.poll() is None and time.monotonic()<end,f'{protocol}: {label}'
                    pump()
            def send(data):os.write(master,data);pump()
            def contains(color,minimum=0,maximum=4096):
                return any(minimum<=w<=maximum and any(all(abs(a-b)<12 for a,b in zip(c,color)) for c in colors) for w,h,colors in frames(output,protocol))
            try:
                if protocol=='none':
                    wait(lambda:'не поддерживает графику'.encode() in output,'video text fallback')
                    assert not list(frames(output,'kitty')) and not list(frames(output,'sixel')) and not list(frames(output,'iterm2'))
                else:
                    wait(lambda:contains(GREEN,100),'video preview pixels')
                    wait(lambda:contains(GREEN,0,64) and contains(RED,0,64),'image and video grid thumbnail pixels')
                    output.clear();send(b'\x07') # Ctrl+G switches to list.
                    wait(lambda:contains(GREEN,0,20) and contains(RED,0,20),'list thumbnail pixels')
                    output.clear();Image.new('RGB',(320,180),BLUE).save(source/'b-image.png')
                    wait(lambda:contains(BLUE,0,20),'watched thumbnail replacement')
                    output.clear();size(34,14);child.send_signal(signal.SIGWINCH)
                    # Kitty reuses uploaded textures on resize; rewrite to require a fresh pixel payload.
                    pump(.2);video(source/'a-video.mp4')
                    wait(lambda:contains(GREEN,0,20),'minimum-size thumbnail pixels')
                    assert not contains(GREEN,100),'hidden preview still rendered at 34x14'
                    output.clear();size(150,40);child.send_signal(signal.SIGWINCH)
                    wait(lambda:contains(GREEN,100),'preview after resize')
                    send(b'\x1b[F');pump(.3);output.clear();send(b'\x0c')
                    assert not contains(GREEN,100),'video pixels remained over text'
                quit_when_ready(child,send)
                print(f'Media PTY passed: {protocol}, actual video/image colors, grid/list, rewrite, 34x14 and text',flush=True)
            except BaseException:
                Path(f'target/media-{protocol}.ansi').write_bytes(output)
                raise
            finally:
                if child.poll() is None:child.terminate();child.wait(timeout=5)
                os.close(master);os.close(slave)

if __name__=='__main__':main()
