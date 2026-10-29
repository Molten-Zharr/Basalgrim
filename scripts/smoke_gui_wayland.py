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
                   LIBGL_ALWAYS_SOFTWARE="1", XDG_CONFIG_HOME=str(base / "config"),
                   XDG_DATA_HOME=str(base / "data"))
        for name in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "SWAYSOCK"):
            env.pop(name, None)
        server_log = open(out / "native-wayland-server.log", "w")
        server = subprocess.Popen([commands["sway"], "--unsupported-gpu", "-c", str(config)],
                                  env=env, stdout=server_log, stderr=server_log)
        process = None
        pointer = None
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
            child = subprocess.Popen([str(binary), str(path)], env=env, stdout=app_log, stderr=app_log)
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
            settings = base / "config/molten-zharr/basalgrim/settings.json"
            settings.parent.mkdir(parents=True)
            settings.write_text(json.dumps({"preview": False, "grid": False, "font_size": 16, "theme": "light"}))
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
            protected=base/"wheel-protected"; protected.mkdir()
            key("c","ctrl"); navigate(protected); key("v","ctrl")
            wait_for(lambda:(protected/"file-000.txt").exists(),"Wayland wheel preserves keyboard selection")
            assert sorted(p.name for p in protected.iterdir())==["file-000.txt"]
            navigate(files)
            for _ in range(3): click(50,20,21)
            click(34,11)
            clicked=base/"wheel-clicked"; clicked.mkdir()
            key("c","ctrl"); navigate(clicked); key("v","ctrl")
            wait_for(lambda:(clicked/"file-009.txt").exists(),"Wayland wheel moves the actual viewport")
            key("q","ctrl"); process.wait(timeout=5); assert process.returncode==0
            print("Native Wayland: wheel scrolls content, preserves selection and updates visible hit regions passed",flush=True)
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
            server.terminate(); server.wait(timeout=5)
            app_log.close(); server_log.close()


if __name__ == "__main__":
    main()
