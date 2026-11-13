"""Private appearance files; shared mouse workflows for real GUI and PTY tests."""
import json
import time
from details_fixture import Fixture as DesktopFixture


class Fixture(DesktopFixture):
    def __init__(self, base):
        super().__init__(base)
        self.settings = base / 'config/molten-zharr/basalgrim/settings.json'
        self.settings.parent.mkdir(parents=True, exist_ok=True)
        self.settings.write_text(json.dumps({'grid': False, 'preview': True, 'theme': 'molten-zharr', 'font_size': 16, 'icon_mode': 'symbols'}))
        self.exported = base / 'saved palette.json'
        self.imported = base / 'independent palette.json'

    def config(self):
        return json.loads(self.settings.read_text())


def wait(predicate, label):
    end=time.monotonic()+8
    while not predicate():
        assert time.monotonic()<end, label
        time.sleep(.05)


def exercise(f, key, text, click, screenshot, cells=(170,53)):
    w,h=cells
    # Ordinary settings renderer geometry, not an application test API.
    menu_step=1 if h-6<13 else 2
    stacked=w-2-(10 if w-2<60 else 18)-2<48
    value_x=15 if stacked else w-15
    step=3 if stacked else 2
    def section(index):click(4,3+index*menu_step);time.sleep(.2)
    def row(index):click(value_x,3+index*step+int(stacked));time.sleep(.2)
    def edit(value):
        key('u','ctrl');text(value);key('Return');time.sleep(.3)
    key('F10');section(5);screenshot('profiles')
    row(0);edit('Моя тема')
    wait(lambda:len(f.config()['appearance_presets'])==1,'save named appearance')
    # Use keyboard scrolling where compact settings show only two rows.
    if h<20:
        key('Home');key('Down');key('Down');key('Return')
    else: row(2)
    edit(str(f.exported));wait(f.exported.exists,'palette export')
    original=f.exported.read_bytes()
    data=json.loads(original);data['name']='Independent palette';data['colors']['background']='#112233'
    f.imported.write_text(json.dumps(data))
    if h<20:
        key('Up');key('Return')
    else: row(1)
    edit(str(f.imported))
    wait(lambda:f.config().get('colors',{}).get('molten-zharr',{}).get('background')==[17,34,51],'import independent colors')
    shot=screenshot('imported')
    if hasattr(shot,'getpixel'):assert shot.convert('RGB').getpixel((200,300))==(17,34,51),'GUI did not apply palette pixels'
    # Applying a saved appearance is independent of the imported palette.
    if h<20:
        key('Down');key('Down');key('Return')
    else: row(3)
    wait(lambda:f.config()['colors']['molten-zharr']['background']==[0,0,0],'apply preset snapshot')
    click(16,h-2);edit('Рабочая')
    wait(lambda:f.config()['appearance_presets'][0]['name']=='Рабочая','rename preset')
    # Test safe refusal through the actual async path, preserving the file.
    if h<20:
        key('Up');key('Return')
    else: row(2)
    edit(str(f.exported));time.sleep(.5)
    assert f.exported.read_bytes()==original,'export overwrote an existing file'
    screenshot('existing-file')
    section(4);row(0);row(0);row(1);row(1)
    wait(lambda:f.config()['sidebar_mode']=='hidden' and f.config()['preview_mode']=='hidden','hide panels')
    if h<20: key('Home');key('Down');key('Down');key('Return')
    else: row(2)
    wait(lambda:f.config()['animations'],'enable optional activity')
    key('Escape');time.sleep(.3)
    click(w-12,0);screenshot('places');key('Escape')
    click(w-4,0);screenshot('preview');click(w-5,1)
    # Auto policy must persist and expose overlays again after narrowing/restart.
    key('F10');section(4);row(0);row(1);key('Escape')
    wait(lambda:f.config()['sidebar_mode']=='auto' and f.config()['preview_mode']=='auto','restore automatic panels')
    assert f.config()['grid'] is False and f.config()['icon_mode']=='symbols'
    print('Appearance: mouse save/apply/rename, independent palette import/export, no overwrite, panel policies and animation persistence passed',flush=True)


def compact(f,key,text,click,screenshot,cells=(50,22)):
    w,h=cells
    assert f.config()['appearance_presets'][0]['name']=='Рабочая'
    assert f.config()['sidebar_mode']=='auto'
    click(w-12,0);screenshot('compact-places')
    click(6,h-3)  # scroll button stays usable
    click(w-5 if w<40 else 35,1);time.sleep(.15)
    click(w-4,0);screenshot('compact-preview');click(w-5,1)
    key('F10');click(4,3+5*(1 if h-6<13 else 2));time.sleep(.2)
    key('Tab');key('End');time.sleep(.2)
    screenshot('compact-preset')
    click(16,h-2);key('u','ctrl');text('После запуска');key('Return');time.sleep(.3)
    wait(lambda:f.config()['appearance_presets'][0]['name']=='После запуска','compact mouse rename')
    click(24 if w<52 else 34,h-2)
    wait(lambda:f.config()['appearance_presets']==[],'compact mouse delete')
    key('Escape');print('Appearance: restart and compact panel/preset mouse controls passed',flush=True)


def image_preview(f,key,click,screenshot,cells=(50,22)):
    from PIL import Image
    w,h=cells
    Image.new('RGB',(64,32),(18,157,109)).save(f.root/'00-image.png')
    time.sleep(.6);key('Home');time.sleep(.4)
    click(w-4,0);time.sleep(.3)
    shot=screenshot('compact-image')
    if hasattr(shot,'getdata'):
        assert sum(p==(18,157,109) for p in shot.convert('RGB').getdata())>1000,'image preview missing from overlay'
    click(w-5,1);time.sleep(.2)
    shot=screenshot('compact-image-closed')
    if hasattr(shot,'getdata'):
        assert (18,157,109) not in set(shot.convert('RGB').getdata()),'image remained after overlay close'
