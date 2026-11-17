"""Private real-input grouping scene; no user files, settings or clipboard."""
import datetime
import json
import os
import time
from details_fixture import Fixture as DesktopFixture
from views_fixture import wait

class Fixture(DesktopFixture):
    def __init__(self,base):
        super().__init__(base);self.file.unlink()
        for name,year,month in [('a-old.txt',2025,11),('b-new.txt',2026,11),('c-mid.txt',2026,10)]:
            p=self.root/name;p.write_text(name)
            timestamp=datetime.datetime(year,month,15,12,tzinfo=datetime.timezone.utc).timestamp()
            os.utime(p,(timestamp,timestamp))
        self.settings=base/'config/molten-zharr/basalgrim/settings.json'
        self.settings.parent.mkdir(parents=True,exist_ok=True)
        self.settings.write_text(json.dumps({'view':'list','preview':False,'sidebar_mode':'hidden','preview_mode':'hidden','theme':'molten-zharr','font_size':16,'icon_mode':'images'}))
    def config(self):return json.loads(self.settings.read_text())

def exercise(f,key,text,click,screenshot,cells=(170,53)):
    w,h=cells
    # Last cell of a list row is deliberately a background hit, even at 34x14.
    click(w-2,8 if h<20 else h-8,3);key('Home')
    for _ in range(6):key('Down')
    key('Return')
    wait(lambda:f.config().get('grouping',{}).get('enabled'),'background grouping checkbox')
    x=w-32;y=h-13
    click(x,y+7);wait(lambda:f.config()['grouping']['field']=='modified','month of modification')
    screenshot('menu');key('Escape');time.sleep(.2);screenshot('months')
    key('Home');key('F2');key('a','ctrl');text('newest.txt');key('Return')
    wait(lambda:(f.root/'newest.txt').exists(),'newest group first');assert not (f.root/'b-new.txt').exists()
    time.sleep(.3)
    key('g','ctrl','alt');time.sleep(.2)
    # Shortcut uses the current pointer as its anchor; same lower-right placement.
    click(x,y+1);wait(lambda:not f.config()['grouping']['descending'],'oldest group order')
    key('Escape');key('Home');key('F2');key('a','ctrl');text('oldest.txt');key('Return')
    wait(lambda:(f.root/'oldest.txt').exists(),'oldest group first');assert not (f.root/'a-old.txt').exists()
    time.sleep(.3);screenshot('oldest')
    print('Grouping: background checkbox, month criterion, mouse order and real first-file operations passed',flush=True)

def compact(f,key,text,click,screenshot,cells=(50,22)):
    w,h=cells
    assert f.config()['grouping']=={'enabled':True,'field':'modified','descending':False}
    key('Home');key('F2');key('a','ctrl');text('persisted.txt');key('Return')
    wait(lambda:(f.root/'persisted.txt').exists(),'group order after restart');time.sleep(.3)
    # All four existing view modes retain groups, including compact layouts.
    for mode in range(4):
        key('g','ctrl','shift');key('Home')
        # View chooser uses Up rather than Home; move to its first row explicitly.
        for _ in range(4):key('Up')
        for _ in range(mode):key('Down')
        key('Return');time.sleep(.2);key('End');time.sleep(.2);screenshot('view-'+str(mode))
        key('Home');time.sleep(.2)
    # Return to List, then turn grouping off through its own checkbox.
    key('g','ctrl','shift')
    for _ in range(4):key('Up')
    key('Down');key('Return')
    click(w-2,8 if h<20 else h-8,3);key('Home')
    for _ in range(6):key('Down')
    key('Return');time.sleep(.2)
    click(w-32,h-13);wait(lambda:not f.config()['grouping']['enabled'],'compact grouping off')
    key('Escape');screenshot('off')
    print('Grouping: restart, four views and compact mouse checkbox passed',flush=True)
