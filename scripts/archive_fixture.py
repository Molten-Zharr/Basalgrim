"""Independent Python archives and real native mouse/keyboard scenarios."""
import time, tarfile, zipfile

def prepare(base):
    root=base/'archive-files'; folder=root/'a-folder'; (folder/'nested').mkdir(parents=True)
    (folder/'nested/hello.txt').write_text('Archive content: hello\n')
    (folder/'second.txt').write_text('second\n')
    with zipfile.ZipFile(root/'z-sample.zip','w',zipfile.ZIP_DEFLATED) as archive:
        archive.writestr('a-folder/nested/hello.txt','Archive content: hello\n')
        archive.writestr('a-folder/second.txt','second\n')
    return root

def exercise(root,key,type_text,click,capture,wait_for):
    # Create using native shortcut, format button and confirm button.
    key('a','ctrl','alt'); click(69,27); capture('create')
    click(58,29)
    packed=root/'a-folder.tar.gz'
    wait_for(packed.is_file,'native create TAR.GZ')
    time.sleep(.3)
    with tarfile.open(packed) as archive:
        assert archive.extractfile('a-folder/nested/hello.txt').read()==b'Archive content: hello\n'
    # Address bar and mouse double-clicks browse ordinary rows inside the ZIP.
    key('l','ctrl');key('u','ctrl');type_text(str(root/'z-sample.zip'));key('Return');time.sleep(.5)
    click(40,11);click(40,11);time.sleep(.4)
    click(40,11);click(40,11);time.sleep(.4)
    capture('browse')
    # Only Open and Extract apply to a member. Extract via the contextual mouse row.
    click(40,11,3);capture('context');click(43,13);time.sleep(.2)
    output=root/'selected';key('u','ctrl');type_text(str(output));capture('extract');click(58,29)
    wait_for(lambda:(output/'hello.txt').is_file(),'native selected archive member extraction')
    assert sorted(p.name for p in output.iterdir())==['hello.txt']
    assert (output/'hello.txt').read_text()=='Archive content: hello\n'
    time.sleep(.4);key('z','ctrl');wait_for(lambda:not output.exists(),'archive extraction undo')
    time.sleep(.3);key('y','ctrl');wait_for(lambda:(output/'hello.txt').is_file(),'archive extraction redo')
    capture('done')
    print('Archive GUI: native shortcut, format/confirm mouse buttons, ZIP folder double-clicks, contextual selected extraction and undo/redo passed',flush=True)
