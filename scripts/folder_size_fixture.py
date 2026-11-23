"""Measure actual native text pixels against the shared renderer's fixture."""
import json,os,time
from pathlib import Path
from PIL import Image,ImageChops

def prepare(base):
    source=base/'size-files';folder=source/'a-folder';(folder/'nested').mkdir(parents=True)
    (folder/'nested/part').write_bytes(bytes(4096));(folder/'.hidden').write_bytes(bytes(2048));(folder/'plain').write_bytes(bytes(6144))
    (folder/'loop').symlink_to('.')
    return source

def reference(name,row):
    with Image.open(Path('assets/screenshots')/(name+'.png')) as image:
        # 150x38, centered 68x20 dialog; text body starts three rows down.
        return image.convert('RGB').crop((42*8,(9+row)*16,(42+32)*8,(10+row)*16))

def matches(image,name,row):
    expected=reference(name,row)
    actual=image.convert('RGB').crop((52*8,(16+row)*16,(52+32)*8,(17+row)*16))
    return ImageChops.difference(actual,expected).getbbox() is None

def exercise_automatic(source,key,click,capture,wait_for,settings):
    folder=source/'a-folder'
    reference=source/'a-folder ≈12.0 KiB'
    # Compare the actual row with an independently named folder rendered with
    # the same text, font, highlight and alignment.
    def row():
        return capture('automatic').crop((28*8,9*16,85*8,15*16)).tobytes()
    plain=row()
    folder.rename(reference)
    wait_for(lambda:row()!=plain,'reference label in file pane')
    expected=row()
    larger=source/'a-folder ≈16.0 KiB'
    reference.rename(larger)
    wait_for(lambda:row()!=expected,'larger reference label')
    expected_larger=row()
    larger.rename(folder)
    wait_for(lambda:row()==plain,'original folder label')
    def toggle(enabled):
        key('F10')
        click(5,11)
        click(155,9)
        wait_for(lambda:json.loads(settings.read_text()).get('auto_folder_sizes') is enabled,'persistent folder size setting')
        key('Escape')
    toggle(True)
    wait_for(lambda:row()==expected,'recursive size matches reference pixels')
    added=folder/'new'
    added.write_bytes(bytes(4096))
    wait_for(lambda:row()==expected_larger,'16 KiB after external write')
    added.unlink()
    wait_for(lambda:row()==expected,'original size restored after deletion')
    toggle(False)
    wait_for(lambda:row()==plain,'folder size hidden after disabling')
    print('Automatic folder sizes: native mouse setting, exact row pixels, external changes and disabling passed',flush=True)

def exercise(source,key,click,capture,wait_for):
    folder=source/'a-folder'
    # Contextual folder menu: the entry owns the right-click and the size command.
    click(35,10,3);click(37,17)
    wait_for(lambda:matches(capture('complete'),'folder-size',3),'native exact folder byte total')
    assert matches(capture('counts'),'folder-size',5),'native file/folder counts'
    click(60,34);key('F4','shift')
    wait_for(lambda:matches(capture('cached'),'folder-size-cached',2),'native cached result')
    # Build many cheap entries to keep the real worker active for mouse Stop.
    large=folder/'large';large.mkdir()
    for group in range(500):
        part=large/str(group);part.mkdir();seed=part/'0';seed.write_bytes(b'x')
        for i in range(1,1000):os.link(seed,part/str(i))
    key('F5');click(61,32)
    wait_for(lambda:matches(capture('stopped'),'folder-size-stopped',2),'native mouse cancellation')
    click(60,34)
    # Closing dismisses the modal without navigating through the file beneath it.
    key('F4','shift');click(60,34)
    print('Folder size GUI: mouse menu, exact rendered totals, cached result, Shift+F4, real Stop and close passed',flush=True)
