"""Private file-tree fixtures and real mouse workflows shared by GUI and PTY."""
import json
import time
from details_fixture import Fixture as DesktopFixture

class Fixture(DesktopFixture):
    def __init__(self, base):
        super().__init__(base)
        self.file.unlink()
        self.folder=self.root/'folder'
        (self.folder/'inner').mkdir(parents=True)
        (self.folder/'inner/deep.txt').write_text('deep original')
        (self.folder/'child.txt').write_text('child original')
        for i in range(150): (self.root/f'item_{i:03}.txt').write_text(str(i))
        self.settings=base/'config/molten-zharr/basalgrim/settings.json'
        self.settings.parent.mkdir(parents=True,exist_ok=True)
        self.settings.write_text(json.dumps({'grid':False,'preview':False,'sidebar_mode':'hidden',
            'preview_mode':'hidden','theme':'molten-zharr','font_size':16,'icon_mode':'images'}))
    def config(self):return json.loads(self.settings.read_text())

def wait(predicate,label):
    end=time.monotonic()+8
    while not predicate():
        assert time.monotonic()<end,label
        time.sleep(.05)

def exercise(f,key,text,click,screenshot,cells=(170,53),wheel=None):
    w,h=cells
    x=max(0,w-50)//2+1;y=max(0,h-12)//2+1
    def menu():key('g','ctrl','shift');time.sleep(.2)
    def choose(index):
        menu();click(x+5,y+index);time.sleep(.3)
        wait(lambda:f.config().get('view')==['icons','list','columns','tree'][index],'view choice')
    def rename(name):key('F2');key('u','ctrl');text(name);key('Return');time.sleep(.4)
    # The toolbar's View button is always a visible mouse target, even at 34x14.
    click(55 if w>=80 else 29,2 if h<20 else 3)
    screenshot('chooser')
    click(x+5,y+2)
    wait(lambda:f.config().get('view')=='columns','toolbar view chooser')
    menu();click(x+min(w,50)-6,y+5);time.sleep(.2)
    wait(lambda:f.config()['view_columns']==3,'column count mouse plus')
    click(x+min(w,50)-11,y+5);click(x+5,y+2);time.sleep(.3)
    wait(lambda:f.config()['view_columns']==2,'column count mouse minus')
    screenshot('columns')
    key('Home');key('Down')  # Focus item_000, then scroll independently.
    if wheel:wheel(10,8 if h<20 else 10,1)
    screenshot('scrolled')
    rename('renamed-column.txt')
    wait(lambda:(f.root/'renamed-column.txt').exists(),'wheel changed the selected file')
    assert not (f.root/'item_000.txt').exists()
    choose(0);screenshot('icons')
    choose(1);screenshot('list')
    choose(3);key('Home');time.sleep(.2)
    first=8 if h<20 else 9 if h<30 else 11
    click(4,first);time.sleep(.4)  # Root expansion arrow; no navigation.
    click(6,first+1);time.sleep(.4)  # Nested expansion arrow.
    screenshot('tree')
    key('Home');key('Down');key('Down');rename('renamed-deep.txt')
    wait(lambda:(f.folder/'inner/renamed-deep.txt').exists(),'nested mouse expansion/rename')
    assert not (f.folder/'inner/deep.txt').exists()
    (f.folder/'inner/aa-new.txt').write_text('watcher')
    time.sleep(.7);key('Home');key('Down');key('Down');rename('watched.txt')
    wait(lambda:(f.folder/'inner/watched.txt').exists(),'expanded directory watch')
    key('Home');click(4,first);time.sleep(.3);screenshot('collapsed')
    print('Views: four mouse choices, column count, viewport scrolling, nested tree and watcher operations passed',flush=True)

def compact(f,key,text,click,screenshot,cells=(50,22)):
    assert f.config()['view']=='tree' and f.config()['view_columns']==2
    w,h=cells;x=max(0,w-50)//2+1;y=max(0,h-12)//2+1
    key('g','ctrl','shift');time.sleep(.2);screenshot('compact-chooser')
    click(x+5,y+3);time.sleep(.3);key('Home')
    first=8 if h<20 else 9
    click(4,first);time.sleep(.3);click(6,first+1);time.sleep(.3)
    key('Home');key('Down');key('Down');key('F2');key('u','ctrl');text('restart.txt');key('Return')
    wait(lambda:(f.folder/'inner/restart.txt').exists(),'compact/restart tree mouse controls')
    screenshot('compact-tree')
    print('Views: restart persistence and compact nested mouse controls passed',flush=True)
