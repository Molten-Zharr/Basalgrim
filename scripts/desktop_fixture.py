"""Independent private session bus and desktop files for desktop integration checks."""
import json
import os
from pathlib import Path
import subprocess
import time

ID='org.moltenzharr.Basalgrim.desktop'
BUS='org.moltenzharr.Basalgrim'
FM='org.freedesktop.FileManager1'
OBJECT='/org/freedesktop/FileManager1'
class Fixture:
    def __init__(self,base,extra=None):
        self.base=base;self.root=base/'desktop-files';self.root.mkdir()
        self.file=self.root/'sample.txt';self.file.write_text('selected by another application\n')
        self.hidden=self.root/'.hidden';self.hidden.write_text('hidden\n')
        self.folder=self.root/'folder';self.folder.mkdir()
        self.apps=base/'data/applications';self.apps.mkdir(parents=True)
        self.config=base/'config';self.config.mkdir(exist_ok=True)
        self.defaults=self.config/'mimeapps.list'
        self.defaults.write_text('[Default Applications]\ninode/directory=fixture-previous.desktop;\ntext/plain=fixture-editor.desktop;\n')
        for id in ('previous','editor'):
            (self.apps/f'fixture-{id}.desktop').write_text(f'[Desktop Entry]\nType=Application\nName=Fixture {id}\nExec=/bin/true %U\nMimeType=inode/directory;text/plain;\n')
        self.env=dict(os.environ if extra is None else extra);self.env.update(HOME=str(base),XDG_DATA_HOME=str(base/'data'),XDG_CONFIG_HOME=str(self.config),XDG_STATE_HOME=str(base/'state'),XDG_DATA_DIRS=str(base/'empty-data'),XDG_CONFIG_DIRS=str(base/'empty-config'),GIO_USE_VFS='local')
        services=base/'data/dbus-1/services';services.mkdir(parents=True)
        conf=base/'bus.conf';conf.write_text(f'<busconfig><type>session</type><listen>unix:tmpdir={base}</listen><servicedir>{services}</servicedir><policy context="default"><allow own="*"/><allow send_destination="*"/><allow receive_sender="*"/></policy></busconfig>')
        self.bus=subprocess.Popen(['dbus-daemon','--nofork','--print-address=1','--config-file='+str(conf)],env=self.env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        self.env['DBUS_SESSION_BUS_ADDRESS']=self.bus.stdout.readline().strip()
        assert self.env['DBUS_SESSION_BUS_ADDRESS']
    def close(self):
        self.bus.terminate();self.bus.wait(timeout=5)
    def call(self,method,paths=(),dest=FM,check=True,timeout=15):
        return subprocess.run(['gdbus','call','--session','--dest',dest,'--object-path',OBJECT,'--method',FM+'.'+method,json.dumps([p.as_uri() for p in paths]),''],env=self.env,capture_output=True,text=True,check=check,timeout=timeout)
    def default(self):
        return subprocess.run(['gio','mime','inode/directory'],env=self.env,capture_output=True,text=True,check=True).stdout.splitlines()[0]
    def install(self,binary,option):
        return subprocess.run([str(binary),option],env=self.env,capture_output=True,text=True,timeout=10)
    def pid(self):
        r=subprocess.run(['gdbus','call','--session','--dest','org.freedesktop.DBus','--object-path','/org/freedesktop/DBus','--method','org.freedesktop.DBus.GetConnectionUnixProcessID',BUS],env=self.env,capture_output=True,text=True,check=True)
        import re
        return int(re.search(r'uint32 (\d+)',r.stdout)[1])

def exercise(f,binary,key,text,click,screenshot,wait):
    assert 'fixture-previous.desktop' in f.default()
    # Install and assign through actual settings mouse controls.
    key('F10');time.sleep(.2);click(5,9);click(150,3)
    wait(lambda:(f.apps/ID).exists(),'mouse installation')
    assert 'fixture-previous.desktop' in f.default(),'install unexpectedly changed the default'
    click(150,5);wait(lambda:ID in f.default(),'mouse default manager');time.sleep(.3);screenshot('settings');key('Escape')
    pid=f.pid()
    f.call('ShowItems',[f.hidden,f.file]);time.sleep(.5);screenshot('selection')
    key('c','ctrl');time.sleep(.3);key('l','ctrl');key('a','ctrl');text(str(f.folder));key('Return');time.sleep(.4);key('v','ctrl')
    wait(lambda:(f.folder/f.file.name).exists() and (f.folder/f.hidden.name).exists(),'FileManager1 grouped selection did not copy both files')
    assert (f.folder/f.file.name).read_bytes()==f.file.read_bytes()
    # The properties method must refer to precisely the requested object.
    f.call('ShowItemProperties',[f.file]);time.sleep(.4);screenshot('properties');key('Escape')
    f.call('ShowFolders',[f.folder]);time.sleep(.4)
    r=subprocess.run([str(binary),'--desktop','--select',str(f.file)],env=f.env,capture_output=True,text=True,timeout=8)
    assert r.returncode==0,r.stderr;assert f.pid()==pid,'launcher spawned a second desktop instance'
    time.sleep(.4)
    # Desktop Exec through GIO forwards to that same window too.
    subprocess.run(['gio','launch',str(f.apps/ID),str(f.folder)],env=f.env,check=True,capture_output=True,timeout=8)
    time.sleep(.5);assert f.pid()==pid
    key('F10');time.sleep(.2);click(5,9);click(150,7)
    wait(lambda:'fixture-previous.desktop' in f.default(),'restore previous manager')
    assert 'text/plain=fixture-editor.desktop;' in f.defaults.read_text()
    key('Escape')
    r=f.install(binary,'--make-default');assert r.returncode==0,r.stderr
    f.call('ShowFolders',[f.folder]);assert f.pid()==pid,'FileManager1 activation failed to reuse the running desktop instance'
    r=f.install(binary,'--restore-default');assert r.returncode==0,r.stderr
    owner=subprocess.run(['gdbus','call','--session','--dest','org.freedesktop.DBus','--object-path','/org/freedesktop/DBus','--method','org.freedesktop.DBus.NameHasOwner',FM],env=f.env,check=True,capture_output=True,text=True)
    assert 'false' in owner.stdout,('CLI restore did not release the running FileManager1 owner',r.stderr,owner.stdout)
    print('Desktop native: settings install/default/restore, FileManager1 methods, hidden selection, CLI/desktop Exec forwarding and unchanged unrelated defaults passed',flush=True)


def activation(f,binary):
    r=f.install(binary,'--make-default');assert r.returncode==0,r.stderr
    f.call('ShowFolders',[f.folder])
    pid=f.pid()
    assert pid!=os.getpid()
    return pid
