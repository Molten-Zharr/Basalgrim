"""Shared assertions for real X11/Wayland text input on private displays."""
import time
import hashlib
import sqlite3


def exercise_rename_refresh(folder, key, type_text, capture, wait_for, journal):
    """Sample native pixels throughout a directory rename, not just afterwards."""
    # Keep the renamed directory first so sorting cannot legitimately move its
    # neighbours. Enough entries make the asynchronous scan observable.
    current = folder / 'a-folder'
    current.mkdir()
    for index in range(3000):
        (folder / f'file-{index:04}.txt').touch()
    key('l', 'ctrl');key('u', 'ctrl');type_text(str(folder));key('Return')
    time.sleep(.7)
    for grid in (False, True):
        if grid:
            key('g', 'ctrl');time.sleep(.3)
        key('Home');key('F2');type_text('a-renamed' if not grid else 'a-folder')
        time.sleep(.1)
        name = 'rename-refresh-grid' if grid else 'rename-refresh-list'
        # Shared 170x53 layout, 8x16 pixel cells. Check both the unchanged
        # neighbours and the edited entry, including the worker/scan interval.
        region = (25 * 8, 16 * 16, 169 * 8, 46 * 16)
        entry = (25 * 8, 10 * 16, 43 * 8, 15 * 16) if grid else (25 * 8, 11 * 16, 169 * 8, 12 * 16)
        digest = lambda frame: hashlib.sha256(frame.crop(entry).tobytes()).digest()
        editing = capture(name + '-before')
        before = editing.crop(region).tobytes()
        samples = 0
        entry_frames = []
        def sample(seconds):
            nonlocal samples
            until = time.monotonic() + seconds
            while time.monotonic() < until:
                frame = capture(None)
                if frame.crop(region).tobytes() != before:
                    capture(name + '-failure')
                    raise AssertionError(f'{name}: unchanged file area flashed or moved')
                entry_frames.append(digest(frame))
                samples += 1
        # Hold only this fixture's private journal. Force several visible
        # pending frames even on machines where renaming itself is instant.
        assert journal.exists()
        with sqlite3.connect(journal) as barrier:
            barrier.execute('BEGIN IMMEDIATE')
            key('Return')
            sample(.25)
            assert current.exists(), 'rename mutated data before its journal intent'
            barrier.commit()
        sample(.7)
        destination = folder / ('a-renamed' if not grid else 'a-folder')
        wait_for(destination.is_dir, 'folder renamed without clearing its pane')
        assert not current.exists()
        current = destination
        final = digest(capture(name + '-after'))
        # An unprocessed Enter may leave the last editor frame on screen.
        # There must be just one transition to the new label, never an old-name
        # frame between closing the editor and receiving the directory scan.
        normal_seen = False
        for frame in entry_frames:
            if frame == final:
                normal_seen = True
            else:
                assert not normal_seen and frame == digest(editing), f'{name}: renamed entry flashed an intermediate label'
        assert normal_seen, f'{name}: final label never appeared'
        assert samples >= 5, f'Not enough intermediate frames: {samples}'
        print(f'Native rename grid={grid}: {samples} frames kept neighbours stable and the renamed entry changed directly to its final label', flush=True)


def exercise(folder, target, key, type_text, put, get, click, drag, capture, wait_for):
    def navigate(path):
        key('l', 'ctrl');key('u', 'ctrl');type_text(str(path));key('Return');time.sleep(.25)

    key('Home');key('F2');type_text('renamed');key('Return')
    wait_for(lambda:(folder/'renamed.txt').exists(),'inline stem rename')
    assert (folder/'renamed.txt').read_text()=='editor data'
    time.sleep(.3)
    key('c','ctrl');time.sleep(.2)  # Retain the file clipboard through later text copies.
    key('F2');capture('inline-rename')
    # 170x53, Fira Code 16px, image icons: first file name starts at cell 31,11.
    drag(32,11,35,11);key('c','ctrl')
    wait_for(lambda:get()=='ena','mouse text range copied exactly once')
    key('x','ctrl');time.sleep(.15)
    key('Home');key('End','shift');key('c','ctrl')
    wait_for(lambda:get()=='rmed.txt','cut removes only the selected text')
    key('Escape');navigate(target);key('v','ctrl')
    wait_for(lambda:(target/'renamed.txt').exists(),'text copy retained internal file clipboard')
    assert (target/'renamed.txt').read_text()=='editor data'
    time.sleep(.3);navigate(folder);key('Home');key('F2')
    put('отчёт 画像');time.sleep(.2)
    click(34,11,3);capture('text-edit-menu');click(36,14)
    time.sleep(.35);key('Return')
    final=folder/'отчёт 画像.txt'
    wait_for(final.exists,'mouse context-menu Unicode paste into filename')
    assert final.read_text()=='editor data'
    time.sleep(.3);key('F2');key('a','ctrl');key('c','ctrl')
    wait_for(lambda:get()=='отчёт 画像.txt','Unicode filename text copy')
    key('Escape');key('l','ctrl');key('Home');key('Right')
    for _ in range(3):key('Right','shift')
    key('c','ctrl');wait_for(lambda:get()=='tmp','path Shift selection')
    key('a','ctrl');put(str(target));time.sleep(.2);key('v','ctrl');time.sleep(.3)
    capture('path-selection');key('Return');time.sleep(.2)
    put('');time.sleep(.2);key('F2');key('v','ctrl');time.sleep(.3)
    key('a','ctrl');key('c','ctrl')
    wait_for(lambda:get()=='renamed.txt','empty clipboard leaves editor text intact')
    type_text('cancelled');key('Escape')
    assert (target/'renamed.txt').exists() and not (target/'cancelled.txt').exists()
