#!/usr/bin/env python3
"""Native Wayland input regression on a private headless Sway compositor.

Requires sway, swaymsg, wtype, wl-copy, wl-paste, grim, cc, pkg-config,
wayland-scanner and Wayland client development headers. Every GUI tool can
be selected through its uppercase environment variable (hyphens become _).
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import select
import subprocess
import sys
import tempfile
import time
from PIL import Image
import preview_fixture


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/release/basalgrim").resolve()
    commands = {}
    for name in ("sway", "swaymsg", "wtype", "wl-copy", "wl-paste", "grim"):
        path = os.environ.get(name.upper().replace("-", "_")) or shutil.which(name)
        assert path, f"Missing {name}"
        commands[name] = str(Path(path).resolve())
    out = Path("target/preview").resolve()
    out.mkdir(parents=True, exist_ok=True)
    build = Path("target/wayland-smoke").resolve()
    build.mkdir(parents=True, exist_ok=True)
    protocol = Path(__file__).parent / "protocols/wlr-virtual-pointer-unstable-v1.xml"
    subprocess.run(["wayland-scanner", "client-header", str(protocol), str(build / "virtual-pointer.h")], check=True)
    subprocess.run(["wayland-scanner", "private-code", str(protocol), str(build / "virtual-pointer.c")], check=True)
    flags = subprocess.check_output(["pkg-config", "--cflags", "--libs", "wayland-client"], text=True).split()
    subprocess.run(["cc", "-std=c11", "-D_DEFAULT_SOURCE", "-Wall", "-Wextra", "-Werror",
                    "-I", str(build), str(Path(__file__).parent / "wayland_pointer.c"),
                    str(build / "virtual-pointer.c"), "-o", str(build / "pointer"), *flags], check=True)
    with tempfile.TemporaryDirectory(prefix="basalgrim-wayland-") as tmp:
        base = Path(tmp)
        runtime = base / "runtime"
        runtime.mkdir(mode=0o700)
        config = base / "sway.conf"
        config.write_text('''output * mode 1600x1000
xwayland disable
default_border none
seat seat0 fallback true
for_window [title="Basalgrim.*"] floating enable, resize set 1360 px 860 px, move absolute position 0 px 0 px
''')
        env = dict(os.environ, XDG_RUNTIME_DIR=str(runtime), WLR_BACKENDS="headless",
                   WLR_RENDERER="pixman", WLR_LIBINPUT_NO_DEVICES="1",
                   LIBGL_ALWAYS_SOFTWARE="1", XDG_STATE_HOME=str(base / "state"), XDG_CONFIG_HOME=str(base / "config"),
                   XDG_DATA_HOME=str(base / "data"), HOME=str(base), GIO_USE_VFS="local")
        for name in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "SWAYSOCK"):
            env.pop(name, None)
        server_log = open(out / "native-wayland-server.log", "w")
        server = subprocess.Popen([commands["sway"], "--unsupported-gpu", "-c", str(config)],
                                  env=env, stdout=server_log, stderr=server_log)
        process = None
        pointer = None
        keyboard = None
        text_owners = []
        app_log = open(out / "native-wayland.log", "w")

        def wait_for(predicate, description, seconds=8):
            end = time.monotonic() + seconds
            while not predicate():
                assert server.poll() is None, "Private compositor exited; see native-wayland-server.log"
                assert time.monotonic() < end, description
                time.sleep(.02)

        def run(name, *args, check=True):
            return subprocess.run([commands[name], *args], env=env, check=check,
                                  capture_output=True, text=True, timeout=8)

        def key(name, *mods):
            # Each wtype process creates a keyboard. Wait for the compositor's
            # focus/keymap handshake before pressing, and before destroying it.
            args = ["-s", "100"]
            for mod in mods:
                args += ["-M", mod]
            args += ["-k", name]
            for mod in reversed(mods):
                args += ["-m", mod]
            args += ["-s", "100"]
            run("wtype", *args)
            time.sleep(.09)

        def navigate(path):
            key("l", "ctrl"); key("u", "ctrl")
            run("wtype", "-s", "100", str(path), "-s", "100"); key("Return"); time.sleep(.15)

        def click(cx, cy, button=1):
            # Floating window at (0,0), bundled Fira Code 16px -> 8x16 cells.
            pointer.stdin.write(f"{int((cx+.5)*8)} {int((cy+.5)*16)} {button}\n")
            pointer.stdin.flush()
            assert select.select([pointer.stdout], [], [], 8)[0], "Virtual pointer timed out"
            assert pointer.stdout.readline().strip() == "clicked"
            time.sleep(.18)

        def find(node, pid):
            if node.get("pid") == pid:
                return node
            for child in node.get("nodes", []) + node.get("floating_nodes", []):
                if found := find(child, pid):
                    return found
            return None

        def launch(path):
            child = subprocess.Popen([str(binary), "--trash-home-only", *([] if path is None else [str(path)])], env=env, stdout=app_log, stderr=app_log)
            def mapped():
                assert child.poll() is None, "Native Wayland app exited; see native-wayland.log"
                return find(json.loads(run("swaymsg", "-t", "get_tree").stdout), child.pid)
            wait_for(mapped, "Native Wayland window not mapped")
            node = mapped()
            assert node["shell"] == "xdg_shell", node
            assert node["rect"] == {"x": 0, "y": 0, "width": 1360, "height": 860}, node["rect"]
            time.sleep(.4)
            return child

        try:
            wait_for(lambda: list(runtime.glob("sway-ipc.*.sock")), "Private IPC socket")
            env["SWAYSOCK"] = str(next(runtime.glob("sway-ipc.*.sock")))
            env["WAYLAND_DISPLAY"] = next(p.name for p in runtime.glob("wayland-*") if not p.name.endswith(".lock"))
            pointer = subprocess.Popen([str(build / "pointer")], env=env, stdin=subprocess.PIPE,
                                       stdout=subprocess.PIPE, stderr=app_log, text=True)
            assert select.select([pointer.stdout], [], [], 8)[0], "Virtual pointer did not start"
            assert pointer.stdout.readline().strip() == "ready"
            # Clipboard ownership uses keyboard focus even for mouse copy.
            # Keep a seat keyboard alive between short-lived wtype commands,
            # as with a real desktop; otherwise the headless seat loses it.
            keyboard = subprocess.Popen([commands["wtype"], "-s", "3600000", "-k", "F24"], env=env, stdout=app_log, stderr=app_log)
            time.sleep(.2)
            settings = base / "config/molten-zharr/basalgrim/settings.json"
            settings.parent.mkdir(parents=True)
            settings.write_text(json.dumps({"preview": False, "grid": False, "font_size": 16, "theme": "light"}))
            if '--devices-only' in sys.argv[2:]:
                import devices_fixture
                with devices_fixture.Fixture(base) as fixture:
                    env['DBUS_SYSTEM_BUS_ADDRESS']=fixture.address
                    settings.write_text(json.dumps({'preview':False,'grid':False,'font_size':16,'theme':'molten-zharr','icon_mode':'symbols'}))
                    process=launch(fixture.root)
                    try:
                        devices_fixture.exercise(fixture,key,click,lambda name:run('grim','-g','0,0 1360x860',str(out/('native-wayland-devices-'+name+'.png'))),wait_for)
                        key('q','ctrl');process.wait(timeout=5);assert process.returncode==0
                    finally:
                        if process.poll() is None:process.terminate();process.wait(timeout=5)
                        process=None
                return
            if '--places-only' in sys.argv[2:]:
                import places_fixture
                source=places_fixture.prepare(base)
                env['PATH']=str(base/'bin')+os.pathsep+env['PATH']
                env['BASALGRIM_OPEN_LOG']=str(base/'opened.txt')
                settings.write_text(json.dumps({'preview':False,'grid':False,'font_size':16,'theme':'molten-zharr','icon_mode':'symbols'}))
                process=launch(source)
                places_fixture.exercise(base,source,key,lambda text:run('wtype',text),click,lambda name:run('grim','-g','0,0 1360x860',str(out/('native-wayland-places-'+name+'.png'))),wait_for)
                key('q','ctrl');process.wait(timeout=5);process=None
                process=launch(source);key('b','ctrl');time.sleep(.3)
                run('grim','-g','0,0 1360x860',str(out/'native-wayland-places-restart.png'))
                assert places_fixture.rows(base,0)[0][1]=='Work shelf' and len(places_fixture.rows(base,1))==1
                assert bytes(base/'Imported') in places_fixture.hidden_bookmarks(base)
                key('Escape');key('q','ctrl');process.wait(timeout=5);process=None
                return
            if '--archives-only' in sys.argv[2:]:
                import archive_fixture
                source=archive_fixture.prepare(base)
                settings.write_text(json.dumps({'preview':True,'grid':False,'font_size':16,'theme':'molten-zharr','icon_mode':'symbols'}))
                process=launch(source)
                def archive_shot(name):
                    path=out/('native-wayland-archive-'+name+'.png')
                    run('grim','-g','0,0 1360x860',str(path))
                    with Image.open(path) as image:return image.copy()
                archive_fixture.exercise(source,key,lambda value:run('wtype','-s','100',value,'-s','100'),click,archive_shot,wait_for)
                key('q','ctrl');process.wait(timeout=5);assert process.returncode==0;process=None
                return
            if '--terminal-only' in sys.argv[2:]:
                import terminal_fixture
                source=terminal_fixture.prepare(base);env['SHELL']='/bin/bash'
                settings.write_text(json.dumps({'preview':False,'grid':False,'font_size':16,'theme':'light','icon_mode':'symbols'}))
                process=launch(source)
                def shell_paste(value):
                    subprocess.run([commands['wl-copy'],'--type','text/plain;charset=utf-8'],input=value.encode(),env=env,check=True)
                    time.sleep(.15);key('v','ctrl','shift');time.sleep(.15)
                def shell_shot(name):
                    path=out/('native-wayland-terminal-'+name+'.png')
                    run('grim','-g','0,0 1360x860',str(path))
                    with Image.open(path) as image:return image.copy()
                terminal_fixture.exercise(source,key,lambda value:run('wtype','-s','100',value,'-s','100'),shell_paste,click,shell_shot,wait_for)
                key('q','ctrl');process.wait(timeout=5);assert process.returncode==0;process=None
                return
            if '--folder-size-only' in sys.argv[2:]:
                import folder_size_fixture
                source=folder_size_fixture.prepare(base)
                settings.write_text(json.dumps({'preview':True,'grid':False,'font_size':16,'theme':'light','icon_mode':'symbols'}))
                process=launch(source)
                def size_shot(name):
                    path=out/('native-wayland-folder-'+name+'.png')
                    run('grim','-g','0,0 1360x860',str(path))
                    with Image.open(path) as image:return image.copy()
                folder_size_fixture.exercise(source,key,click,size_shot,wait_for)
                key('q','ctrl');process.wait(timeout=5);process=None
                return
            if '--syntax-only' in sys.argv[2:]:
                import syntax_fixture
                source=syntax_fixture.prepare(base)
                settings.write_text(json.dumps({'preview':True,'grid':False,'font_size':16,'theme':'light','icon_mode':'symbols'}))
                process=launch(source)
                def syntax_shot(name):
                    path=out/('native-wayland-syntax-'+name+'.png')
                    run('grim','-g','0,0 1360x860',str(path))
                    with Image.open(path) as image:return image.copy()
                syntax_fixture.exercise(source,key,click,syntax_shot,wait_for)
                key('q','ctrl');process.wait(timeout=5);process=None
                return
            if '--media-only' in sys.argv[2:]:
                import media_fixture
                source=media_fixture.prepare(base)
                settings.write_text(json.dumps({'preview':True,'grid':True,'font_size':16,'theme':'light','thumbnails':True}))
                process=launch(source)
                def media_shot(name):
                    path=out/('native-wayland-media-'+name+'.png')
                    run('grim','-g','0,0 1360x860',str(path))
                    with Image.open(path) as image:return image.copy()
                media_fixture.exercise(source,key,click,media_shot,wait_for,settings)
                key('q','ctrl');process.wait(timeout=5);process=None
                return
            if '--recursive-search-only' in sys.argv[2:]:
                import search_fixture
                source=search_fixture.prepare(base);process=launch(source)
                search_fixture.exercise(source,key,lambda value:run('wtype','-s','100',value,'-s','100'),click,
                    lambda name:run('grim','-g','0,0 1360x860',str(out/('native-wayland-'+name+'.png'))),wait_for)
                key('q','ctrl');process.wait(timeout=5);process=None
                return
            if '--batch-rename-only' in sys.argv[2:]:
                import batch_rename_fixture
                source=batch_rename_fixture.prepare(base);process=launch(source)
                batch_rename_fixture.exercise(source,key,
                    lambda value:run('wtype','-s','100',value,'-s','100'),click,
                    lambda name:run('grim','-g','0,0 1360x860',str(out/('native-wayland-'+name+'.png'))),wait_for)
                key('q','ctrl');process.wait(timeout=5);process=None
                print('Native Wayland bulk rename passed',flush=True)
                return
            if '--range-selection-only' in sys.argv[2:]:
                import range_selection_fixture
                source=range_selection_fixture.prepare(base)
                process=launch(source)
                def range_click(cx,cy,*mods):
                    if not mods:
                        click(cx,cy);return
                    args=['-s','100']
                    for mod in mods:args+=['-M',mod]
                    args+=['-s','700']
                    for mod in reversed(mods):args+=['-m',mod]
                    args+=['-s','100']
                    held=subprocess.Popen([commands['wtype'],*args],env=env,stdout=app_log,stderr=app_log)
                    try:
                        time.sleep(.25);click(cx,cy,10)
                        held.wait(timeout=4);assert held.returncode==0
                        click(cx,cy,12)
                    finally:
                        if held.poll() is None:held.terminate();held.wait(timeout=4)
                range_selection_fixture.exercise(base,source,key,navigate,range_click,
                    lambda cx,cy:click(cx,cy,21),
                    lambda name:run('grim','-g','0,0 1360x860',str(out/('native-wayland-'+name+'.png'))),wait_for)
                key('q','ctrl');process.wait(timeout=5);process=None
                print('Native Wayland range selection passed',flush=True)
                return
            if '--text-edit-only' in sys.argv[2:]:
                import text_edit_fixture
                folder=base/'editor';folder.mkdir();(folder/'alpha.txt').write_text('editor data')
                paste_target=base/'paste-target';paste_target.mkdir()
                process=launch(folder)
                def text_drag(sx,sy,ex,ey):
                    click(sx,sy,10);click(ex,ey,11);click(ex,ey,12)
                def put_text(value):
                    owner=subprocess.Popen([commands['wl-copy'],'--foreground','--type','text/plain;charset=utf-8',value],env=env,stdout=subprocess.DEVNULL,stderr=app_log)
                    text_owners.append(owner)
                    wait_for(lambda:run('wl-paste','--no-newline',check=False).stdout==value,'private text clipboard owner')
                text_edit_fixture.exercise(folder,paste_target,key,
                    lambda value:run('wtype','-s','100',value,'-s','100'),
                    put_text,
                    lambda:run('wl-paste','--no-newline',check=False).stdout,
                    click,text_drag,lambda name:run('grim','-g','0,0 1360x860',str(out/('native-wayland-'+name+'.png'))),wait_for)
                key('q','ctrl');process.wait(timeout=5);process=None
                print('Native Wayland text editing: inline rename, mouse range/cut/paste, Unicode, path selection and retained file clipboard passed',flush=True)
                return
            import sqlite3
            storage_files=base/"history-storage";storage_files.mkdir();(storage_files/"a.txt").write_text("keep Wayland source")
            storage_db=base/"state/molten-zharr/basalgrim/operations.sqlite3"
            def storage_rows(sql):
                if not storage_db.exists():return []
                with sqlite3.connect(storage_db) as connection:return connection.execute(sql).fetchall()
            process=launch(storage_files);duplicate=storage_files/"a (1).txt"
            key("F5","ctrl");wait_for(duplicate.exists,"Wayland duplicate for storage");time.sleep(.3)
            key("z","ctrl");wait_for(lambda:not duplicate.exists(),"Wayland retained undo data");time.sleep(.3)
            key("F6");time.sleep(.2);click(22,51);time.sleep(.3)
            click(5,3);wait_for(lambda:json.loads(settings.read_text()).get("history_retention")=="days7","Wayland mouse retention policy")
            for _ in range(3):click(5,3)
            wait_for(lambda:json.loads(settings.read_text()).get("history_retention")=="all","Wayland all-record retention")
            run("grim","-g","0,0 1360x860",str(out/"native-wayland-history-storage.png"))
            click(5,50);key("Escape");assert storage_rows("SELECT state FROM undo_items")==[("undone",)]
            click(5,50);key("Return");wait_for(lambda:storage_rows("SELECT state FROM undo_items")==[("expired",)],"Wayland confirmed storage cleanup")
            assert (storage_files/"a.txt").read_text()=="keep Wayland source" and not duplicate.exists()
            key("q","ctrl");process.wait(timeout=5);process=launch(storage_files)
            key("y","ctrl");time.sleep(.2);assert not duplicate.exists(),"Wayland redo survived cleanup"
            key("F6","ctrl","shift");time.sleep(.3)
            run("grim","-g","0,0 1360x860",str(out/"native-wayland-history-storage-restart.png"))
            key("Escape");key("q","ctrl");process.wait(timeout=5);process=None
            print("Native Wayland history storage: mouse policy, confirmation, cleanup, shortcut and restart passed",flush=True)
            if '--history-storage-only' in sys.argv[2:]:return
            if '--external-dnd-only' in sys.argv[2:]:
                source=base/'external-source';target=base/'external-target';source.mkdir();target.mkdir();(source/'folder').mkdir()
                for name in ('a.txt','b.txt','copy.txt','keep.txt'):(source/name).write_text(name)
                left=launch(source);right=None
                def focus(child):run('swaymsg',f'[pid={child.pid}] focus');time.sleep(.12)
                def arrange(child,x):run('swaymsg',f'[pid={child.pid}] resize set 800 px 860 px, move absolute position {x} px 0 px');time.sleep(.25)
                def hold_ctrl(callback):
                    held=subprocess.Popen([commands['wtype'],'-s','100','-M','ctrl','-s','4000','-m','ctrl'],env=env,stdout=app_log,stderr=app_log)
                    try:time.sleep(.3);callback()
                    finally:held.wait(timeout=6)
                def replay(name,predicate,label):
                    # A path appears before the completion event/history reload reaches the UI.
                    # Retrying while no journal intent exists is safe; once one exists, wait for it.
                    before=storage_rows('SELECT COALESCE(MAX(id),0) FROM operations')[0][0]
                    deadline=time.monotonic()+8
                    while storage_rows('SELECT COALESCE(MAX(id),0) FROM operations')[0][0]==before:
                        key(name,'ctrl')
                        if predicate():break
                        assert time.monotonic()<deadline,label
                    wait_for(predicate,label)
                    wait_for(lambda:not storage_rows("SELECT id FROM operations WHERE state='running'"),'durable replay completion')
                try:
                    arrange(left,0);right=launch(target);arrange(right,800)
                    focus(left);click(34,15);key('c','ctrl')
                    before=run('wl-paste','--no-newline','--type','text/plain;charset=utf-8').stdout
                    click(34,11);hold_ctrl(lambda:(click(34,12),click(34,13)))
                    click(34,13,10);click(145,25,11)
                    run('grim',str(out/'native-wayland-external-dnd.png'));click(145,25,12)
                    wait_for(lambda:all((target/n).exists() for n in ('a.txt','b.txt')),'Wayland native group drop into another window')
                    assert not (source/'a.txt').exists() and not (source/'b.txt').exists()
                    assert run('wl-paste','--no-newline','--type','text/plain;charset=utf-8').stdout==before,'Wayland DnD replaced clipboard'
                    focus(left);hold_ctrl(lambda:(click(34,12,10),click(145,25,11),click(145,25,12)))
                    wait_for(lambda:(target/'copy.txt').exists(),'Wayland external Ctrl-copy');assert (source/'copy.txt').exists()
                    focus(left);click(34,13,10);click(145,25,11);key('Escape');click(105,25,11);click(105,25,12);time.sleep(.3)
                    assert (source/'keep.txt').exists() and not (target/'keep.txt').exists(),'Wayland rejected drop moved a file'
                    focus(right);replay('z',lambda:not (target/'copy.txt').exists(),'Wayland external copy undo')
                    replay('z',lambda:(source/'a.txt').exists() and (source/'b.txt').exists(),'Wayland external move undo')
                    replay('y',lambda:(target/'a.txt').exists() and not (source/'a.txt').exists(),'Wayland external redo')
                    print('Native Wayland external DnD: two windows, group move, Ctrl-copy, rejected drop, unchanged clipboard and undo/redo passed',flush=True)
                    focus(left);key('q','ctrl');left.wait(timeout=5)
                    gtk_source=base/'gtk-source';gtk_target=base/'gtk-target';gtk_source.mkdir();gtk_target.mkdir();(gtk_source/'gtk-input.txt').write_text('GTK Wayland interoperability')
                    peer_env=dict(env,GDK_BACKEND='wayland',GSETTINGS_BACKEND='memory');peer_env.pop('DBUS_SESSION_BUS_ADDRESS',None)
                    peer=subprocess.Popen([sys.executable,str(Path(__file__).with_name('dnd_peer.py')),str(gtk_source),str(gtk_target)],env=peer_env,stdout=app_log,stderr=app_log)
                    try:
                        def peer_rect():
                            node=find(json.loads(run('swaymsg','-t','get_tree').stdout),peer.pid)
                            return node.get('rect') if node else None
                        # Wait for the initial for_window rule before overriding it; GTK maps asynchronously.
                        wait_for(lambda:peer_rect()=={'x':0,'y':0,'width':1360,'height':860},'Wayland GTK initial compositor geometry')
                        arrange(peer,0)
                        wait_for(lambda:peer_rect()=={'x':0,'y':0,'width':800,'height':860},'Wayland GTK test geometry')
                        run('grim',str(out/'native-wayland-gtk-before.png'))
                        (out/'native-wayland-gtk-tree.json').write_text(run('swaymsg','-t','get_tree').stdout)
                        focus(peer);click(50,25,10);click(60,25,11);click(145,25,11)
                        run('grim',str(out/'native-wayland-gtk-drag.png'));click(145,25,12)
                        wait_for(lambda:(target/'gtk-input.txt').exists(),'Wayland GTK -> Basalgrim drop');assert (gtk_source/'gtk-input.txt').exists()
                        wait_for(lambda:'DRAG_FINISHED' in (out/'native-wayland.log').read_text(),'GTK native completion acknowledgement')
                        focus(right);click(134,13,10);click(50,25,11);click(50,25,12)
                        wait_for(lambda:(gtk_target/'gtk-input.txt').exists(),'Wayland Basalgrim -> GTK drop');assert (target/'gtk-input.txt').exists()
                        run('grim',str(out/'native-wayland-external-dnd-gtk.png'))
                        print('Native Wayland GTK4 interoperability: incoming and outgoing file-list Copy passed',flush=True)
                    finally:peer.terminate();peer.wait(timeout=5)
                    focus(right);key('q','ctrl');right.wait(timeout=5)
                finally:
                    for child in (left,right):
                        if child is not None and child.poll() is None:child.terminate();child.wait(timeout=5)
                return
            if '--chrome-only' in sys.argv[2:]:
                parent=base/'up';inside=parent/'one/inside';inside.mkdir(parents=True)
                (inside/'ancestor.txt').write_text('ancestor move');(inside/'peer.txt').write_text('ancestor copy')
                process=launch(inside);session=base/'state/molten-zharr/basalgrim/session.json'
                def snapshot():return json.loads(session.read_text()) if session.exists() else None
                parent_x=25+3+1+5+1+len(base.name)+2+1+1
                click(34,11,10);click(parent_x,8,11)
                run('grim','-g','0,0 1360x860',str(out/'native-wayland-breadcrumb-drop.png'));click(parent_x,8,12)
                wait_for(lambda:(parent/'ancestor.txt').exists(),'Wayland breadcrumb move several levels up');assert not (inside/'ancestor.txt').exists()
                held=subprocess.Popen([commands['wtype'],'-s','100','-M','ctrl','-s','2500','-m','ctrl'],env=env,stdout=app_log,stderr=app_log)
                try:time.sleep(.3);click(34,11,10);click(parent_x,8,11);click(parent_x,8,12)
                finally:held.wait(timeout=5)
                wait_for(lambda:(parent/'peer.txt').exists(),'Wayland Ctrl breadcrumb copy');assert (inside/'peer.txt').exists()
                wait_for(lambda:snapshot() and snapshot()['panes'][0]['tabs']==[str(inside)],'breadcrumb drop preserves view')
                click(37,6);wait_for(lambda:len(snapshot()['panes'][0]['tabs'])==2,'Wayland left plus')
                click(155,5);wait_for(lambda:json.loads(settings.read_text()).get('dual') is True,'Wayland second-pane header')
                click(109,6);wait_for(lambda:len(snapshot()['panes'][1]['tabs'])==2,'Wayland inactive pane plus')
                assert len(snapshot()['panes'][0]['tabs'])==2
                click(167,5);wait_for(lambda:json.loads(settings.read_text()).get('dual') is False,'Wayland second-pane close')
                click(155,5);wait_for(lambda:json.loads(settings.read_text()).get('dual') is True,'Wayland second-pane restore');assert len(snapshot()['panes'][1]['tabs'])==2
                run('grim','-g','0,0 1360x860',str(out/'native-wayland-pane-tabs.png'))
                run('grim','-g','0,0 1360x860',str(out/'native-wayland-tab-close.png'))
                click(47,6);wait_for(lambda:len(snapshot()['panes'][0]['tabs'])==1,'Wayland unfocused left tab close cross')
                assert len(snapshot()['panes'][1]['tabs'])==2
                click(119,6);wait_for(lambda:len(snapshot()['panes'][1]['tabs'])==1,'Wayland right tab close cross')
                assert snapshot()['panes'][0]['tabs']==[str(inside)] and snapshot()['panes'][1]['tabs']==[str(inside)]
                key('q','ctrl');process.wait(timeout=5);process=None
                print('Native Wayland chrome: breadcrumbs, pane-scoped plus, second-pane toggle, retained tabs and pane-scoped tab close crosses passed',flush=True)
                return
            drag_root=base/'drag-native';drag_root.mkdir();(drag_root/'folder').mkdir()
            for name in ('a.txt','b.txt','c.txt'):(drag_root/name).write_text(name)
            process=launch(drag_root)
            def control_mouse(callback):
                held=subprocess.Popen([commands['wtype'],'-s','100','-M','ctrl','-s','2500','-m','ctrl'],env=env,stdout=app_log,stderr=app_log)
                try:time.sleep(.3);callback()
                finally:held.wait(timeout=5)
            control_mouse(lambda:(click(34,12),click(34,13)))
            click(34,13,10);click(34,11,11)
            run('grim','-g','0,0 1360x860',str(out/'native-wayland-drag-group.png'));click(34,11,12)
            wait_for(lambda:all((drag_root/'folder'/n).exists() for n in ('a.txt','b.txt')),'Wayland marked group drag')
            assert not (drag_root/'a.txt').exists() and not (drag_root/'b.txt').exists();time.sleep(.25)
            control_mouse(lambda:(click(34,12,10),click(34,11,11),click(34,11,12)))
            wait_for(lambda:(drag_root/'folder/c.txt').exists(),'Wayland Ctrl drag copy');assert (drag_root/'c.txt').exists()
            click(34,12,10);click(34,11,11);key('Escape');click(34,11,12);assert (drag_root/'c.txt').exists()
            tab_dest=base/'drop-target';tab_dest.mkdir();key('t','ctrl');navigate(tab_dest);click(28,6)
            click(34,12,10);click(43,6,11);click(43,6,12)
            wait_for(lambda:(tab_dest/'c.txt').exists(),'Wayland inactive tab drop');assert not (drag_root/'c.txt').exists()
            time.sleep(.25);key('z','ctrl');wait_for(lambda:(drag_root/'c.txt').exists(),'Wayland drag undo')
            time.sleep(.25);key('y','ctrl');wait_for(lambda:not (drag_root/'c.txt').exists(),'Wayland drag redo')
            (drag_root/'pane.txt').write_text('pane drag');time.sleep(.5);key('d','ctrl');click(110,25)
            pane_dest=base/'drop-pane';pane_dest.mkdir();navigate(pane_dest)
            click(34,12,10);click(110,25,11);click(110,25,12)
            wait_for(lambda:(pane_dest/'pane.txt').exists(),'Wayland other pane drop')
            key('q','ctrl');process.wait(timeout=5)
            settings.write_text(json.dumps({'preview':False,'grid':True,'font_size':16,'theme':'light'}))
            grid_drag=base/'drag-grid';grid_drag.mkdir();(grid_drag/'folder').mkdir();(grid_drag/'image.txt').write_text('grid drag')
            process=launch(grid_drag);click(54,12,10);click(34,12,11)
            run('grim','-g','0,0 1360x860',str(out/'native-wayland-drag-grid.png'));click(34,12,12)
            wait_for(lambda:(grid_drag/'folder/image.txt').exists(),'Wayland grid drag')
            key('q','ctrl');process.wait(timeout=5);process=None
            print('Native Wayland drag/drop: group move, Ctrl copy, Escape, tabs, panes, undo/redo and grid passed',flush=True)
            settings.write_text(json.dumps({'preview':False,'grid':False,'font_size':16,'theme':'light'}))
            if '--drag-drop-only' in sys.argv[2:]:return
            files = base / "files"
            source, destination = files / "source", files / "destination"
            source.mkdir(parents=True); destination.mkdir()
            (source / "alpha.txt").write_text("native Wayland keyboard")
            assert run("wl-paste", "--list-types", check=False).returncode != 0
            process = launch(source)
            key("c", "ctrl")
            wait_for(lambda:run("wl-paste","--list-types",check=False).returncode==0,"Wayland file clipboard owner")
            assert run("wl-paste","--no-newline","--type","x-special/gnome-copied-files").stdout=='copy\n'+(source/'alpha.txt').as_uri()
            assert run("wl-paste","--no-newline","--type","application/x-kde-cutselection").stdout=='0'
            navigate(destination); key("v", "ctrl")
            run("grim", "-g", "0,0 1360x860", str(out / "native-wayland-before-check.png"))
            wait_for(lambda: (destination / "alpha.txt").exists(), "Wayland Ctrl+C/V with empty OS clipboard")
            key("x", "ctrl"); navigate(files); key("v", "ctrl")
            wait_for(lambda: (files / "alpha.txt").exists(), "Wayland Ctrl+X/V")
            assert not (destination / "alpha.txt").exists()
            owner = subprocess.Popen([commands["wl-copy"], "--foreground", "pasted.txt"], env=env,
                                     stdout=subprocess.DEVNULL, stderr=app_log)
            try:
                wait_for(lambda: run("wl-paste", "--list-types", check=False).returncode == 0, "Wayland text clipboard")
                key("n", "ctrl"); key("v", "ctrl"); key("Return")
                wait_for(lambda: (files / "pasted.txt").exists(), "Wayland system text paste")
            finally:
                owner.terminate(); owner.wait(timeout=5)
            # A native text copy must keep the internal file selection accessible through its marker.
            key('f','ctrl');run('wtype','-s','100','pasted.txt','-s','100');key('Return');key('c','ctrl');click(165,8)
            assert run('wl-paste','--no-newline').stdout==str(files)
            navigate(source);key('v','ctrl')
            wait_for(lambda:(source/'pasted.txt').exists(),'Wayland files survive copying address text')
            time.sleep(.2);navigate(files)
            external=base/'external';external.mkdir()
            for cut in (False,True):
                name='external-cut.txt' if cut else 'Картинка #1.txt'
                item=external/name;item.write_text('external Wayland clipboard')
                owner=subprocess.Popen([commands['wl-copy'],'--foreground','--type','x-special/gnome-copied-files',('cut' if cut else 'copy')+'\n'+item.as_uri()],env=env,stdout=subprocess.DEVNULL,stderr=app_log)
                wait_for(lambda:'x-special/gnome-copied-files' in run('wl-paste','--list-types',check=False).stdout,'external file offer')
                try:
                    if cut:key('v','ctrl')
                    else:click(34,22,3);click(36,25)
                    wait_for(lambda:(files/name).exists(),'Wayland external file clipboard import')
                    assert item.exists()!=cut
                    time.sleep(.2)
                finally:owner.terminate();owner.wait(timeout=5)
            run("grim", "-g", "0,0 1360x860", str(out / "native-wayland-keyboard.png"))
            key("q", "ctrl"); process.wait(timeout=5); assert process.returncode == 0
            print("Native Wayland: Ctrl+C/X/V, empty system clipboard and editor text paste passed", flush=True)
            if "--clipboard-only" in sys.argv[2:]:
                isolated=dict(env,BASALGRIM_CLIPBOARD_TEST_ISOLATED='1')
                subprocess.run([sys.executable,'scripts/smoke_file_clipboard_pty.py',str(binary.with_name('basalgrim-tui'))],env=isolated,check=True,timeout=45)
                peer=os.environ.get('BASALGRIM_NATIVE_CLIPBOARD_TEST')
                if peer:subprocess.run([peer,'--ignored','--nocapture'],env=isolated,check=True,timeout=30)
                return
            mouse = base / "mouse"
            (mouse / "destination").mkdir(parents=True); (mouse / "moved").mkdir()
            (mouse / "alpha.txt").write_text("native Wayland mouse")
            process = launch(mouse)
            click(34, 13, 3); click(36, 15)  # alpha -> Copy
            click(34, 11); click(34, 11)  # destination
            click(34, 22, 3); click(36, 25)  # background -> Paste
            wait_for(lambda: (mouse / "destination/alpha.txt").exists(), "Wayland mouse Copy/Paste")
            click(34, 11, 3); click(36, 14)  # Cut
            click(21, 3)  # Parent
            click(34, 12); click(34, 12)  # moved
            click(34, 22, 3); click(36, 25)  # Paste
            wait_for(lambda: (mouse / "moved/alpha.txt").exists(), "Wayland mouse Cut/Paste")
            assert not (mouse / "destination/alpha.txt").exists()
            run("grim", "-g", "0,0 1360x860", str(out / "native-wayland-mouse.png"))
            # Hold the primary button across motion events in the file list.
            key("l", "ctrl"); key("u", "ctrl"); run("wtype", "-s", "100", str(mouse), "-s", "100"); key("Return")
            # Folder rows 11/12, file row 13: a rectangle selects both folders.
            click(168,12,10); click(26,11,11)
            run("grim", "-g", "0,0 1360x860", str(out / "native-wayland-selection.png"))
            click(26,11,12)
            destination = base / "wayland-selection"; destination.mkdir()
            key("c", "ctrl"); navigate(destination); key("v", "ctrl")
            wait_for(lambda: (destination / "destination").is_dir() and (destination / "moved/alpha.txt").exists(), "Wayland rectangle selects exactly both folders")
            assert not (destination / "alpha.txt").exists()
            key("q", "ctrl"); process.wait(timeout=5); assert process.returncode == 0
            print("Native Wayland: context-menu copy/cut/paste, double click and rubber-band folder selection passed", flush=True)
            files=base/"wheel"; files.mkdir()
            for index in range(100): (files/f"file-{index:03}.txt").write_text(str(index))
            process=launch(files)
            for _ in range(3): click(50,20,21)
            run("grim", "-g", "208,176 800x16", str(out/"native-wayland-watch-before.png"))
            (files/"file--new.txt").write_text("external")
            time.sleep(1)
            run("grim", "-g", "208,176 800x16", str(out/"native-wayland-watch-after.png"))
            assert (out/"native-wayland-watch-before.png").read_bytes()==(out/"native-wayland-watch-after.png").read_bytes(), "Wayland auto refresh moved the viewport"
            protected=base/"wheel-protected"; protected.mkdir()
            key("c","ctrl"); navigate(protected); key("v","ctrl")
            wait_for(lambda:(protected/"file-000.txt").exists(),"Wayland wheel preserves keyboard selection")
            assert sorted(p.name for p in protected.iterdir())==["file-000.txt"]
            navigate(files)
            for _ in range(3): click(50,20,21)
            click(34,11)
            clicked=base/"wheel-clicked"; clicked.mkdir()
            key("c","ctrl"); navigate(clicked); key("v","ctrl")
            # One externally inserted entry now sorts before file-000.
            wait_for(lambda:(clicked/"file-008.txt").exists(),"Wayland auto refresh updates entries and wheel hit regions")
            key("q","ctrl"); process.wait(timeout=5); assert process.returncode==0
            print("Native Wayland: wheel scrolls content, preserves selection and updates visible hit regions passed",flush=True)
            print("Native Wayland: external create preserves viewport pixels and selected path passed",flush=True)
            cross_source=base/"cross-source"; cross_source.mkdir()
            (cross_source/"folder").mkdir(); (cross_source/"folder/data").write_text("folder payload")
            (cross_source/"payload.txt").write_text("file payload")
            with tempfile.TemporaryDirectory(prefix="basalgrim-wayland-move-",dir="/dev/shm") as remote:
                cross_destination=Path(remote)
                assert cross_source.stat().st_dev!=cross_destination.stat().st_dev
                process=launch(cross_source)
                key("a","ctrl"); key("x","ctrl"); navigate(cross_destination); key("v","ctrl")
                wait_for(lambda:not list(cross_source.iterdir()) and (cross_destination/"payload.txt").exists(),"Wayland cross-device cut/paste")
                assert (cross_destination/"folder/data").read_text()=="folder payload"
                assert (cross_destination/"payload.txt").read_text()=="file payload"
                key("q","ctrl"); process.wait(timeout=5); assert process.returncode==0
                print("Native Wayland: cross-device file/directory move passed",flush=True)
            conflict_source=base/"conflict-source"; conflict_source.mkdir()
            conflict_dest=base/"conflict-destination"; conflict_dest.mkdir()
            for name in ("a.txt","b.txt"):
                (conflict_source/name).write_text("incoming "+name);(conflict_dest/name).write_text("existing "+name)
            process=launch(conflict_source)
            key("a","ctrl");key("c","ctrl");navigate(conflict_dest);key("v","ctrl");time.sleep(.6)
            click(55,26);click(55,30)
            wait_for(lambda:all((conflict_dest/n).exists() for n in ("a (1).txt","b (1).txt")),"Wayland keep both for remaining conflicts")
            for name in ("a.txt","b.txt"):assert (conflict_dest/name).read_text()=="existing "+name
            key("v","ctrl");time.sleep(.6);click(55,26);click(55,28)
            wait_for(lambda:all((conflict_dest/n).read_text()=="incoming "+n for n in ("a.txt","b.txt")),"Wayland replace for remaining conflicts")
            key("v","ctrl");time.sleep(.6);key("Escape");time.sleep(.3)
            key("q","ctrl");process.wait(timeout=5);assert process.returncode==0
            print("Native Wayland conflicts: mouse Keep Both/Replace for remaining and Escape passed",flush=True)
            pane_source=base/"pane-source"; pane_source.mkdir()
            pane_dest=base/"pane-destination"; pane_dest.mkdir()
            for name in ("a.txt","b.txt"): (pane_source/name).write_text("incoming "+name)
            settings=base/"config/molten-zharr/basalgrim/settings.json"
            preferences=json.loads(settings.read_text()); preferences.update(dual=True,grid=False,preview=False)
            settings.write_text(json.dumps(preferences))
            process=launch(pane_source)
            key("Tab");navigate(pane_dest);key("Tab");key("a","ctrl")
            key("F5","shift")
            wait_for(lambda:all((pane_dest/n).exists() for n in ("a.txt","b.txt")),"Wayland copy to other pane")
            time.sleep(.3);click(34,11,3);click(36,14)
            wait_for(lambda:all((pane_source/n).exists() for n in ("a (1).txt","b (1).txt")),"Wayland mouse duplicate")
            key("F6","shift");time.sleep(.5);key("space");key("Up");key("Return");time.sleep(.3)
            for name in ("a.txt","b.txt"): assert (pane_source/name).exists();(pane_dest/name).unlink()
            time.sleep(.4);click(34,12,3);click(36,17)
            wait_for(lambda:all(not (pane_source/n).exists() and (pane_dest/n).exists() for n in ("a.txt","b.txt")),"Wayland mouse move to other pane")
            key("Home");key("F5","ctrl")
            wait_for(lambda:(pane_source/"a (1) (1).txt").exists(),"Wayland duplicate shortcut")
            key("q","ctrl");process.wait(timeout=5);assert process.returncode==0
            print("Native Wayland panes: group copy/move, conflicts, mouse duplicate and shortcuts passed",flush=True)
            session_dirs=[base/f"session-{i}" for i in range(4)]
            for directory in session_dirs:directory.mkdir();(directory/"marker.txt").write_text(directory.name)
            process=launch(session_dirs[0]);key("t","ctrl");navigate(session_dirs[1]);key("Tab");navigate(session_dirs[2]);key("t","ctrl");navigate(session_dirs[3]);key("q","ctrl");process.wait(timeout=5)
            state=base/"state/molten-zharr/basalgrim/session.json"
            saved=json.loads(state.read_text());assert saved["active"]==1
            assert [p["tabs"] for p in saved["panes"]]==[[str(session_dirs[0]),str(session_dirs[1])],[str(session_dirs[2]),str(session_dirs[3])]]
            process=launch(None);key("c","ctrl");navigate(session_dirs[0]);key("v","ctrl");time.sleep(.5);key("Return")
            wait_for(lambda:(session_dirs[0]/"marker (1).txt").exists(),"Wayland restored active pane")
            assert (session_dirs[0]/"marker (1).txt").read_text()=="session-3"
            key("F6");time.sleep(.6);run("grim","-g","0,16 1360x144",str(out/"native-wayland-journal-before.png"));key("Escape")
            key("t","ctrl");run("swaymsg",f"[pid={process.pid}] kill");process.wait(timeout=5);assert process.returncode==0
            saved=json.loads(state.read_text());assert len(saved["panes"][1]["tabs"])==3 and saved["panes"][1]["active"]==2
            print("Native Wayland session: both panes/tabs, restart, active-pane file commands and window close passed",flush=True)
            process=launch(None);key("F6");time.sleep(.6)
            run("grim","-g","0,16 1360x144",str(out/"native-wayland-journal-after.png"))
            assert (out/"native-wayland-journal-before.png").read_bytes()==(out/"native-wayland-journal-after.png").read_bytes(),"Wayland journal changed across restart"
            click(4,1);click(36,17);time.sleep(.6);run("grim","-g","0,16 1360x144",str(out/"native-wayland-journal-refreshed.png"))
            assert (out/"native-wayland-journal-after.png").read_bytes()==(out/"native-wayland-journal-refreshed.png").read_bytes(),"Wayland journal refresh changed history"
            key("q","ctrl");process.wait(timeout=5);assert process.returncode==0
            print("Native Wayland journal: persistent history and mouse refresh passed",flush=True)
            preview_dir=base/"image-preview";preview_fixture.create(preview_dir)
            preferences=json.loads(settings.read_text());preferences.update(dual=False,grid=False,preview=True,icon_mode="symbols")
            settings.write_text(json.dumps(preferences))
            process=launch(preview_dir)
            def preview_shot(name):
                path=out/f"native-wayland-preview-{name}.png"
                run("grim","-g","0,0 1360x860",str(path))
                with Image.open(path) as image:
                    return image.copy()
            wait_for(lambda:preview_fixture.bounds(preview_shot("initial"),preview_fixture.RED),"Wayland image preview")
            preview_fixture.assert_aspect(preview_shot("initial"),preview_fixture.RED)
            preview_fixture.create(preview_dir,preview_fixture.BLUE)
            wait_for(lambda:preview_fixture.bounds(preview_shot("rewrite"),preview_fixture.BLUE),"Wayland same-path image rewrite")
            preview_fixture.assert_aspect(preview_shot("rewrite"),preview_fixture.BLUE)
            click(151,29);key("F10","shift");time.sleep(.3)
            assert not preview_fixture.bounds(preview_shot("modal"),preview_fixture.BLUE),"Wayland image covered a menu"
            click(2,52)
            wait_for(lambda:preview_fixture.bounds(preview_shot("dismiss"),preview_fixture.BLUE),"Wayland mouse modal dismissal")
            key("F3");time.sleep(.2)
            assert not preview_fixture.bounds(preview_shot("off"),preview_fixture.BLUE),"Wayland image remained after F3"
            key("F3");run("swaymsg",f"[pid={process.pid}] resize set 800 px 600 px");time.sleep(.3)
            run("swaymsg",f"[pid={process.pid}] resize set 1360 px 860 px")
            wait_for(lambda:preview_fixture.bounds(preview_shot("resized"),preview_fixture.BLUE),"Wayland image after resize")
            key("Down");time.sleep(.3)
            assert not preview_fixture.bounds(preview_shot("text"),preview_fixture.BLUE),"Wayland cached image over text"
            key("q","ctrl");process.wait(timeout=5);assert process.returncode==0
            print("Native Wayland preview: aspect, watched rewrite, menu, F3, resize and text passed",flush=True)
            undo_dir=base/"undo";undo_dir.mkdir();(undo_dir/"a.txt").write_text("undo source");copied=undo_dir/"a (1).txt"
            preferences=json.loads(settings.read_text());preferences.update(dual=False,grid=False,preview=False)
            settings.write_text(json.dumps(preferences));process=launch(undo_dir)
            key("F5","ctrl");wait_for(copied.exists,"Wayland duplicate for undo");time.sleep(.35)
            def idle_file_view():
                run("grim", "-g", "0,0 1360x860", str(out / "native-wayland-idle.png"))
                with Image.open(out / "native-wayland-idle.png") as image:
                    # Busy operations draw the Stop control at the right of the status row.
                    return len(set(image.crop((1160,832,1350,848)).getdata())) == 1
            key("z","ctrl");wait_for(lambda:not copied.exists(),"Wayland Ctrl+Z")
            wait_for(idle_file_view,"Wayland undo result acknowledged by UI")
            key("y","ctrl");wait_for(copied.exists,"Wayland Ctrl+Y");time.sleep(.2)
            key("F6");time.sleep(.3);click(4,50);wait_for(lambda:not copied.exists(),"Wayland F6 mouse undo");time.sleep(.2)
            click(22,50);wait_for(copied.exists,"Wayland F6 mouse redo");time.sleep(.2)
            click(4,50);wait_for(lambda:not copied.exists(),"Wayland undo before restart");time.sleep(.2)
            key("q","ctrl");process.wait(timeout=5);process=launch(undo_dir)
            copied.write_text("external destination");time.sleep(.3);key("y","ctrl");time.sleep(.4)
            assert copied.read_text()=="external destination","Wayland redo overwrote a conflict"
            copied.unlink();time.sleep(.2);key("y","ctrl");wait_for(copied.exists,"Wayland redo after restart");time.sleep(.2)
            assert copied.read_text()=="undo source"
            copied.write_text("external edit");time.sleep(.3);key("z","ctrl");time.sleep(.4)
            assert copied.read_text()=="external edit","Wayland undo hid an edited file"
            key("q","ctrl");process.wait(timeout=5);assert process.returncode==0
            print("Native Wayland undo: shortcuts, F6 mouse, restart, conflicts and edited-file guard passed",flush=True)
            path_parent=base/"path parent";path_dir=path_parent/"Кириллица spaces";path_dir.mkdir(parents=True)
            settings.write_text(json.dumps({"preview":False,"grid":False,"font_size":16,"theme":"light"}))
            process=launch(path_dir)
            def copied_path():return run("wl-paste","--no-newline",check=False).stdout
            click(165,8);wait_for(lambda:copied_path()==str(path_dir),"Wayland copy full Unicode path")
            current_x=25+sum(len(part)+3 for part in path_dir.parts[:-1])+1
            click(current_x,8);key("c","ctrl");assert copied_path()==str(path_dir);key("Escape")
            click(current_x,8);run("wtype","-s","100","/tmp/abcdef","-s","100");key("End")
            run("grim","-g","200,128 1000x16",str(out/"native-wayland-caret-end.png"))
            key("Left");run("grim","-g","200,128 1000x16",str(out/"native-wayland-caret-left.png"))
            with Image.open(out/"native-wayland-caret-end.png") as end_caret, Image.open(out/"native-wayland-caret-left.png") as left_caret:
                assert end_caret.tobytes()!=left_caret.tobytes(),"Wayland caret did not move on unchanged text"
            run("wtype","x");key("c","ctrl");wait_for(lambda:copied_path()=="/tmp/abcdexf","Wayland insertion follows visible caret")
            click(32,8);run("wtype","y");key("c","ctrl");wait_for(lambda:copied_path()=="/tmp/abycdexf","Wayland mouse positions caret");key("Escape")
            key("l","ctrl");key("c","ctrl");assert copied_path()==str(path_dir);key("Escape")
            parent_x=25+sum(len(part)+3 for part in path_parent.parts[:-1])+1
            click(parent_x,8);click(165,8);wait_for(lambda:copied_path()==str(path_parent),"Wayland ancestor breadcrumb")
            click(26,8);click(165,8);wait_for(lambda:copied_path()=="/","Wayland root breadcrumb")
            click(26,8);run("wtype","-s","100",str(path_parent),"-s","100");key("Return");time.sleep(.2)
            click(165,8);wait_for(lambda:copied_path()==str(path_parent),"Wayland mouse path editor replaces selection")
            key("q","ctrl");process.wait(timeout=5);assert process.returncode==0
            print("Native Wayland paths: Unicode clipboard, Ctrl+L/C, ancestors, root and mouse editor passed",flush=True)
            trash_dir=base/"trash-native";trash_dir.mkdir();a=trash_dir/"a.txt";tree=trash_dir/"b-tree"
            a.write_text("native restore");tree.mkdir();(tree/"nested").write_text("native tree")
            subprocess.run(["gio","trash","--",str(a),str(tree)],env=env,check=True)
            settings.write_text(json.dumps({"preview":False,"grid":False,"font_size":16,"theme":"light"}));process=launch(trash_dir)
            key("b","ctrl","shift");time.sleep(.4);key("f","ctrl");run("wtype","-s","100",str(trash_dir),"-s","100");key("Return");time.sleep(.3)
            key("a","ctrl");click(34,11,3);click(36,12);click(57,29)
            wait_for(lambda:a.exists() and tree.exists(),"Wayland trash group via context menu");time.sleep(.3)
            assert a.read_text()=="native restore" and (tree/"nested").read_text()=="native tree"
            key("z","ctrl");wait_for(lambda:not a.exists() and not tree.exists(),"Wayland undo restoration");time.sleep(.3)
            key("y","ctrl");wait_for(lambda:a.exists() and tree.exists(),"Wayland redo restoration");time.sleep(.3)
            subprocess.run(["gio","trash","--",str(a)],env=env,check=True);a.write_text("occupied");key("r","ctrl");time.sleep(.4)
            click(34,11);click(34,9);click(57,29);time.sleep(.3);key("Return")
            restored=trash_dir/"a (1).txt";wait_for(restored.exists,"Wayland Keep Both restoration");time.sleep(.3)
            assert restored.read_text()=="native restore" and a.read_text()=="occupied"
            key("q","ctrl");process.wait(timeout=5)
            subprocess.run(["gio","trash","--",str(tree)],env=env,check=True)
            destination=base/"trash-destination";destination.mkdir();process=launch(destination)
            key("b","ctrl","shift");time.sleep(.4);key("f","ctrl");run("wtype","-s","100",str(trash_dir),"-s","100");key("Return");time.sleep(.3)
            click(34,11,3);click(36,13);click(57,29);wait_for(lambda:(destination/"b-tree/nested").exists(),"Wayland restore to current directory after restart");time.sleep(.3)
            assert (destination/"b-tree/nested").read_text()=="native tree" and not tree.exists()
            purge_a=destination/"delete-a.txt";purge_b=destination/"delete-b.txt"
            purge_a.write_text("selected deletion");purge_b.write_text("hidden deletion")
            subprocess.run(["gio","trash","--",str(purge_a),str(purge_b)],env=env,check=True)
            key("f","ctrl");key("u","ctrl");run("wtype","delete-");key("Return");key("r","ctrl");time.sleep(.4)
            click(34,11,3);click(36,14);key("Escape")
            trash_files=base/"data/Trash/files";trash_info=base/"data/Trash/info"
            assert (trash_files/purge_a.name).exists() and (trash_files/purge_b.name).exists()
            key("Delete");click(57,29);wait_for(lambda:not (trash_files/purge_a.name).exists(),"Wayland selected permanent deletion");time.sleep(.3)
            assert (trash_files/purge_b.name).exists()
            key("f","ctrl");key("u","ctrl");run("wtype","no-matching-file");key("Return")
            click(34,3);time.sleep(.4);click(57,29)
            wait_for(lambda:not any(trash_files.iterdir()) and not any(trash_info.iterdir()),"Wayland whole-trash empty with hidden entries");time.sleep(.3)
            assert (destination/"b-tree/nested").read_text()=="native tree"
            key("q","ctrl");process.wait(timeout=5);assert process.returncode==0
            print("Native Wayland trash: gio group, context mouse, undo/redo, conflicts, restart and current-folder restore passed",flush=True)
            print("Verified binary:", binary, hashlib.sha256(binary.read_bytes()).hexdigest())
        except Exception:
            if process and process.poll() is None:
                run("grim", "-g", "0,0 1360x860", str(out / "native-wayland-failure.png"), check=False)
                (out / "native-wayland-tree.json").write_text(run("swaymsg", "-t", "get_tree").stdout)
            raise
        finally:
            for owner in text_owners:
                if owner.poll() is None:owner.terminate();owner.wait(timeout=5)
            if process and process.poll() is None:
                process.terminate(); process.wait(timeout=5)
            if pointer:
                pointer.terminate(); pointer.wait(timeout=5)
            if keyboard:
                keyboard.terminate(); keyboard.wait(timeout=5)
            server.terminate(); server.wait(timeout=5)
            app_log.close(); server_log.close()


if __name__ == "__main__":
    main()
