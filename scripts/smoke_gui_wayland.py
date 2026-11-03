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
            files = base / "files"
            source, destination = files / "source", files / "destination"
            source.mkdir(parents=True); destination.mkdir()
            (source / "alpha.txt").write_text("native Wayland keyboard")
            assert run("wl-paste", "--list-types", check=False).returncode != 0
            process = launch(source)
            key("c", "ctrl"); navigate(destination); key("v", "ctrl")
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
            run("grim", "-g", "0,0 1360x860", str(out / "native-wayland-keyboard.png"))
            key("q", "ctrl"); process.wait(timeout=5); assert process.returncode == 0
            print("Native Wayland: Ctrl+C/X/V, empty system clipboard and editor text paste passed", flush=True)
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
            click(75,12,10); click(26,11,11)
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
