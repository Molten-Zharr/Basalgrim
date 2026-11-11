"""Loopback-only servers and private GVfs session for network integration tests."""
import contextlib
import os
from pathlib import Path
import select
import signal
import socket
import subprocess
import sys
import threading
import time

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path("target/network-test-python").resolve()))
USER = "demo"
PASSWORD = "fixture-pass"


class Fixture:
    def __init__(self, base, protocol="ftp"):
        self.closed = False
        try:
            self.setup(base, protocol)
        except BaseException:
            self.close()
            raise

    def setup(self, base, protocol):
        self.base = base
        self.root = base / "remote"
        self.root.mkdir()
        (self.root / "hello.txt").write_text("independent network server\n")
        (self.root / "Подпапка").mkdir()
        (self.root / "Подпапка/nested.txt").write_text("nested network file\n")
        self.local = base / "local"
        self.local.mkdir()
        self.runtime = base / "runtime"
        self.runtime.mkdir(mode=0o700, exist_ok=True)
        self.env = dict(os.environ, HOME=str(base), XDG_CONFIG_HOME=str(base / "config"),
                        XDG_DATA_HOME=str(base / "data"), XDG_STATE_HOME=str(base / "state"),
                        XDG_CACHE_HOME=str(base / "cache"), XDG_RUNTIME_DIR=str(self.runtime),
                        GIO_USE_VFS="gvfs", LC_ALL="C.UTF-8")
        for key in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "GVFS_DISABLE_FUSE"):
            self.env.pop(key, None)
        self.env.pop("DBUS_SESSION_BUS_ADDRESS", None)
        self.env.pop("GIO_USE_VOLUME_MONITOR", None)
        ssh = base / "bin/ssh"
        ssh.parent.mkdir()
        ssh.write_text('#!/usr/bin/python3\nimport os,sys\nos.execv("/usr/bin/ssh", ["ssh", "-F", "/dev/null", "-o", "UserKnownHostsFile=' + str(base / 'known_hosts') + '", "-o", "GlobalKnownHostsFile=/dev/null", "-o", "IdentityAgent=none", "-o", "IdentityFile=none", "-o", "ControlMaster=no", "-o", "ControlPath=none", "-o", "NoHostAuthenticationForLocalhost=no"] + sys.argv[1:])\n')
        ssh.chmod(0o700)
        self.env["PATH"] = str(base / "bin") + os.pathsep + self.env["PATH"]
        self.log = (base / "gvfs.log").open("w")
        self.bus = subprocess.Popen(["dbus-daemon", "--session", "--nofork", "--print-address=1"],
                                    env=self.env, stdout=subprocess.PIPE, stderr=self.log, text=True, start_new_session=True)
        assert select.select([self.bus.stdout], [], [], 5)[0], "Private session bus startup"
        self.env["DBUS_SESSION_BUS_ADDRESS"] = self.bus.stdout.readline().strip()
        secret_env = dict(self.env)
        secret_env.pop("G_MESSAGES_DEBUG", None)
        self.secret = subprocess.Popen(["/usr/bin/python3", __file__, "--no-keyring"], env=secret_env, stdout=subprocess.PIPE, stderr=self.log, text=True, start_new_session=True)
        assert select.select([self.secret.stdout], [], [], 5)[0]
        assert self.secret.stdout.readline().strip() == "ready"
        self.daemon = subprocess.Popen(["/usr/libexec/gvfsd"], env=self.env, stdout=self.log, stderr=self.log, start_new_session=True)
        self.server = None
        self.transports = []
        if protocol == "ftp":
            from pyftpdlib.authorizers import DummyAuthorizer
            from pyftpdlib.handlers import FTPHandler
            from pyftpdlib.servers import FTPServer
            import logging
            logging.getLogger("pyftpdlib").setLevel(logging.CRITICAL)
            auth = DummyAuthorizer()
            auth.add_user(USER, PASSWORD, str(self.root), perm="elradfmwMT")
            class Handler(FTPHandler):
                pass
            Handler.authorizer = auth
            self.server = FTPServer(("127.0.0.1", 0), Handler)
            port = self.server.socket.getsockname()[1]
            target = lambda: self.server.serve_forever(timeout=.1, handle_exit=False)
            self.stop_server = self.server.close_all
        elif protocol == "dav":
            from wsgidav.wsgidav_app import WsgiDAVApp
            from cheroot.wsgi import Server
            app = WsgiDAVApp({"provider_mapping": {"/": str(self.root)}, "verbose": 0,
                             "http_authenticator": {"accept_basic": True, "accept_digest": False},
                             "simple_dc": {"user_mapping": {"*": {USER: {"password": PASSWORD}}}}})
            self.server = Server(("127.0.0.1", 0), app)
            self.server.prepare()
            port = self.server.socket.getsockname()[1]
            target = self.server.serve
            self.stop_server = self.server.stop
        elif protocol == "smb":
            from impacket.smbserver import SimpleSMBServer
            from impacket.ntlm import compute_lmhash, compute_nthash
            self.server = SimpleSMBServer(listenAddress="127.0.0.1", listenPort=0)
            self.server.setSMB2Support(True)
            self.server.addShare("share", str(self.root))
            self.server.addCredential(USER, 0, compute_lmhash(PASSWORD), compute_nthash(PASSWORD))
            port = self.server.getServer().server_address[1]
            target = self.server.start
            self.stop_server = self.server.stop
        elif protocol == "sftp":
            import paramiko
            root = self.root
            class Auth(paramiko.ServerInterface):
                def check_auth_password(self, username, password):
                    return paramiko.AUTH_SUCCESSFUL if (username, password) == (USER, PASSWORD) else paramiko.AUTH_FAILED
                def get_allowed_auths(self, username): return "password"
                def check_channel_request(self, kind, channel):
                    return paramiko.OPEN_SUCCEEDED if kind == "session" else paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED
            class SFTP(paramiko.SFTPServerInterface):
                def local(self, path):
                    value = (root / path.lstrip("/")).resolve()
                    if not value.is_relative_to(root): raise PermissionError("outside fixture")
                    return value
                def list_folder(self, path):
                    try:
                        result = []
                        for entry in self.local(path).iterdir():
                            item = paramiko.SFTPAttributes.from_stat(entry.stat()); item.filename = entry.name
                            result.append(item)
                        return result
                    except OSError as error: return paramiko.SFTPServer.convert_errno(error.errno)
                def stat(self, path):
                    try: return paramiko.SFTPAttributes.from_stat(self.local(path).stat())
                    except OSError as error: return paramiko.SFTPServer.convert_errno(error.errno)
                lstat = stat
                def chattr(self, path, attr):
                    try:
                        paramiko.SFTPServer.set_file_attr(str(self.local(path)), attr)
                        return paramiko.SFTP_OK
                    except OSError as error: return paramiko.SFTPServer.convert_errno(error.errno)
                def mkdir(self, path, attr):
                    try: self.local(path).mkdir(); return paramiko.SFTP_OK
                    except OSError as error: return paramiko.SFTPServer.convert_errno(error.errno)
                def rmdir(self, path):
                    try: self.local(path).rmdir(); return paramiko.SFTP_OK
                    except OSError as error: return paramiko.SFTPServer.convert_errno(error.errno)
                def remove(self, path):
                    try: self.local(path).unlink(); return paramiko.SFTP_OK
                    except OSError as error: return paramiko.SFTPServer.convert_errno(error.errno)
                def rename(self, source, target):
                    try:
                        if self.local(target).exists(): return paramiko.SFTP_FAILURE
                        self.local(source).rename(self.local(target)); return paramiko.SFTP_OK
                    except OSError as error: return paramiko.SFTPServer.convert_errno(error.errno)
                def open(self, path, flags, attr):
                    try:
                        handle = paramiko.SFTPHandle(flags)
                        fd = os.open(self.local(path), flags, 0o600)
                        stream = os.fdopen(fd, "r+b" if flags & os.O_RDWR else "wb" if flags & os.O_WRONLY else "rb")
                        if flags & (os.O_WRONLY | os.O_RDWR): handle.writefile = stream
                        else: handle.readfile = stream
                        return handle
                    except OSError as error: return paramiko.SFTPServer.convert_errno(error.errno)
            listener = socket.socket(); listener.bind(("127.0.0.1", 0)); listener.listen(); listener.settimeout(.2)
            self.server = listener
            port = listener.getsockname()[1]
            host_key = paramiko.RSAKey.generate(2048)
            self.stopped = False
            def target():
                while not self.stopped:
                    try: sock, _ = listener.accept()
                    except (TimeoutError, OSError): continue
                    transport = paramiko.Transport(sock)
                    self.transports.append(transport)
                    transport.add_server_key(host_key)
                    transport.set_subsystem_handler("sftp", paramiko.SFTPServer, SFTP)
                    try: transport.start_server(server=Auth())
                    except Exception: transport.close()
            self.stop_server = lambda: (setattr(self, "stopped", True), listener.close())
        else:
            raise ValueError(protocol)
        self.uri = f"{protocol}://" + (USER + "@" if protocol == "sftp" else "") + f"127.0.0.1:{port}/" + ("share/" if protocol == "smb" else "")
        self.thread = threading.Thread(target=target, daemon=True)
        self.thread.start()

    def close(self):
        if self.closed: return
        self.closed = True
        # Only fixture-owned session/process groups and mounts are addressed.
        if hasattr(self, "uri"):
            with contextlib.suppress(subprocess.TimeoutExpired, OSError):
                subprocess.run(["gio", "mount", "-u", self.uri], env=self.env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5)
        if hasattr(self, "runtime"):
            with contextlib.suppress(subprocess.TimeoutExpired, OSError):
                subprocess.run(["fusermount3", "-u", str(self.runtime / "gvfs")], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5)
        for name in ("daemon", "secret", "bus"):
            child = getattr(self, name, None)
            if child is None: continue
            with contextlib.suppress(ProcessLookupError): os.killpg(child.pid, signal.SIGTERM)
            try: child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                with contextlib.suppress(ProcessLookupError): os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=3)
        for transport in getattr(self, "transports", []): transport.close()
        if hasattr(self, "stop_server"):
            stopper = threading.Thread(target=self.stop_server, daemon=True)
            stopper.start(); stopper.join(timeout=2)
        if hasattr(self, "thread"): self.thread.join(timeout=2)
        if hasattr(self, "log"): self.log.close()

    def __enter__(self): return self
    def __exit__(self, *_): self.close()


