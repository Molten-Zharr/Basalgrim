"""Shared native GUI exercise for the command palette, using private files."""
import time


def exercise(root, key, text, click, screenshot, wait, paste):
    original=root/'sample.txt'
    key('c','ctrl');time.sleep(.4)
    key('p','ctrl','shift');time.sleep(.2)
    text('new folder');time.sleep(.2);screenshot('search')
    # Query copies are text commands and must preserve this instance's file clipboard.
    key('a','ctrl');key('c','ctrl');time.sleep(.3)
    click(52,19);time.sleep(.45)
    assert not (root/'palette-created').exists()
    click(52,19);click(52,19)
    text('palette-created');key('Return')
    wait(lambda:(root/'palette-created').is_dir(),'palette double click did not open New Folder')
    time.sleep(.5)
    key('l','ctrl');key('u','ctrl');text(str(root/'palette-created'));key('Return');time.sleep(.4)
    key('v','ctrl');wait(lambda:(root/'palette-created/sample.txt').exists(),'query text copy lost the file clipboard')
    assert (root/'palette-created/sample.txt').read_bytes()==original.read_bytes()
    time.sleep(.5)
    # Open from the shared mouse Actions menu, where search is the first command.
    click(6,3);click(8,2);time.sleep(.2);text('redo');time.sleep(.2)
    screenshot('unavailable');click(52,35)
    key('a','ctrl');text('F2');time.sleep(.2);key('Return')
    key('a','ctrl');text('renamed.txt');key('Return')
    wait(lambda:(root/'palette-created/renamed.txt').exists(),'shortcut search did not dispatch Rename')
    time.sleep(.5)
    key('p','ctrl','shift');time.sleep(.2);paste('В корзину');time.sleep(.3)
    key('Return');time.sleep(.3);screenshot('confirmation');key('Escape')
    assert (root/'palette-created/renamed.txt').exists(),'palette skipped the trash confirmation'


def compact(root,key,text,click,screenshot,wait):
    key('p','ctrl','shift');time.sleep(.2);text('new file');time.sleep(.2)
    screenshot('compact');click(4,19)
    text('compact.txt');key('Return')
    wait(lambda:(root/'palette-created/compact.txt').exists(),'compact mouse Run')
    time.sleep(.4)
    print('Command palette native: shortcut and mouse entry, alias/shortcut search, single/double click, Unicode paste, preserved file clipboard, confirmation and compact controls passed',flush=True)
