"""Independent executable-plugin peer; all packages, files, logs and settings are private."""
import json
import os
from pathlib import Path
import time
from details_fixture import Fixture as DesktopFixture


class Fixture(DesktopFixture):
    def __init__(self, base):
        super().__init__(base)
        self.file.unlink()
        self.file = self.root / 'данные.json'
        self.file.write_text('{"answer":42}')
        self.package = base / 'пакет'
        self.package.mkdir()
        self.invocations = base / 'plugin-requests.jsonl'
        self.env['BASALGRIM_PLUGIN_TEST_LOG'] = str(self.invocations)
        manifest = {'api': 1, 'id': 'native-peer', 'name': 'Проверка JSON', 'version': '1',
                    'description': 'Независимое расширение для проверки мыши.', 'executable': 'peer.py',
                    'commands': [{'id': 'check', 'name': 'Проверить JSON', 'contexts': ['file', 'selection'], 'extensions': ['json']},
                                 {'id': 'folder', 'name': 'Текущая папка', 'contexts': ['background']}],
                    'preview': ['json'], 'events': ['directory_changed', 'operation_finished']}
        (self.package / 'basalgrim-plugin.json').write_text(json.dumps(manifest))
        peer = self.package / 'peer.py'
        peer.write_text('''#!/usr/bin/python3
import json,sys,os
r=json.load(sys.stdin)
with open(os.environ['BASALGRIM_PLUGIN_TEST_LOG'],'a') as f:f.write(json.dumps(r)+'\\n')
print(json.dumps({'message':'Проверено успешно','text':'EXTENSION PREVIEW 42'}))
''')
        peer.chmod(0o700)
        self.store = base / 'data/molten-zharr/basalgrim/extensions'

    def requests(self, hook=None):
        rows = [json.loads(line) for line in self.invocations.read_text().splitlines()] if self.invocations.exists() else []
        return [r for r in rows if hook is None or r['hook'] == hook]

    def enabled(self):
        path = self.store / 'registry.json'
        return path.exists() and 'native-peer' in json.loads(path.read_text())['enabled']


def wait(predicate, label):
    end = time.monotonic() + 10
    while not predicate():
        assert time.monotonic() < end, label
        time.sleep(.04)


def exercise(f, key, text, click, screenshot, cells=(170, 53), install=True):
    width, height = cells
    mx = (width - min(width, 84)) // 2 + 1
    my = (height - min(height, 12)) // 2 + 1
    bottom = my + min(height, 12) - 2
    key('x', 'ctrl', 'alt'); time.sleep(.3)
    if install:
        click(mx+3, bottom-2); time.sleep(.2)
        key('a', 'ctrl'); text(str(f.package)); time.sleep(.2)
        screenshot('install-path')
        click((width-min(width,68))//2+3, (height-10)//2+8)
        wait(lambda:(f.store/'native-peer').is_dir(), 'package installation')
        time.sleep(.3); assert not f.enabled() and not f.requests(), 'install executed disabled code'
        screenshot('disabled')
        click(mx+3,bottom-3); wait(f.enabled, 'mouse enable'); time.sleep(.4)
    else:
        assert f.enabled(), 'enabled state did not survive restart'
    click(mx+2,my); time.sleep(.2)
    screenshot('commands')
    before=len(f.requests('command'))
    click(mx+3,my+1); time.sleep(.25)
    assert len(f.requests('command'))==before, 'single click launched a command'
    # Execute by mouse; double-click is separately verified by shared input tests.
    click(mx+3,bottom-3)
    wait(lambda:len(f.requests('command'))==before+1, 'mouse plugin command')
    last=f.requests('command')[-1]
    assert last['command']=='check' and bytes(last['context']['selected'][0]['path'])==os.fsencode(f.file)
    time.sleep(.6)
    click(mx+3,my+1);click(mx+3,my+1)
    wait(lambda:len(f.requests('command'))==before+2,'double click command')
    time.sleep(.4);assert len(f.requests('command'))==before+2,'double click ran more than once'
    time.sleep(.2); screenshot('completed'); key('Escape');time.sleep(.3)
    # The existing preview is automatically refreshed after enabling the provider.
    wait(lambda:bool(f.requests('preview')), 'plugin preview provider')
    screenshot('preview')
    # A successful ordinary operation is delivered to the enabled event handler.
    before=len(f.requests('operation_finished'))
    key('n','ctrl');time.sleep(.15);text('event-file.txt');time.sleep(.15)
    click((width-min(width,68))//2+3,(height-10)//2+8)
    wait(lambda:(f.root/'event-file.txt').exists(),'ordinary create')
    wait(lambda:len(f.requests('operation_finished'))>before,'operation event')
    key('x','ctrl','alt');time.sleep(.25)
    screenshot('installed')
    print('Extension native: disabled install, mouse enable/run, exact Unicode selection, preview and operation event passed',flush=True)


def remove(f,key,click,cells):
    width,height=cells;mx=(width-min(width,84))//2+1;my=(height-min(height,12))//2+1;bottom=my+min(height,12)-2
    # Manager is already open on Installed.
    click(mx+3,bottom-3);wait(lambda:not f.enabled(),'mouse disable');time.sleep(.2)
    click(mx+17,bottom-3);wait(lambda:not (f.store/'native-peer').exists(),'mouse remove')
    assert f.package.is_dir() and f.file.exists()
    key('Escape');time.sleep(.2)
