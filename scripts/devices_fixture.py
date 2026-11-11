"""Independent UDisks wire peer on a private bus. Never uses host block devices."""
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import time

ROOT = "/org/freedesktop/UDisks2"
DRIVE = ROOT + "/drives/Test_USB"
BLOCK = ROOT + "/block_devices/test"


class Fixture:
    def __init__(self, base):
        self.base = base
        self.root = base / "source"
        self.root.mkdir(exist_ok=True)
        (self.root / "local.txt").write_text("local file\n")
        for i in (1, 2):
            folder = base / f"USB{i}"
            folder.mkdir(exist_ok=True)
            (folder / f"device-{i}.txt").write_text("test device, ordinary folder\n")
        self.state = base / "devices.json"
        self.log = base / "device-calls.jsonl"
        self.update(present=True, mounted=[False, True], detected=42, error="", raw=False)
        self.bus = subprocess.Popen(["dbus-daemon", "--session", "--nofork", "--print-address=1"],
                                    stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        assert select.select([self.bus.stdout], [], [], 5)[0], "private bus startup"
        self.address = self.bus.stdout.readline().strip()
        self.peer = subprocess.Popen(["/usr/bin/python3", __file__, "--serve", str(base), self.address],
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        assert select.select([self.peer.stdout], [], [], 5)[0], "UDisks peer startup"
        assert self.peer.stdout.readline().strip() == "ready", self.peer.stderr.read()

    def update(self, **fields):
        state = json.loads(self.state.read_text()) if self.state.exists() else {}
        state.update(fields)
        temp = self.state.with_suffix(".tmp")
        temp.write_text(json.dumps(state))
        temp.replace(self.state)

    def calls(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def close(self):
        for child in (self.peer, self.bus):
            if child.poll() is None:
                child.terminate()
            child.wait(timeout=5)

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()


def serve(base, address):
    import gi
    gi.require_version("Gio", "2.0")
    from gi.repository import Gio, GLib

    conn = Gio.DBusConnection.new_for_address_sync(address,
        Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION, None, None)
    conn.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "RequestName",
                   GLib.Variant("(su)", ("org.freedesktop.UDisks2", 0)), None, Gio.DBusCallFlags.NONE, 2000, None)
    xml = '''<node>
      <interface name="org.freedesktop.DBus.ObjectManager">
        <method name="GetManagedObjects"><arg direction="out" type="a{oa{sa{sv}}}"/></method>
      </interface>
      <interface name="org.freedesktop.UDisks2.Filesystem">
        <method name="Mount"><arg direction="in" type="a{sv}"/><arg direction="out" type="s"/></method>
        <method name="Unmount"><arg direction="in" type="a{sv}"/></method>
      </interface>
      <interface name="org.freedesktop.UDisks2.Drive">
        <method name="PowerOff"><arg direction="in" type="a{sv}"/></method>
        <method name="Eject"><arg direction="in" type="a{sv}"/></method>
      </interface>
    </node>'''
    node = Gio.DBusNodeInfo.new_for_xml(xml)
    state_path = base / "devices.json"

    def mount_path(i, state):
        raw = os.fsencode(base / f"USB{i}")
        return raw + (b"-\xff" if state.get("raw") else b"")

    def objects(state):
        if not state["present"]:
            return {}
        v = GLib.Variant
        result = {DRIVE: {"org.freedesktop.UDisks2.Drive": {
            "Id": v("s", "test-usb-serial"), "TimeDetected": v("t", state["detected"]),
            "TimeMediaDetected": v("t", state["detected"]), "Model": v("s", "Test USB"), "Vendor": v("s", "Basalgrim"),
            "Removable": v("b", True), "MediaAvailable": v("b", True), "CanPowerOff": v("b", True),
            "Ejectable": v("b", False), "SiblingId": v("s", "")}}}
        for i in (1, 2):
            result[BLOCK + str(i)] = {
                "org.freedesktop.UDisks2.Block": {
                    "Drive": v("o", DRIVE), "Device": v("ay", b"/dev/basalgrim-test" + str(i).encode() + b"\0"),
                    "CryptoBackingDevice": v("o", "/"), "DeviceNumber": v("t", 9000 + i),
                    "IdUUID": v("s", f"test-uuid-{i}"), "IdLabel": v("s", f"USB {i}"), "Size": v("t", 8 * 1024**3),
                    "IdType": v("s", "ext4"), "HintSystem": v("b", False), "HintIgnore": v("b", False)},
                "org.freedesktop.UDisks2.Filesystem": {
                    "MountPoints": v("aay", [mount_path(i, state) + b"\0"] if state["mounted"][i-1] else [])}}
        return result

    def method(_connection, _sender, path, _interface, name, params, invocation):
        try:
            state = json.loads(state_path.read_text())
            if name == "GetManagedObjects":
                invocation.return_value(GLib.Variant("(a{oa{sa{sv}}})", (objects(state),)))
                return
            options = params.unpack()[0]
            with (base / "device-calls.jsonl").open("a") as log:
                log.write(json.dumps({"method": name, "path": path, "options": options}) + "\n")
            if state["error"]:
                invocation.return_dbus_error("org.freedesktop.UDisks2.Error." + state["error"], "Test service rejected operation")
                return
            if name in ("Mount", "Unmount"):
                i = int(path[-1])
                assert options.get("auth.no_user_interaction") is False
                if name == "Unmount":
                    assert options.get("force") is False, "Forced unmount forbidden"
                state["mounted"][i-1] = name == "Mount"
                if name == "Mount" and state.get("replace_after_mount"):
                    state["detected"] += 1
            else:
                assert not any(state["mounted"]), "PowerOff before unmounting all partitions"
                state["present"] = False
            temp = state_path.with_suffix(".peer-tmp")
            temp.write_text(json.dumps(state))
            temp.replace(state_path)
            invocation.return_value(GLib.Variant("(s)", (os.fsdecode(mount_path(i, state)),)) if name == "Mount" else None)
        except BaseException as error:
            invocation.return_dbus_error("org.freedesktop.UDisks2.Error.Failed", repr(error))

    for path, index in ((ROOT, 0), (BLOCK + "1", 1), (BLOCK + "2", 1), (DRIVE, 2)):
        conn.register_object(path, node.interfaces[index], method, None, None)
    print("ready", flush=True)
    GLib.MainLoop().run()


def exercise(fixture, key, click, capture, wait_for):
    # 1360x860 native GUI: 170x53 cells, modal 76x12 at (47,20).
    key("m", "ctrl", "alt")
    time.sleep(.3)
    capture("list")
    click(51, 29)  # Open mounts USB1 then opens the ordinary pane.
    wait_for(lambda: any(c["method"] == "Mount" for c in fixture.calls()), "native mount")
    time.sleep(.6)
    capture("open")
    key("m", "ctrl", "alt")
    fixture.update(error="DeviceBusy")
    click(72, 29)
    wait_for(lambda: len(fixture.calls()) == 2, "busy unmount response")
    time.sleep(.3)
    capture("busy")
    assert json.loads(fixture.state.read_text())["mounted"] == [True, True]
    fixture.update(error="")
    click(72, 29)
    wait_for(lambda: json.loads(fixture.state.read_text())["mounted"] == [False, True], "native unmount")
    time.sleep(.4)
    key("Escape")
    capture("departed")
    key("m", "ctrl", "alt")
    click(51, 30)  # Whole device confirmation.
    time.sleep(.2)
    capture("confirm")
    count = len(fixture.calls())
    click(64, 30)  # Cancel.
    assert len(fixture.calls()) == count
    click(51, 30)
    click(51, 30)  # Confirm.
    wait_for(lambda: not json.loads(fixture.state.read_text())["present"], "native eject")
    assert [c["method"] for c in fixture.calls()][-2:] == ["Unmount", "PowerOff"]
    time.sleep(.3)
    capture("ejected")
    fixture.update(present=True, mounted=[False, False], detected=43)
    time.sleep(2.4)  # Automatic discovery without a Refresh action.
    click(61, 29)  # Explicit Mount (keeps dialog open).
    wait_for(lambda: json.loads(fixture.state.read_text())["mounted"][0], "automatic hotplug discovery")
    time.sleep(.3)
    capture("reconnected")
    key("Escape")
    print("Devices GUI: private UDisks discovery, mouse mount/open/unmount, busy error, cancel/confirm whole-device eject, hotplug; no host devices touched", flush=True)


if __name__ == "__main__" and sys.argv[1] == "--serve":
    serve(Path(sys.argv[2]), sys.argv[3])
elif __name__ == "__main__" and sys.argv[1] == "--host":
    with Fixture(Path(sys.argv[2])) as fixture:
        print(fixture.address, flush=True)
        sys.stdin.read()
