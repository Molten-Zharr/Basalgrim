"""Native shell input, clipboard and actual VT pixels on private displays."""
import time
from pathlib import Path
from PIL import Image,ImageChops

def prepare(base):
    source=base/'shell-files';source.mkdir()
    (source/"a space' имя.txt").write_text('keep')
    (base/'.bashrc').write_text("PS1='BASALGRIM> '\nbind 'set enable-bracketed-paste on'\n")
    return source

def exercise(source,key,type_text,paste,click,capture,wait_for):
    files_header=capture('files-before').convert('RGB').crop((0,0,1360,16))
    key('F4')
    def command(text):paste(text);key('Return')
    command('pwd > cwd; echo $$ > shellpid')
    wait_for(lambda:(source/'shellpid').exists(),'native shell startup and clipboard paste')
    assert (source/'cwd').read_text().strip()==str(source)
    pid=(source/'shellpid').read_text().strip()
    type_text('touch typed');key('Return')
    wait_for(lambda:(source/'typed').exists(),'native raw keyboard text')
    type_text('touc');key('Tab');type_text(' tabbed');key('Return')
    wait_for(lambda:(source/'tabbed').exists(),'native Tab completion')
    paste('printf %s ');click(3,31);paste(' > quoted');key('Return')
    wait_for(lambda:(source/'quoted').exists(),'native mouse path insertion')
    assert (source/'quoted').read_text()==str(source/"a space' имя.txt")
    command('sleep 60');key('c','ctrl');command('touch interrupted')
    wait_for(lambda:(source/'interrupted').exists(),'native Ctrl+C sent to PTY')
    command("printf '\\033[2J\\033[H\\033[48;2;17;199;111mCOLOR_OK\\033[0m\\n'")
    # Compare actual font pixels, background, ANSI color and cursor-independent output row.
    with Image.open('target/preview/terminal-native.png') as image:
        expected=image.convert('RGB').crop((8,33*16,9*8,34*16))
    def matches():
        actual=capture('color').convert('RGB').crop((8,33*16,9*8,34*16))
        return ImageChops.difference(actual,expected).getbbox() is None
    wait_for(matches,'native exact terminal color/text pixels')
    # Change terminal size through its actual mouse control and observe kernel winsize.
    command('stty size > before')
    wait_for(lambda:(source/'before').exists(),'size before expand')
    click(160,30);time.sleep(.3)
    command('stty size > after')
    wait_for(lambda:(source/'after').exists(),'expanded SIGWINCH')
    assert int((source/'after').read_text().split()[0])>int((source/'before').read_text().split()[0])
    capture('expanded')
    click(147,1);time.sleep(.2) # hide expanded shell; worker/session stays alive
    assert Path('/proc/'+pid).exists()
    key('F4');command('echo $$ > resumed')
    wait_for(lambda:(source/'resumed').exists(),'resume hidden native shell')
    assert (source/'resumed').read_text().strip()==pid
    click(167,1);key('Escape') # cancel close without sending Escape to shell
    command('touch aftercancel')
    wait_for(lambda:(source/'aftercancel').exists(),'close dialog cancelled')
    command('exit')
    def files_visible():
        actual=capture('auto-close').convert('RGB').crop((0,0,1360,16))
        return ImageChops.difference(actual,files_header).getbbox() is None
    wait_for(files_visible,'shell exit automatically restores native file panel')
    wait_for(lambda:not Path('/proc/'+pid).exists(),'exited shell reaped')
    other=source/'next-folder';other.mkdir()
    key('l','ctrl');key('u','ctrl');type_text(str(other));key('Return');time.sleep(.3)
    key('F4');command('pwd > fresh-cwd; echo $$ > fresh-pid')
    wait_for(lambda:(other/'fresh-cwd').exists() and (other/'fresh-pid').exists(),'F4 fresh shell in current folder')
    assert (other/'fresh-cwd').read_text().strip()==str(other)
    assert (other/'fresh-pid').read_text().strip()!=pid
    key('d','ctrl');wait_for(files_visible,'Ctrl+D automatically restores native file panel')
    key('F4');command('echo $$ > final-pid')
    wait_for(lambda:(other/'final-pid').exists(),'new shell after EOF')
    pid=(other/'final-pid').read_text().strip()
    click(167,1);key('Return')
    wait_for(lambda:not Path('/proc/'+pid).exists(),'confirmed native shell close/reap')
    capture('closed')
    assert (source/"a space' имя.txt").read_text()=='keep'
    print('Embedded terminal GUI: native text/Tab/Ctrl+C, clipboard, quoted path, exact VT pixels, resize, hide/resume, automatic exit/EOF close, fresh cwd and confirmed close passed',flush=True)
