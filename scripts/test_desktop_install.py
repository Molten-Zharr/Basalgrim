#!/usr/bin/env python3
"""Install/restore checks with private HOME/XDG/bus, including hostile Exec paths."""
import os
import json
import time
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
sys.dont_write_bytecode=True
from desktop_fixture import Fixture,ID
binary=Path(sys.argv[1] if len(sys.argv)>1 else 'target/release/basalgrim').resolve()
with tempfile.TemporaryDirectory(prefix='bg-install-') as tmp:
    f=Fixture(Path(tmp))
    try:
        # Path contains shell syntax; no shell is ever used by the installer/launcher.
        directory=Path(tmp)/'a % " $ ` \\';directory.mkdir();exe=directory/'basalgrim';shutil.copy2(binary,exe)
        r=f.install(exe,'--install-desktop');assert r.returncode==0,r.stderr
        assert 'fixture-previous.desktop' in f.default()
        # Independent GIO parser checks the exact decoded executable (no shell execution).
        script='''import gi,sys\ngi.require_version("Gio","2.0")\nfrom gi.repository import Gio\na=Gio.DesktopAppInfo.new_from_filename(sys.argv[1]);assert a\nprint(a.get_commandline())\n'''
        result=subprocess.run(['/usr/bin/python3','-c',script,str(f.apps/ID)],env=f.env,capture_output=True,text=True,check=True)
        assert result.stdout.strip(),result
        log=Path(tmp)/'argv.json'
        exe.write_text('#!/usr/bin/python3\nimport json,sys\nopen('+repr(str(log))+',"w").write(json.dumps(sys.argv[1:]))\n');exe.chmod(0o755)
        r=subprocess.run(['gio','launch',str(f.apps/ID),str(f.folder)],env=f.env,capture_output=True,text=True,timeout=5);assert r.returncode==0,(r.stderr,(f.apps/ID).read_text(),result.stdout)
        end=time.monotonic()+5
        while not log.exists():
            assert time.monotonic()<end,'desktop Exec did not start the exact executable';time.sleep(.01)
        assert json.loads(log.read_text()) in (['--desktop','--',f.folder.as_uri()],['--desktop','--',str(f.folder)]),log.read_text()
        shutil.copy2(binary,exe)
        r=f.install(exe,'--make-default');assert r.returncode==0,r.stderr;assert ID in f.default()
        r=f.install(exe,'--make-default');assert r.returncode==0,r.stderr
        log.unlink()
        exe.write_text('#!/usr/bin/python3\nimport json,sys\nopen('+repr(str(log))+',"w").write(json.dumps(sys.argv[1:]))\n');exe.chmod(0o755)
        # The recorder intentionally exits without owning the name; activation must
        # still execute the exact binary/argv rather than interpreting the path.
        try:activation=f.call('ShowFolders',[f.folder],check=False,timeout=2)
        except subprocess.TimeoutExpired:activation=None
        assert log.exists() and json.loads(log.read_text())==['--file-manager'],(activation.stderr if activation else 'activation did not own the name',(Path(f.env['XDG_DATA_HOME'])/'dbus-1/services/org.freedesktop.FileManager1.service').read_text())
        shutil.copy2(binary,exe)
        r=f.install(exe,'--restore-default');assert r.returncode==0,r.stderr
        assert 'fixture-previous.desktop' in f.default();assert 'text/plain=fixture-editor.desktop;' in f.defaults.read_text()
        alias=Path(f.env['XDG_DATA_HOME'])/'dbus-1/services/org.freedesktop.FileManager1.service'
        alias.write_text('[D-BUS Service]\nName=org.freedesktop.FileManager1\nExec=/bin/false\n')
        before=alias.read_bytes();r=f.install(exe,'--make-default');assert r.returncode!=0
        assert alias.read_bytes()==before and 'fixture-previous.desktop' in f.default();alias.unlink()
        desktop=f.apps/ID;desktop.unlink();desktop.symlink_to(f.defaults)
        before=f.defaults.read_bytes();r=f.install(exe,'--install-desktop');assert r.returncode!=0
        assert f.defaults.read_bytes()==before
        print('Desktop installer: private install, default/restore/idempotency, foreign service and symlink preservation passed')
    finally:f.close()