if __name__ == "__main__" and sys.argv[1] == "--no-keyring":
    import gi
    gi.require_version("Gio", "2.0")
    from gi.repository import Gio, GLib
    connection = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    connection.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "RequestName",
        GLib.Variant("(su)", ("org.freedesktop.secrets", 0)), None, Gio.DBusCallFlags.NONE, 2000, None)
    connection.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "RequestName",
        GLib.Variant("(su)", ("org.gnome.keyring", 0)), None, Gio.DBusCallFlags.NONE, 2000, None)
    node=Gio.DBusNodeInfo.new_for_xml('<node><interface name="org.gnome.keyring.Daemon"><method name="GetEnvironment"><arg type="a{ss}" direction="out"/></method></interface></node>')
    connection.register_object("/org/gnome/keyring/daemon",node.interfaces[0],lambda c,s,p,i,m,v,call:call.return_value(GLib.Variant("(a{ss})",({},))),None,None)
    print("ready", flush=True)
    GLib.MainLoop().run()


def exercise(fixture,key,type_text,click,capture,wait_for):
    key("k","ctrl","alt");time.sleep(.3)
    click(53,32);key("u","ctrl");type_text(fixture.uri);click(53,33)
    time.sleep(.6);capture("credentials")
    click(53,21);key("u","ctrl");type_text(USER)
    click(53,23);key("u","ctrl");type_text(PASSWORD)
    capture("masked-password");click(53,33);time.sleep(.7)
    capture("folder")
    key("slash");type_text("hello.txt");key("Return");time.sleep(.2);key("c","ctrl")
    key("l","ctrl");key("u","ctrl");type_text(str(fixture.local));key("Return");time.sleep(.3);key("v","ctrl")
    wait_for(lambda:(fixture.local/"hello.txt").exists(),"native GVfs file clipboard download")
    assert (fixture.local/"hello.txt").read_text()=="independent network server\n"
    key("k","ctrl","alt");time.sleep(.3);capture("connections")
    click(74,32);time.sleep(.2);capture("disconnect");click(53,33)
    wait_for(lambda:not any((fixture.runtime/"gvfs").iterdir()),"native GVfs disconnect")
    key("Escape")
    print("Network GUI: real private FTP/GVfs auth, masked password, ordinary pane, common clipboard download, confirmed unmount passed",flush=True)


def compact(fixture,key,type_text,click,capture,wait_for):
    """Smallest native window: 400x360 pixels, 50x22 text cells."""
    key("k","ctrl","alt");time.sleep(.2);click(3,17)
    click(22,8);type_text(fixture.uri.removeprefix("ftp://"));click(3,18);time.sleep(.6)
    click(3,6);key("u","ctrl");type_text(USER)
    click(3,8);key("u","ctrl");type_text(PASSWORD);capture("compact-password")
    click(3,18)
    wait_for(lambda:any((fixture.runtime/"gvfs").iterdir()),"compact GUI connect")
    time.sleep(.5);capture("compact-folder")
    key("k","ctrl","alt");time.sleep(.2);click(25,17);click(3,18)
    wait_for(lambda:not any((fixture.runtime/"gvfs").iterdir()),"compact GUI disconnect from remote pane")
    key("Escape")
    print("Network GUI: minimum window mouse auth and disconnect passed",flush=True)
