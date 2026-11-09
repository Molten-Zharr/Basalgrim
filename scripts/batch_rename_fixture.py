"""Bulk rename through real GUI/PTY input and private on-disk fixtures."""
import time

EXTENSIONS = ('jpg', 'jpg', 'png')


def prepare(base):
    source=base/'batch-files';source.mkdir()
    for i,extension in enumerate(EXTENSIONS):
        (source/f'file-{i:03}.{extension}').write_text(f'data {i}')
    return source


def exercise(source,key,insert,click,capture,wait_for,resize=None):
    def original():return all((source/f'file-{i:03}.{ext}').exists() for i,ext in enumerate(EXTENSIONS))
    def renamed():return all((source/f'Фото_{i+1:03}.{ext}').exists() for i,ext in enumerate(EXTENSIONS))
    for grid in (False,True):
        w,h=170,53
        if grid:
            key('g','ctrl');time.sleep(.2)
            if resize:
                w,h=34,14;resize(w,h);time.sleep(.25)
        left=(w-min(w,84))//2+1;top=(h-min(h,16))//2+1
        bottom=top+min(h,16)-3
        mode='grid' if grid else 'list'
        key('a','ctrl');key('F2');time.sleep(.25)
        capture('batch-open-'+mode)
        click(left+3,top+1)
        capture('batch-field-'+mode)
        insert('Фото');time.sleep(.3)
        capture('batch-name-'+mode)
        click(left+3,top+3);time.sleep(.3);key('Return')
        capture('batch-conflict-'+mode)
        assert original() and not (source/'Фото.jpg').exists(),'duplicate names must block the batch'
        click(left+3,top+3);time.sleep(.3)  # Enabling numbers must preserve the entered name.
        click(left+3,top+4);time.sleep(.2)  # Optional advanced mode retains its own input.
        key('a','ctrl');insert('advanced_{full}');time.sleep(.2)
        click(left+3,top+4);time.sleep(.3)
        conflict=source/'Фото_002.jpg';conflict.write_text('external occupant')
        click(left+21,top+4);time.sleep(.3);key('Return')
        assert original() and conflict.read_text()=='external occupant','occupied destination was overwritten'
        conflict.unlink();click(left+21,top+4);time.sleep(.3)
        capture('batch-preview-'+mode)
        click(left+5,bottom)
        wait_for(renamed,mode+' mouse apply')
        assert not any((source/f'file-{i:03}.{ext}').exists() for i,ext in enumerate(EXTENSIONS))
        for i,ext in enumerate(EXTENSIONS):assert (source/f'Фото_{i+1:03}.{ext}').read_text()==f'data {i}'
        time.sleep(.25);key('z','ctrl');wait_for(original,mode+' batch undo')
        time.sleep(.25);key('y','ctrl');wait_for(renamed,mode+' batch redo')
        time.sleep(.25);key('z','ctrl');wait_for(original,mode+' restore fixture')
        for pattern,suffixes,label in (
            ('Photo_{n:03}',EXTENSIONS,'preserve'),
            ('Photo_{n:03}{ext:jpg}',('jpg',)*3,'set'),
            ('Photo_{n:03}{ext:}',('',)*3,'remove'),
        ):
            time.sleep(.25);key('a','ctrl');key('F2');time.sleep(.2)
            click(left+3,top+4);time.sleep(.2)
            key('a','ctrl');insert(pattern);time.sleep(.3)
            capture('batch-extension-'+label+'-'+mode)
            expected=[source/f'Photo_{i+1:03}{"."+ext if ext else ""}' for i,ext in enumerate(suffixes)]
            click(left+5,bottom)
            wait_for(lambda:all(p.exists() for p in expected),mode+' extension '+label)
            for i,path in enumerate(expected):assert path.read_text()==f'data {i}'
            time.sleep(.25);key('z','ctrl');wait_for(original,mode+' extension '+label+' undo')
        time.sleep(.25);key('a','ctrl');key('F2');time.sleep(.2)
        click(left+3,top+4);time.sleep(.2)
        key('a','ctrl');insert('Снимок_{n:03}{jpg}');time.sleep(.3);key('Return')
        capture('batch-template-error-'+mode)
        assert original() and not any(p.name.startswith('Снимок_') for p in source.iterdir()),'bad template must not rename anything'
        click(left+4,bottom-1);time.sleep(.2)
        capture('batch-template-hint-'+mode)
        key('a','ctrl');insert('Снимок_{n:03}{ext:jpg}');time.sleep(.3)
        capture('batch-template-fixed-'+mode)
        click(left+5,bottom)
        wait_for(lambda:all((source/f'Снимок_{i+1:03}.jpg').exists() for i in range(3)),mode+' corrected custom template apply')
        for i in range(3):assert (source/f'Снимок_{i+1:03}.jpg').read_text()==f'data {i}'
        time.sleep(.25);key('z','ctrl');wait_for(original,mode+' corrected template undo')
        time.sleep(.25);key('a','ctrl');key('F2');time.sleep(.2)
        key('a','ctrl');insert('cancelled_{full}');time.sleep(.25)
        click(left+22,bottom)
        assert original() and not any(p.name.startswith('cancelled_') for p in source.iterdir())
        print(f'Batch rename {mode}: plain Фото input, implicit extensions, explicit extension change/removal, duplicate/occupied preview, exact Снимок_{{n:03}}{{jpg}} error, scrollable hints, mouse apply/cancel and undo/redo passed',flush=True)
