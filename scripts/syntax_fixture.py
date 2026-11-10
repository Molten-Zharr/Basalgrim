"""Shared fixtures and actual native pixels for code previews."""
import time

STRING = (72, 106, 99)
KEYWORD = (110, 94, 123)

def prepare(base):
    source = base / 'syntax-files'
    source.mkdir()
    (source / 'a.rs').write_text('fn main() {\n    let greeting = "Hello world";\n    // comment\n}\n' * 35)
    (source / 'b.txt').write_text('Plain text after code\nfn main() { "No highlighting" }')
    return source

def count(image, color):
    # Only code rows, below the metadata header. Frontends use the same layout.
    return sum(pixel == color for pixel in image.convert('RGB').crop((1072, 300, 1352, 760)).getdata())

def exercise(source, key, click, capture, wait_for):
    wait_for(lambda: count(capture('current'), STRING) > 100 and count(capture('current'), KEYWORD) > 50,
             'native colored Rust preview')
    capture('code')
    # Real mouse selects the second row; file names resembling code stay plain.
    click(35, 12)
    wait_for(lambda: count(capture('plain'), STRING) == 0 and count(capture('plain'), KEYWORD) == 0,
             'plain text has no stale syntax colors')
    click(35, 11)
    wait_for(lambda: count(capture('return'), STRING) > 100, 'mouse returns to colored code')
    (source / 'a.rs').write_text('// rewritten file, comments only\n' * 35)
    wait_for(lambda: count(capture('rewrite'), STRING) == 0 and count(capture('rewrite'), KEYWORD) == 0,
             'watched rewrite clears previous syntax colors')
    (source / 'a.rs').write_text('fn main() {\n    let greeting = "Hello again";\n}\n' * 35)
    wait_for(lambda: count(capture('updated'), STRING) > 100, 'watched update restores string colors')
    key('F3'); time.sleep(.2)
    assert count(capture('off'), STRING) == 0, 'code remained after hiding preview'
    key('F3')
    wait_for(lambda: count(capture('on'), STRING) > 100, 'preview after F3')
    print('Syntax GUI: actual keyword/string pixels, mouse selection, plain text, watched rewrite and F3 passed', flush=True)
