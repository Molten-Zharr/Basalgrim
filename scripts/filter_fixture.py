"""Private minecraft scene shared by native GUI and plain PTY filter checks."""
import json
import time
from details_fixture import Fixture as DesktopFixture
from views_fixture import wait

class Fixture(DesktopFixture):
    def __init__(self, base):
        super().__init__(base)
        self.file.unlink()
        self.root.rename(base/'minecraft'); self.root=base/'minecraft'
        (self.root/'config/weapon_master').mkdir(parents=True)
        (self.root/'config/bulk').mkdir()
        for i in range(140): (self.root/f'config/bulk/paged-{i:03}.txt').write_text(str(i))
        for name in ('readme.txt','config/weapon_master.toml','config/weapon_master/weapon_master.toml','config/UPPER.TOML'):
            (self.root/name).write_text(name)
        self.settings=base/'config/molten-zharr/basalgrim/settings.json'
        self.settings.parent.mkdir(parents=True,exist_ok=True)
        self.settings.write_text(json.dumps({'view':'list','preview':False,'sidebar_mode':'hidden','preview_mode':'hidden','theme':'molten-zharr','font_size':16,'icon_mode':'images'}))
    def config(self): return json.loads(self.settings.read_text())

def exercise(f,key,text,click,shot,cells=(170,53)):
    w,h=cells
    bar=h-4 if h<20 else h-9 if w<76 else h-6
    compact=w<76
    def query(value):
        key('f','ctrl');key('a','ctrl');key('BackSpace')
        if value:text(value)
        key('Return');time.sleep(.25)
    def depth(n,deep=False):
        click(w-(13 if compact else 31)-(6 if deep else 0),bar)
        time.sleep(.15);shot('depth-menu')
        click(3 if compact else w-40,h-10+n);time.sleep(.35)
    def options(index,deep=True):
        click(w-(6 if compact else 10),bar);time.sleep(.15)
        click(3 if compact else w-40,h-(12 if deep else 9)+index);time.sleep(.2)
    # Click the real bottom input, then use ordinary typing and text selection.
    click(5 if compact else 12,bar);text('weapon_master.toml')
    if not compact:
        key('a','ctrl');key('c','ctrl');time.sleep(.4)
        key('BackSpace');key('v','ctrl');time.sleep(.5)
    time.sleep(.2);shot('depth-zero')
    depth(1)
    key('Home');key('F2');text('level_one');key('Return')
    wait(lambda:(f.root/'config/level_one.toml').exists(),'depth 1 result is the direct child')
    assert (f.root/'config/weapon_master/weapon_master.toml').exists()
    time.sleep(.3)
    depth(2,True)
    shot('depth-two');key('Home');key('F2');text('level_two');key('Return')
    wait(lambda:(f.root/'config/weapon_master/level_two.toml').exists(),'depth 2 operation uses the nested raw path')
    time.sleep(.3)
    query('')
    options(5);text('toml');key('Return');time.sleep(.3)
    options(0);key('Escape')  # Case-sensitive extension: exclude UPPER.TOML.
    options(3);key('Escape')  # Files only; directories are still traversed.
    options(1);key('Escape')
    wait(lambda:f.config().get('keep_filters_on_navigation'),'persistent preserve-filter preference')
    shot('options')
    # Four real renderers retain the source-folder line.
    for mode in range(4):
        key('g','ctrl','shift')
        for _ in range(4):key('Up')
        for _ in range(mode):key('Down')
        key('Return');time.sleep(.2);shot('view-'+str(mode))
    key('g','ctrl','shift')
    for _ in range(4):key('Up')
    key('Down');key('Return');time.sleep(.2)
    query('level_two')
    # Navigate using the ordinary address editor, retaining all filter options.
    key('l','ctrl');key('a','ctrl');text(str(f.root/'config'));key('Return');time.sleep(.4)
    key('Home');key('F2');text('preserved');key('Return')
    wait(lambda:(f.root/'config/weapon_master/preserved.toml').exists(),'preserved filter after changing directory')
    time.sleep(.3)
    click(w-3,bar);time.sleep(.3);shot('cleared')
    query('paged-');depth(1)
    key('f','ctrl');click(10,8 if h<20 else 10,5);time.sleep(.2);shot('wheel-editing')
    key('Return');key('F2');text('wheel_kept');key('Return')
    wait(lambda:(f.root/'config/bulk/wheel_kept.txt').exists(),'wheel while editing preserves selected file')
    assert not (f.root/'config/bulk/paged-000.txt').exists()
    time.sleep(.3)
    click(w-(9 if compact else 16),bar);time.sleep(.4);shot('late-page')
    key('End');key('F2');text('late_page');key('Return')
    wait(lambda:(f.root/'config/bulk/late_page.txt').exists(),'a real result beyond the first 128 matches')
    assert not (f.root/'config/bulk/paged-139.txt').exists()
    time.sleep(.3);click(w-3,bar);time.sleep(.2)
    print('Filter: mouse field/depth/options/reset, exact nested rename targets, case/extension, four views and preserved navigation passed',flush=True)

def compact(f,key,text,click,shot,cells=(50,22)):
    assert f.config().get('keep_filters_on_navigation')
    # New application does not restore stale query text. The preference survives.
    w,h=cells;bar=h-4 if h<20 else h-9
    click(5,bar);text('preserved');key('Return');time.sleep(.2)
    click(w-13,bar);time.sleep(.15);click(max(3,w-40),h-3);time.sleep(.35) # Maximum.
    shot('compact-max')
    key('Home');key('F2');text('restarted');key('Return')
    wait(lambda:(f.root/'config/weapon_master/restarted.toml').exists(),'maximum depth after restart')
    time.sleep(.3);click(w-3,bar);time.sleep(.2);shot('compact-reset')
    print('Filter: compact maximum depth and persisted preference passed',flush=True)
