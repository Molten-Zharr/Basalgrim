"""Recursive search through real input; all files belong to the test sandbox."""
import time

def prepare(base):
    source=base/'search-files';(source/'nested/deeper').mkdir(parents=True)
    for index in range(2105):
        (source/f'nested/deeper/needle-{index:04}.txt').write_text(f'search fixture {index}')
    return source

def exercise(source,key,insert,click,capture,wait_for,w=170,h=53):
    inventory=list((source/'nested/deeper').iterdir())
    expected=inventory[-1]
    expected_text=expected.read_text()
    key('f','ctrl');time.sleep(.2);key('F3');time.sleep(.2)
    insert('needle');time.sleep(1)
    capture('recursive-search')
    x=(w-min(w,96))//2+1;y=(h-min(h,28))//2+1
    click(x+21,y+min(h,28)-4);time.sleep(.5) # Last result, beyond the old 2000-result cutoff.
    capture('recursive-search-last')
    click(x+2,y+min(h,28)-4);time.sleep(.6)
    key('F2');time.sleep(.2);key('a','ctrl');insert('located.txt');key('Return')
    result=source/'nested/deeper/located.txt'
    wait_for(result.exists,'recursive result navigation and focused rename')
    assert result.read_text()==expected_text
    assert not expected.exists(), 'navigation must focus the final raw result path'
    key('f','ctrl');time.sleep(.2);key('F3');time.sleep(.2);insert('absent');time.sleep(.4)
    click(x+2,y+min(h,28)-3);time.sleep(.2) # Stop remains reachable at minimum size.
    key('Escape')
    print('Recursive search: 2105 matches, last-page mouse navigation, exact focus, Stop and close passed',flush=True)
