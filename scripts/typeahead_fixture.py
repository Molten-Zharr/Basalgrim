"""Timed filename selection through genuine keyboard events on private files."""
import json
import time
from columns_fixture import Fixture as ColumnsFixture
from views_fixture import wait

class Fixture(ColumnsFixture):
    def __init__(self,base):
        super().__init__(base)
        for p in self.root.iterdir():p.unlink()
        for name in ['a.txt','ab.txt','b.txt','Фото тест.txt']:(self.root/name).write_text(name)
        settings=self.config();settings.update(typeahead_timeout_ms=2000,restore_session=False)
        self.settings.write_text(json.dumps(settings))

def exercise(f,key,text,click,typing,screenshot,cells=(170,53)):
    def prove(original,new):
        key('F2');key('u','ctrl');text(new);key('Return')
        wait(lambda:(f.root/new).exists(), 'selected file rename '+new)
        assert not (f.root/original).exists(), 'wrong selected file: '+original
        time.sleep(.3)
    typing('a');time.sleep(.9);typing('b');screenshot('joined')
    prove('ab.txt','joined.txt')
    typing('a');time.sleep(2.25);screenshot('expired');typing('b')
    prove('b.txt','expired.txt')
    typing('ФоТо');screenshot('unicode');prove('Фото тест.txt','unicode.txt')
    # Deleting the final query character must not already navigate to the parent.
    typing('zz');key('BackSpace');key('BackSpace');typing('a')
    prove('a.txt','erased.txt')
    # A short deadline is set via the actual settings editor, including comma input.
    w,h=cells
    key('F10');time.sleep(.2);click(3,8 if h<20 else 15)
    click(*( (17,4) if w<70 else (w-10,3) ));text('0,2');key('Return');time.sleep(.2)
    wait(lambda:f.config()['typeahead_timeout_ms']==200,'timeout settings editor')
    screenshot('settings');key('Escape');time.sleep(.2)
    typing('e');time.sleep(.4);typing('j');prove('joined.txt','short.txt')
    # Restore two seconds for compact restart; raw query is never persisted.
    key('F10');time.sleep(.2);click(3,8 if h<20 else 15)
    click(*( (17,4) if w<70 else (w-10,3) ));text('2');key('Return');key('Escape')
    wait(lambda:f.config()['typeahead_timeout_ms']==2000,'persist two seconds')
    typing('u');screenshot('last')
    # Restart from a real older saved configuration, not only new defaults.
    legacy=f.config();legacy.pop('bindings_version',None)
    for binding in legacy['bindings']:
        if binding['action']=='Parent' and binding['key']['key']=='Backspace':binding['key']['ctrl']=False
    f.settings.write_text(json.dumps(legacy))
    print('Typeahead: accumulation, idle reset, Cyrillic, Backspace and editable 0.2/2-second timing passed',flush=True)

def compact(f,key,text,click,typing,screenshot,cells=(50,22)):
    assert f.config()['typeahead_timeout_ms']==2000
    typing('e');time.sleep(.9);typing('x');screenshot('compact')
    key('F2');key('u','ctrl');text('compact.txt');key('Return')
    wait(lambda:(f.root/'compact.txt').exists(),'compact focus after restart')
    assert not (f.root/'expired.txt').exists()
    time.sleep(.3)
    typing('z');key('BackSpace');key('BackSpace');time.sleep(.3)
    key('n','ctrl');text('stayed.txt');key('Return')
    wait(lambda:(f.root/'stayed.txt').exists(),'empty Backspace must stay in folder')
    time.sleep(.3);typing('z');time.sleep(2.25);key('BackSpace')
    key('n','ctrl');text('expired-stayed.txt');key('Return')
    wait(lambda:(f.root/'expired-stayed.txt').exists(),'expired Backspace must stay in folder')
    time.sleep(.3);typing('s');key('BackSpace','ctrl');time.sleep(.3)
    key('n','ctrl');text('parent-proof.txt');key('Return')
    wait(lambda:(f.root.parent/'parent-proof.txt').exists(),'Ctrl+Backspace parent while query is active')
    print('Typeahead: compact/restart persistence and plain Backspace safety and Ctrl+Backspace parent passed',flush=True)
