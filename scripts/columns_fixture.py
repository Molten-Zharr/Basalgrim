"""Private desktop files for real header sorting, resizing and checkbox input."""
import json
import time
from details_fixture import Fixture as DesktopFixture
from views_fixture import wait

class Fixture(DesktopFixture):
    def __init__(self,base):
        super().__init__(base);self.file.unlink()
        for name,size in [('a.txt',100),('b.txt',9),('c.txt',20)]: (self.root/name).write_text('x'*size)
        self.settings=base/'config/molten-zharr/basalgrim/settings.json'
        self.settings.parent.mkdir(parents=True,exist_ok=True)
        self.settings.write_text(json.dumps({'view':'list','preview':False,'sidebar_mode':'hidden','preview_mode':'hidden','theme':'molten-zharr','font_size':16,'icon_mode':'images'}))
    def config(self):return json.loads(self.settings.read_text())
    def specs(self):return self.config()['list_columns']['columns']

def first_width(specs,width):
    widths=[s['share']*width//1_000_000 for s in specs]
    while sum(widths)<width:
        i=max(range(len(specs)),key=lambda i:(specs[i]['share']*width-widths[i]*1_000_000,i))
        widths[i]+=1
    return widths[0]

def exercise(f,key,text,click,drag,screenshot,cells=(170,53)):
    w,h=cells;y=7 if h<20 else 8 if h<30 else 9
    my=min(y,h-13)+1
    click(2,y,3);time.sleep(.2);screenshot('menu')
    for index in [1,5,4]:click(5,my+index);time.sleep(.2)
    wait(lambda:len(f.specs())==4,'four checkbox columns')
    assert [s['column'] for s in f.specs()]==['name','size','modified','created']
    key('Escape');time.sleep(.3);screenshot('equal')
    width=w-2;edge=width//4
    delta=max(1,round(width*.15))
    drag((edge,y),(edge+delta,y));time.sleep(.4)
    wait(lambda:f.specs()[0]['manual'],'manual first column')
    first=f.specs()[0]['share']
    # First drag borrows from Size; its right boundary remains in the same place.
    edge=width//2
    drag((edge,y),(edge+max(1,round(width*.08)),y));time.sleep(.4)
    wait(lambda:f.specs()[1]['manual'],'manual second column')
    edge+=max(1,round(width*.08))
    time.sleep(.45);click(edge,y);click(edge,y);time.sleep(.4)
    wait(lambda:not f.specs()[1]['manual'],'double click releases only this custom column')
    assert f.specs()[0]['share']==first
    shares=[s['share'] for s in f.specs()[1:]]
    assert max(shares)-min(shares)<=1,shares
    # The last column can also shrink from its right edge, then rejoin the
    # automatic pool. This exercises the user's 2/5 + 1/5 + 1.5/5 + 0.5/5 case.
    drag((width,y),(width-max(1,round(width*.1)),y));time.sleep(.4)
    wait(lambda:f.specs()[3]['manual'],'last column resize')
    time.sleep(.45);click(width,y);click(width,y);time.sleep(.4)
    wait(lambda:not f.specs()[3]['manual'],'last column double click')
    assert f.specs()[0]['share']==first
    shares=[s['share'] for s in f.specs()[1:]]
    assert max(shares)-min(shares)<=1,shares
    screenshot('custom')
    # Sorting acts on real values, preserving files and selecting no new object.
    x=1+round(width*first/1_000_000)+1
    click(x,y);key('Home');key('F2');key('u','ctrl');text('smallest.txt');key('Return')
    wait(lambda:(f.root/'smallest.txt').exists(),'ascending size header')
    assert not (f.root/'b.txt').exists()
    time.sleep(.3);click(x,y);key('Home');key('F2');key('u','ctrl');text('largest.txt');key('Return')
    wait(lambda:(f.root/'largest.txt').exists(),'descending size header')
    assert not (f.root/'a.txt').exists()
    # Remove one field while the menu stays open; pinned Name is unchanged.
    click(2,y,3);time.sleep(.2);click(5,my+4);key('Escape');time.sleep(.3)
    wait(lambda:len(f.specs())==3,'remove creation field')
    assert f.specs()[0]['share']==first
    screenshot('sorted')
    print('Detail columns: real header menu, equal widths, drag pinning, double-click pool reset and asc/desc size sorting passed',flush=True)

def compact(f,key,text,click,drag,screenshot,cells=(50,22)):
    w,h=cells;y=7 if h<20 else 8;my=min(y,h-13)+1
    before=f.specs();assert len(before)==3 and before[0]['manual']
    screenshot('restart')
    click(2,y,3);time.sleep(.2);click(5,my+8);key('Escape');time.sleep(.3)
    wait(lambda:len(f.specs())==4,'compact add owner field')
    assert f.specs()[0]['share']==before[0]['share']
    # With one pinned column, its right edge is determined by the persisted share.
    edge=first_width(f.specs(),w-2)
    time.sleep(.45);click(edge,y);click(edge,y);time.sleep(.4)
    wait(lambda:not f.specs()[0]['manual'],'compact reset after restart')
    assert max(s['share'] for s in f.specs())-min(s['share'] for s in f.specs())<=1
    screenshot('compact')
    print('Detail columns: persisted widths, compact checkbox controls and double-click reset passed',flush=True)
