"""Private desktop associations and an independent executable for file-details smoke tests."""
import json
import os
from pathlib import Path
import subprocess
import time


class Fixture:
    def __init__(self, base):
        self.base = base
        self.root = base / 'details-files'
        self.root.mkdir()
        self.file = self.root / 'sample.txt'
        self.file.write_text('independent app input\n')
        self.file.chmod(0o640)
        self.log = base / 'opened.json'
        self.apps = base / 'data/applications'
        self.apps.mkdir(parents=True)
        config = base / 'config'
        config.mkdir(exist_ok=True)
        system = base / 'system-data'
        system.mkdir()
        (system / 'mime').symlink_to('/usr/share/mime')
        self.env = dict(HOME=str(base), XDG_DATA_HOME=str(base / 'data'),
                        XDG_CONFIG_HOME=str(config), XDG_STATE_HOME=str(base / 'state'),
                        XDG_DATA_DIRS=str(system), XDG_CONFIG_DIRS=str(base / 'empty-config'),
                        GIO_USE_VFS='local', BASALGRIM_DETAILS_FIXTURE=str(base))
        self.peer = base / 'open-peer.py'
        self.peer.write_text('''import json, os, pathlib, sys, urllib.parse
p=sys.argv[2]
if p.startswith('file:'): p=os.fsdecode(urllib.parse.unquote_to_bytes(urllib.parse.urlsplit(p).path))
data={'app':sys.argv[1], 'path':os.fsencode(p).hex(), 'data':pathlib.Path(p).read_bytes().hex()}
out=pathlib.Path(os.environ['BASALGRIM_DETAILS_FIXTURE'])/'opened.json'
with out.with_name('launches.jsonl').open('a') as events: events.write(json.dumps(data)+'\\n')
tmp=out.with_suffix('.tmp'); tmp.write_text(json.dumps(data)); tmp.replace(out)
''')
        for name, mime, argument in [('Alpha', 'text/plain;', '%f'), ('Beta', 'text/plain;', '%U'), ('Gamma', 'image/png;', '%f')]:
            (self.apps / f'fixture-{name}.desktop').write_text(
                f'[Desktop Entry]\nType=Application\nName=Fixture {name}\nComment=Independent application fixture\n'
                f'Exec=/usr/bin/python3 "{self.peer}" {name} {argument}\nMimeType={mime}\nTerminal=false\n')
        (self.apps / 'mimeinfo.cache').write_text('[MIME Cache]\ntext/plain=fixture-Alpha.desktop;fixture-Beta.desktop;\nimage/png=fixture-Gamma.desktop;\n')
        self.defaults = config / 'mimeapps.list'
        self.defaults.write_text('[Default Applications]\ntext/plain=fixture-Alpha.desktop;\n')
        self.before = self.defaults.read_bytes()
        # Keep GIO away from the user's session bus too.
        bus = subprocess.run(['dbus-daemon', '--session', '--fork', '--print-address=1', '--print-pid=1'], check=True, capture_output=True, text=True).stdout.splitlines()
        self.env['DBUS_SESSION_BUS_ADDRESS'] = bus[0]
        self.bus_pid = int(bus[1])

    def close(self):
        assert self.defaults.read_bytes() == self.before, 'Opening a file changed MIME defaults'
        os.kill(self.bus_pid, 15)


def exercise(f, key, text, click, screenshot, wait):
    key('Return', 'alt'); time.sleep(.3); screenshot('properties')
    click(64, 17); click(66, 21); screenshot('permissions')
    click(52, 34)
    wait(lambda: f.file.stat().st_mode & 0o777 == 0o660, 'mouse Save did not apply permissions')
    time.sleep(.5)  # Wait for the journal result and snapshot-preserving rescan too.
    key('Return', 'alt'); time.sleep(.3); click(64, 17); click(66, 21); click(115, 35)
    assert f.file.stat().st_mode & 0o777 == 0o660, 'Cancel applied permissions'
    key('Return', 'ctrl'); time.sleep(.3); screenshot('applications')
    text('Beta'); time.sleep(.2); screenshot('filtered'); click(52, 34); screenshot('launch')
    wait(f.log.exists, 'chosen application did not receive the file')
    result = json.loads(f.log.read_text())
    assert result['app'] == 'Beta' and bytes.fromhex(result['path']) == os.fsencode(f.file), result
    assert bytes.fromhex(result['data']) == f.file.read_bytes()
    f.log.unlink()
    key('Return', 'ctrl'); time.sleep(.3); text('Gamma'); click(65, 34); time.sleep(.3); key('Return')
    wait(f.log.exists, 'All programs selection did not launch')
    assert json.loads(f.log.read_text())['app'] == 'Gamma'
    f.log.unlink()
    launches = f.base / 'launches.jsonl'
    before = len(launches.read_text().splitlines())
    key('Return', 'ctrl'); time.sleep(.3)
    click(52,20); time.sleep(.45)  # Single click selects Beta without starting it.
    assert not f.log.exists() and len(launches.read_text().splitlines()) == before
    click(52,19); click(52,19)  # Double click Alpha, rather than the previous selection.
    wait(f.log.exists, 'double click did not launch the clicked program')
    time.sleep(.3)
    result=json.loads(f.log.read_text())
    assert result['app']=='Alpha' and bytes.fromhex(result['path'])==os.fsencode(f.file),result
    assert len(launches.read_text().splitlines()) == before+1, 'double click launched twice'
    assert f.defaults.read_bytes() == f.before


def compact(f, key, click, screenshot, wait):
    key('g', 'ctrl'); key('Return', 'alt'); time.sleep(.3)
    click(16, 2); click(19, 6); screenshot('compact-permissions')
    click(4, 19)
    wait(lambda: f.file.stat().st_mode & 0o777 == 0o640, 'compact grid permissions')
    time.sleep(.5)
    f.log.unlink()
    key('Return','ctrl');time.sleep(.3)
    click(4,5);click(4,5)
    wait(f.log.exists,'compact double click')
    assert json.loads(f.log.read_text())['app']=='Beta'
    print('File details native: properties, mouse permissions/save/cancel, filtered MIME choice, All programs, single/double click (also compact), exact argv and unchanged defaults passed', flush=True)


if __name__ == '__main__':
    import sys
    import tempfile
    with tempfile.TemporaryDirectory(prefix='bg-details-worker-') as tmp:
        fixture=Fixture(Path(tmp))
        env=dict(os.environ,**fixture.env)
        # Keep the installed toolchain while isolating desktop settings.
        for name, default in [('RUSTUP_HOME','.rustup'),('CARGO_HOME','.cargo')]:
            env[name]=os.environ.get(name,str(Path.home()/default))
        try:
            result=subprocess.run(sys.argv[1:],env=env)
        finally:
            fixture.close()
        sys.exit(result.returncode)
