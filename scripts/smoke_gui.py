#!/usr/bin/env python3
"""Real native GUI regression, isolated from the user display/clipboard (Xvfb, xclip, Pillow)."""
import ctypes as C
import ctypes.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import sys
import shutil
import select
import hashlib
from PIL import Image

# Run on an isolated X server: never replace the user's clipboard or focus.
xvfb = os.environ.get("XVFB") or shutil.which("Xvfb")
xclip = os.environ.get("XCLIP") or shutil.which("xclip")
assert xvfb and xclip, "Install Xvfb and xclip, or set XVFB and XCLIP"
read_fd, write_fd = os.pipe()
server = subprocess.Popen([xvfb, "-displayfd", str(write_fd), "-screen", "0", "1600x1000x24", "-nolisten", "tcp", "-ac"], pass_fds=(write_fd,), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
os.close(write_fd)
assert select.select([read_fd], [], [], 10)[0], "Xvfb did not start"
os.environ["DISPLAY"] = ":" + os.read(read_fd, 64).decode().strip()
os.close(read_fd)

x = C.CDLL(ctypes.util.find_library('X11'))
xt = C.CDLL(ctypes.util.find_library('Xtst'))
D, W, A = C.c_void_p, C.c_ulong, C.c_ulong
x.XOpenDisplay.argtypes = [C.c_char_p]; x.XOpenDisplay.restype = D
x.XDefaultRootWindow.argtypes = [D]; x.XDefaultRootWindow.restype = W
x.XInternAtom.argtypes = [D, C.c_char_p, C.c_int]; x.XInternAtom.restype = A
x.XQueryTree.argtypes = [D, W, C.POINTER(W), C.POINTER(W), C.POINTER(C.POINTER(W)), C.POINTER(C.c_uint)]
x.XGetWindowProperty.argtypes = [D, W, A, C.c_long, C.c_long, C.c_int, A, C.POINTER(A), C.POINTER(C.c_int), C.POINTER(C.c_ulong), C.POINTER(C.c_ulong), C.POINTER(C.POINTER(C.c_ubyte))]
x.XFree.argtypes = [C.c_void_p]
x.XRaiseWindow.argtypes = [D, W]
x.XMapRaised.argtypes = [D, W]
x.XSync.argtypes = [D, C.c_int]
x.XSetInputFocus.argtypes = [D, W, C.c_int, C.c_ulong]
x.XGetInputFocus.argtypes = [D, C.POINTER(W), C.POINTER(C.c_int)]
x.XStringToKeysym.argtypes = [C.c_char_p]; x.XStringToKeysym.restype = C.c_ulong
x.XKeysymToKeycode.argtypes = [D, C.c_ulong]; x.XKeysymToKeycode.restype = C.c_uint
x.XFlush.argtypes = [D]
x.XCloseDisplay.argtypes = [D]
x.XGetGeometry.argtypes = [D, W, C.POINTER(W), C.POINTER(C.c_int), C.POINTER(C.c_int), C.POINTER(C.c_uint), C.POINTER(C.c_uint), C.POINTER(C.c_uint), C.POINTER(C.c_uint)]
xt.XTestFakeKeyEvent.argtypes = [D, C.c_uint, C.c_int, C.c_ulong]
class XImage(C.Structure):
    _fields_ = [('width', C.c_int), ('height', C.c_int), ('xoffset', C.c_int), ('format', C.c_int), ('data', C.c_void_p), ('byte_order', C.c_int), ('bitmap_unit', C.c_int), ('bitmap_bit_order', C.c_int), ('bitmap_pad', C.c_int), ('depth', C.c_int), ('bytes_per_line', C.c_int), ('bits_per_pixel', C.c_int)]
x.XGetImage.argtypes = [D, W, C.c_int, C.c_int, C.c_uint, C.c_uint, C.c_ulong, C.c_int]; x.XGetImage.restype = C.POINTER(XImage)
x.XDestroyImage.argtypes = [C.POINTER(XImage)]
d = x.XOpenDisplay(None)
assert d
root = x.XDefaultRootWindow(d)
old_focus, old_revert = W(), C.c_int()
x.XGetInputFocus(d, C.byref(old_focus), C.byref(old_revert))
pid_atom = x.XInternAtom(d, b'_NET_WM_PID', 0)
def children(win):
    rr, pp, array, count = W(), W(), C.POINTER(W)(), C.c_uint()
    if not x.XQueryTree(d, win, C.byref(rr), C.byref(pp), C.byref(array), C.byref(count)):
        return []
    result = [array[i] for i in range(count.value)]
    if array: x.XFree(array)
    return result

def pid(win):
    actual, fmt, count, rest, data = A(), C.c_int(), C.c_ulong(), C.c_ulong(), C.POINTER(C.c_ubyte)()
    x.XGetWindowProperty(d, win, pid_atom, 0, 1, 0, 0, C.byref(actual), C.byref(fmt), C.byref(count), C.byref(rest), C.byref(data))
    result = C.cast(data, C.POINTER(C.c_ulong))[0] if data and count.value else 0
    if data: x.XFree(data)
    return result

def find_window(process):
    pending = [root]
    for _ in range(4):
        next_level = []
        for parent in pending:
            for win in children(parent):
                if pid(win) == process.pid:
                    return win
                next_level.append(win)
        pending = next_level
    return None

def key(name, down=None):
    code = x.XKeysymToKeycode(d, x.XStringToKeysym(name.encode()))
    assert code
    for pressed in ((1, 0) if down is None else (int(down),)):
        xt.XTestFakeKeyEvent(d, code, pressed, 0)
    x.XFlush(d)
    time.sleep(.025)

def screenshot(win, name):
    rr, px, py = W(), C.c_int(), C.c_int()
    width, height, border, depth = C.c_uint(), C.c_uint(), C.c_uint(), C.c_uint()
    assert x.XGetGeometry(d, win, C.byref(rr), C.byref(px), C.byref(py), C.byref(width), C.byref(height), C.byref(border), C.byref(depth))
    raw = x.XGetImage(d, win, 0, 0, width.value, height.value, C.c_ulong(-1).value, 2)
    assert raw and raw.contents.bits_per_pixel == 32
    info = raw.contents
    data = C.string_at(info.data, info.bytes_per_line * info.height)
    image = Image.frombytes('RGB', (info.width, info.height), data, 'raw', 'BGRX', info.bytes_per_line)
    x.XDestroyImage(raw)
    image.save(Path('target/preview') / name)
    return image

x.XResizeWindow.argtypes = [D, W, C.c_uint, C.c_uint]
x.XTranslateCoordinates.argtypes = [D, W, W, C.c_int, C.c_int, C.POINTER(C.c_int), C.POINTER(C.c_int), C.POINTER(W)]
x.XGetSelectionOwner.argtypes = [D, A]; x.XGetSelectionOwner.restype = W
xt.XTestFakeMotionEvent.argtypes = [D, C.c_int, C.c_int, C.c_int, C.c_ulong]
xt.XTestFakeButtonEvent.argtypes = [D, C.c_uint, C.c_int, C.c_ulong]
Path('target/preview').mkdir(parents=True, exist_ok=True)
binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/basalgrim').resolve()

def chord(letter, *mods):
    for m in mods: key(m, True)
    key(letter)
    for m in reversed(mods): key(m, False)

def move(win, cx, cy):
    # The test uses bundled Fira Code at 16px: exactly 8x16 physical cells.
    px, py, child = C.c_int(), C.c_int(), W()
    x.XTranslateCoordinates(d, win, root, int((cx+.5)*8), int((cy+.5)*16), C.byref(px), C.byref(py), C.byref(child))
    xt.XTestFakeMotionEvent(d, -1, px.value, py.value, 0)
    x.XFlush(d)

def click(win, cx, cy, button=1):
    move(win, cx, cy)
    xt.XTestFakeButtonEvent(d, button, 1, 0); xt.XTestFakeButtonEvent(d, button, 0, 0)
    x.XFlush(d); time.sleep(.18)

def band_begin(win, start, finish):
    move(win,*start); xt.XTestFakeButtonEvent(d,1,1,0); x.XFlush(d); time.sleep(.12)
    for step in range(1,5):
        move(win, start[0]+(finish[0]-start[0])*step/4, start[1]+(finish[1]-start[1])*step/4)
        time.sleep(.08)

def band_end():
    xt.XTestFakeButtonEvent(d,1,0,0); x.XFlush(d); time.sleep(.18)

def wait_for(predicate, description):
    until = time.monotonic() + 6
    while not predicate():
        assert time.monotonic() < until, description
        time.sleep(.01)

def type_text(text):
    for c in text:
        name = {'/':'slash', '.':'period', '-':'minus', '_':'underscore'}.get(c,c)
        if c == '_': chord('minus', 'Shift_L')
        else: key(name)

def navigate(path):
    chord('l', 'Control_L'); chord('u','Control_L'); type_text(str(path)); key('Return'); time.sleep(.2)

def search(name):
    key('slash'); type_text(name); key('Return'); time.sleep(.2)

def launch(path, config):
    env = dict(os.environ, XDG_CONFIG_HOME=str(config), XDG_DATA_HOME=str(config.parent/'data'), LIBGL_ALWAYS_SOFTWARE='1', WINIT_X11_SCALE_FACTOR='1')
    for n in ('WAYLAND_DISPLAY','WAYLAND_SOCKET'): env.pop(n, None)
    log = open('target/preview/native-gui.log', 'w')
    process = subprocess.Popen([str(binary), str(path)], env=env, stdout=log, stderr=log)
    window = None; until=time.monotonic()+10
    while window is None:
        assert process.poll() is None, 'GUI exited before opening'
        assert time.monotonic() < until, 'GUI window timeout'
        window = find_window(process); time.sleep(.05)
    x.XMapRaised(d,window); x.XSync(d,0); time.sleep(.5)
    x.XSetInputFocus(d,window,2,0); x.XFlush(d); time.sleep(.4)
    return process, window, log

def stop(process,log):
    if process.poll() is None:
        process.terminate(); process.wait(timeout=5)
    log.close()

try:
    with tempfile.TemporaryDirectory(prefix='basalgrim-gui-') as tmp:
        base=Path(tmp); files=base/'files'; files.mkdir()
        source=files/'source'; source.mkdir()
        destination=files/'destination'; destination.mkdir()
        (source/'alpha.txt').write_text('native keyboard copy')
        config=base/'config'; settings=config/'molten-zharr/basalgrim/settings.json'
        settings.parent.mkdir(parents=True)
        settings.write_text(json.dumps({'preview':False, 'grid':False,'font_size':16,'theme':'light'}))
        clipboard_atom=x.XInternAtom(d,b'CLIPBOARD',0)
        assert x.XGetSelectionOwner(d,clipboard_atom)==0, 'Clipboard must start empty'
        process,window,log=launch(source,config)
        try:
            chord('c','Control_L'); navigate(destination); chord('v','Control_L')
            wait_for(lambda:(destination/'alpha.txt').exists(),'Ctrl+C/V with empty OS clipboard')
            assert (source/'alpha.txt').read_text()==(destination/'alpha.txt').read_text()
            chord('x','Control_L'); navigate(files); chord('v','Control_L')
            wait_for(lambda:(files/'alpha.txt').exists(),'Ctrl+X/V move')
            assert not (destination/'alpha.txt').exists()
            assert x.XGetSelectionOwner(d,clipboard_atom)==0, 'File commands unexpectedly wrote system text clipboard'
            search('alpha.txt'); chord('c','Control_L'); navigate(destination); chord('v','Control_L')
            wait_for(lambda:(destination/'alpha.txt').exists(),'Slash search then copy')
            screenshot(window,'native-keyboard-fixed.png')
            # A real text clipboard must still paste into editors and must not
            # cause duplicate file commands. This clipboard belongs to Xvfb only.
            owner=subprocess.Popen([xclip,'-selection','clipboard','-quiet'],stdin=subprocess.PIPE,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            owner.stdin.write(b'native-pasted.txt'); owner.stdin.close()
            wait_for(lambda:x.XGetSelectionOwner(d,clipboard_atom)!=0,'text clipboard owner')
            try:
                chord('n','Control_L'); chord('v','Control_L'); key('Return')
                wait_for(lambda:(destination/'native-pasted.txt').exists(),'OS text paste into New File editor')
                search('native-pasted.txt'); chord('c','Control_L'); navigate(files); chord('v','Control_L')
                wait_for(lambda:(files/'native-pasted.txt').exists(),'file paste with nonempty text clipboard')
            finally:
                owner.terminate(); owner.wait(timeout=5)
            # Capture a clipboard-shaped combination before command dispatch.
            key('F10'); key('Down'); key('Right'); key('Down'); key('Return')
            chord('v','Control_L','Alt_L','Shift_L')
            wait_for(lambda:any(b['action']=='Help' and b['key']=={'key':{'Char':'v'},'ctrl':True,'alt':True,'shift':True} for b in json.loads(settings.read_text()).get('bindings',[])), 'native clipboard-key shortcut capture')
            key('Escape'); chord('v','Control_L','Alt_L','Shift_L'); key('Return')
            chord('q','Control_L'); process.wait(timeout=5); assert process.returncode==0
            print('Native GUI: Ctrl+C/X/V, empty and nonempty OS clipboard, slash search, editor paste passed', flush=True)
        except Exception:
            screenshot(window,'native-keyboard-failure.png'); raise
        finally: stop(process,log)
        preferences=json.loads(settings.read_text())
        for b in preferences['bindings']:
            if b['action'] in ('Copy','Cut','Paste'): b['key']['shift']=True
        settings.write_text(json.dumps(preferences))
        custom=base/'custom'; custom.mkdir(); (custom/'destination').mkdir()
        (custom/'beta.txt').write_text('custom modifiers')
        process,window,log=launch(custom,config)
        try:
            search('beta.txt'); chord('c','Control_L'); navigate(custom/'destination'); chord('v','Control_L')
            time.sleep(.2); assert not (custom/'destination/beta.txt').exists(), 'Reassigned defaults must not dispatch'
            navigate(custom); search('beta.txt'); chord('c','Control_L','Shift_L')
            navigate(custom/'destination'); chord('v','Control_L','Shift_L')
            wait_for(lambda:(custom/'destination/beta.txt').exists(),'reassigned Ctrl+Shift+C/V')
            chord('x','Control_L','Shift_L'); navigate(source); chord('v','Control_L','Shift_L')
            wait_for(lambda:(source/'beta.txt').exists(),'reassigned Ctrl+Shift+X/V')
            assert not (custom/'destination/beta.txt').exists()
            chord('q','Control_L'); process.wait(timeout=5); assert process.returncode==0
            print('Native GUI: custom clipboard shortcuts, exact modifiers and capture passed',flush=True)
        except Exception:
            screenshot(window,'native-custom-failure.png'); raise
        finally: stop(process,log)
        # Restore default combinations for the mouse scenario.
        for b in preferences['bindings']:
            if b['action'] in ('Copy','Cut','Paste'): b['key']['shift']=False
        settings.write_text(json.dumps(preferences))
        # Fresh mouse-only file operations. Keep two directories and one file
        # so the row positions have explicit, stable meaning.
        mouse=base/'mouse'; mouse.mkdir()
        (mouse/'destination').mkdir(); (mouse/'moved').mkdir()
        (mouse/'alpha.txt').write_text('native mouse copy')
        process,window,log=launch(mouse,config)
        try:
            # Every command row owns its hover area, including disabled Paste.
            click(window,4,3); move(window,0,0); time.sleep(.2)
            baseline=screenshot(window,'native-menu-hover-baseline.png')
            for name,row in [('enabled',8),('disabled',10)]:
                move(window,6,row); time.sleep(.2)
                hovered=screenshot(window,f'native-menu-hover-{name}.png')
                assert baseline.tobytes()!=hovered.tobytes(), f'{name} row has no hover'
                for box in [(0,0,baseline.width,row*16),(0,(row+1)*16,baseline.width,baseline.height)]:
                    assert baseline.crop(box).tobytes()==hovered.crop(box).tobytes(), f'{name} hover repainted outside its row'
            move(window,4,4); time.sleep(.2) # Popup padding, not a command.
            assert screenshot(window,'native-menu-hover-padding.png').tobytes()==baseline.tobytes(), 'Menu background flashed on hover'
            click(window,6,10,3) # Dismiss the complete command palette.
            screenshot(window,'native-mouse-start.png')
            # 170x53 cells, first file row 11. A file has five commands.
            click(window,34,13,3); screenshot(window,'native-context-menu.png')
            click(window,36,15) # Open, Copy, Cut, Rename, Trash
            click(window,34,11); click(window,34,11)
            # Directory background -> New File, New Folder, Paste, ...
            click(window,34,22,3); screenshot(window,'native-directory-menu.png'); click(window,36,25)
            wait_for(lambda:(mouse/'destination/alpha.txt').exists(),'mouse context copy/paste')
            # Right click the pasted file -> Cut, navigate Parent via toolbar.
            click(window,34,11,3); click(window,36,14)
            click(window,21,3)
            # Open moved (row12), then Paste from the background menu.
            click(window,34,12); click(window,34,12)
            click(window,34,22,3); click(window,36,25)
            wait_for(lambda:(mouse/'moved/alpha.txt').exists(),'mouse context cut/paste')
            assert not (mouse/'destination/alpha.txt').exists()
            screenshot(window,'native-mouse-fixed.png')
            # Compact layout retains a menu; its scrolling controls are clickable.
            x.XResizeWindow(d,window,560,416); x.XSync(d,0); time.sleep(.4)
            click(window,4,3); screenshot(window,'native-context-compact.png')
            click(window,8,11) # NewFile index9 at y11 in full-height popup.
            type_text('compact.txt')
            click(window,4,16) # Confirm: centered 10-row editor, bottom y16.
            wait_for(lambda:(mouse/'moved/compact.txt').exists(),'compact mouse New File and confirmation')
            print('Native GUI: file/directory menus, row-only hover including disabled commands, mouse copy/cut/paste and compact New File passed', flush=True)
        except Exception:
            screenshot(window,'native-mouse-failure.png'); raise
        finally: stop(process,log)
        group=base/'group'; group.mkdir(); (group/'destination').mkdir()
        for name in ('alpha.txt','beta.txt','gamma.txt'): (group/name).write_text(name)
        process,window,log=launch(group,config)
        try:
            key('Control_L',True); click(window,34,12); click(window,34,13); key('Control_L',False)
            click(window,34,12,3); screenshot(window,'native-group-menu.png')
            click(window,36,13) # Group has only Copy, Cut, Trash.
            click(window,34,11); click(window,34,11)
            click(window,34,22,3); click(window,36,25)
            wait_for(lambda:all((group/'destination'/n).exists() for n in ('alpha.txt','beta.txt')),'Ctrl-selected group copy')
            assert not (group/'destination/gamma.txt').exists(), 'Copied an unmarked file'
            print('Native GUI: compact group menu and copying only Ctrl-selected files passed',flush=True)
        except Exception:
            screenshot(window,'native-group-failure.png'); raise
        finally: stop(process,log)
        band_files=base/'band'; band_files.mkdir(); (band_files/'destination').mkdir()
        for name in ('alpha.txt','beta.txt','gamma.txt'): (band_files/name).write_text(name)
        process,window,log=launch(band_files,config)
        try:
            band_begin(window,(76,13),(26,12)); screenshot(window,'native-selection-list.png'); band_end()
            chord('c','Control_L'); navigate(band_files/'destination'); chord('v','Control_L')
            wait_for(lambda:all((band_files/'destination'/n).exists() for n in ('alpha.txt','beta.txt')),'list rectangle copy')
            assert not (band_files/'destination/gamma.txt').exists()
            navigate(band_files)
            band_begin(window,(76,13),(26,12)); band_end()
            key('Control_L',True); band_begin(window,(76,14),(26,14)); band_end(); key('Control_L',False)
            screenshot(window,'native-selection-ctrl.png')
            all_files=base/'band-all'; all_files.mkdir()
            chord('c','Control_L'); navigate(all_files); chord('v','Control_L')
            wait_for(lambda:all((all_files/n).exists() for n in ('alpha.txt','beta.txt','gamma.txt')),'Ctrl additive rectangle')
            navigate(band_files); band_begin(window,(76,13),(26,12)); band_end()
            band_begin(window,(76,14),(26,14)); key('Escape'); band_end()
            cancelled=base/'band-cancelled'; cancelled.mkdir()
            chord('c','Control_L'); navigate(cancelled); chord('v','Control_L')
            wait_for(lambda:all((cancelled/n).exists() for n in ('alpha.txt','beta.txt')),'Escape restores original rectangle selection')
            assert not (cancelled/'gamma.txt').exists()
            navigate(band_files); chord('g','Control_L'); time.sleep(.25)
            band_begin(window,(78,15),(44,10))
            rectangle=screenshot(window,'native-selection-grid.png')
            # The thin image-backend outline stays above SVG icons.
            left,top,right,bottom=44*8,10*16,79*8-1,16*16-1
            accent=rectangle.getpixel((left,top))
            assert all(rectangle.getpixel((px,top))==accent and rectangle.getpixel((px,bottom))==accent for px in range(left,right+1)), 'Broken horizontal selection outline'
            assert all(rectangle.getpixel((left,py))==accent and rectangle.getpixel((right,py))==accent for py in range(top,bottom+1)), 'Broken vertical selection outline'
            band_end(); chord('c','Control_L')
            grid_copy=base/'band-grid'; grid_copy.mkdir(); navigate(grid_copy); chord('v','Control_L')
            wait_for(lambda:all((grid_copy/n).exists() for n in ('alpha.txt','beta.txt')),'grid rectangle copy')
            assert not (grid_copy/'gamma.txt').exists()
            print('Native GUI: grid/list rubber band, outline above SVG, Ctrl addition, Escape cancellation and exact copied sets passed',flush=True)
        except Exception:
            screenshot(window,'native-selection-failure.png'); raise
        finally: stop(process,log)
        # Reset the persisted grid mode for the remaining scenario.
        preferences['grid']=False; settings.write_text(json.dumps(preferences))
        wheel_files=base/'wheel'; wheel_files.mkdir()
        for index in range(100): (wheel_files/f'file-{index:03}.txt').write_text(f'File {index}\n')
        for grid in (False,True):
            mode='grid' if grid else 'list'
            preferences['grid']=grid; settings.write_text(json.dumps(preferences))
            process,window,log=launch(wheel_files,config)
            try:
                move(window,50,20); time.sleep(.2)
                before=screenshot(window,f'native-wheel-{grid}-before.png')
                notches=1 if grid else 3
                for _ in range(notches): click(window,50,20,5)
                after=screenshot(window,f'native-wheel-{grid}-after.png')
                top=(200,10*16,1000,14*16)
                assert before.crop(top).tobytes()!=after.crop(top).tobytes(), 'Wheel did not scroll file contents'
                protected=base/f'wheel-protected-{mode}'; protected.mkdir()
                chord('c','Control_L'); navigate(protected); chord('v','Control_L')
                wait_for(lambda:(protected/'file-000.txt').exists(),'wheel preserves original keyboard selection')
                assert sorted(p.name for p in protected.iterdir())==['file-000.txt']
                navigate(wheel_files)
                for _ in range(notches): click(window,50,20,5)
                click(window,34,12 if grid else 11)
                # XTest emits legacy wheel press/release pairs; winit maps
                # both to wheel events. Wayland and PTY test one event per notch.
                expected=f'file-{16 if grid else 18:03}.txt'
                clicked=base/f'wheel-clicked-{mode}'; clicked.mkdir()
                chord('c','Control_L'); navigate(clicked); chord('v','Control_L')
                wait_for(lambda:(clicked/expected).exists(),'click resolves the newly scrolled first entry')
                if not grid:
                    navigate(wheel_files)
                    for _ in range(3): click(window,50,20,5)
                    key('Down'); chord('c','Control_L')
                    keyboard=base/'wheel-keyboard'; keyboard.mkdir(); navigate(keyboard); chord('v','Control_L')
                    wait_for(lambda:(keyboard/'file-001.txt').exists(),'keyboard navigation continues from the preserved focus')
                print(f'Native GUI wheel: {"grid" if grid else "list"}, content scroll, unchanged selection, correct visible entry and keyboard focus passed',flush=True)
            except Exception:
                screenshot(window,'native-wheel-failure.png'); raise
            finally: stop(process,log)
        preferences['grid']=False; settings.write_text(json.dumps(preferences))
        operations=base/'operations'; operations.mkdir()
        op_destination=base/'op-destination'; op_destination.mkdir()
        large=operations/'large.bin'
        with large.open('wb') as stream: stream.truncate(2*1024*1024*1024)
        process,window,log=launch(operations,config)
        try:
            chord('c','Control_L'); navigate(op_destination); chord('v','Control_L')
            def copying():
                for path in op_destination.glob('.basalgrim-copy-*/item'):
                    try:
                        if path.stat().st_size>0: return True
                    except FileNotFoundError: pass
                return False
            wait_for(copying,'native copy must have written bytes before cancellation')
            key('F6'); click(window,20,51) # Full shared operations screen: Stop.
            wait_for(lambda:not list(op_destination.glob('.basalgrim-copy-*')),'native mouse cancellation cleanup')
            assert not (op_destination/'large.bin').exists(), 'Cancelled GUI copy published output'
            assert large.stat().st_size==2*1024*1024*1024
            screenshot(window,'native-operations-mouse-cancelled.png')
            chord('q','Control_L'); process.wait(timeout=5); assert process.returncode==0
            print('Native GUI: copy progress, F6 details, mouse Stop, staging cleanup and intact source passed',flush=True)
        except Exception:
            screenshot(window,'native-operations-failure.png'); raise
        finally: stop(process,log)
    print('Verified binary:', binary, hashlib.sha256(binary.read_bytes()).hexdigest(), flush=True)
finally:
    x.XCloseDisplay(d)
    server.terminate(); server.wait(timeout=5)
