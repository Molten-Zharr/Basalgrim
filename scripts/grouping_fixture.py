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

def controls(cells):
    w, h = cells
    button_x = 62 if w >= 80 else 34 if w >= 42 else 22
    button_y = 2 if h < 20 else 3
    first_x = min(button_x, w - 34) + 2
    first_y = min(button_y + 1, h - 14) + 1
    return button_x + 2, button_y, first_x, first_y

def exercise(f,key,text,click,screenshot,cells=(170,53)):
    bx,by,x,y = controls(cells)
    click(bx,by)
    screenshot('menu-off')
    assert not f.config().get('grouping',{}).get('enabled',False)
    key('Escape')
    click(bx,by)
    click(x,y+6)
    wait(lambda:f.config().get('grouping')=={'enabled':True,'field':'modified','descending':True},
         'toolbar enables month grouping in one selection')
    screenshot('months')
    key('Home');key('F2');key('a','ctrl');text('newest.txt');key('Return')
    wait(lambda:(f.root/'newest.txt').exists(),'newest group first');assert not (f.root/'b-new.txt').exists()
    time.sleep(.3)
    key('g','ctrl','alt');time.sleep(.2)
    screenshot('menu')
    key('End');key('Return')
    wait(lambda:not f.config()['grouping']['descending'],'keyboard changes group order')
    key('Home');key('F2');key('a','ctrl');text('oldest.txt');key('Return')
    wait(lambda:(f.root/'oldest.txt').exists(),'oldest group first');assert not (f.root/'a-old.txt').exists()
    time.sleep(.3);screenshot('oldest')
    print('Grouping: toolbar, month criterion, keyboard order and real first-file operations passed',flush=True)

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
    # Reopen via toolbar after restart and choose the named first row to turn it off.
    key('g','ctrl','shift')
    for _ in range(4):key('Up')
    key('Down');key('Return')
    bx,by,x,y = controls(cells)
    click(bx,by)
    click(x,y);wait(lambda:not f.config()['grouping']['enabled'],'Без группировки')
    screenshot('off')
    key('g','ctrl','alt');key('End');key('Return')
    wait(lambda:f.config()['grouping']['enabled'] and f.config()['grouping']['field']=='location',
         'last field accessible by keyboard while grouping was off')
    key('g','ctrl','alt');key('Home');key('Return')
    wait(lambda:not f.config()['grouping']['enabled'],'keyboard disables grouping')
    print('Grouping: restart, four views, compact toolbar and direct disabling passed',flush=True)
