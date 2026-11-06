"""Shared range-selection scenarios for private X11, Wayland and plain PTY input."""
import time


def prepare(base):
    source = base / 'range-source'
    source.mkdir()
    for i in range(100):
        (source / f'file-{i:03}.txt').write_text(f'content {i}')
    return source


def exercise(base, source, key, navigate, click, wheel, capture, wait_for, wheel_steps=1):
    check_number = 0

    def verify(indices, label):
        nonlocal check_number
        check_number += 1
        target = base / f'range-result-{check_number}'
        target.mkdir()
        key('c', 'ctrl');time.sleep(.15)
        navigate(target);key('v', 'ctrl')
        expected = {f'file-{i:03}.txt' for i in indices}
        wait_for(lambda:expected <= {p.name for p in target.iterdir()}, label)
        time.sleep(.15)
        assert {p.name for p in target.iterdir()} == expected, (label, sorted(p.name for p in target.iterdir()), sorted(expected))
        for i in indices:
            assert (target / f'file-{i:03}.txt').read_text() == f'content {i}'
        navigate(source);key('Home')

    for grid in (False, True):
        if grid:
            key('g', 'ctrl');time.sleep(.25)
        columns = 8 if grid else 1  # Real shared 170x53 layout, preview disabled.
        mode = 'grid' if grid else 'list'

        def entry(index, *mods):
            cx, cy = (34 + (index % 8) * 18, 12 + (index // 8) * 5) if grid else (34, 11 + index)
            click(cx, cy, *mods)

        key('Home');key('Down')
        key('Down', 'shift');key('Down', 'shift');key('Up', 'shift');key('Home', 'shift')
        capture('range-keyboard-' + mode)
        verify(range(columns + 1), mode + ' keyboard range reverses around its anchor')

        entry(2);entry(6, 'shift');entry(3, 'shift')
        entry(7, 'ctrl');entry(4, 'ctrl', 'shift');entry(6, 'ctrl', 'shift')
        capture('range-additive-' + mode)
        verify([2, 3, 6, 7], mode + ' Ctrl+Shift contracts without losing the original set')

        entry(2);entry(4, 'shift')
        for _ in range(1 if grid else 3):
            wheel(50, 20)
        time.sleep(.4)  # Wait for the GUI's accumulated wheel input before clicking.
        capture('range-scrolled-' + mode)
        # The wheel leaves focus/anchor at their original paths. Shift-click the
        # first newly visible file, spanning rows outside the viewport as well.
        entry(0, 'shift')
        verify(range(2, (8 if grid else 9) * wheel_steps + 1), mode + ' offscreen anchor after wheel scrolling')

        entry(0, 'ctrl');entry(7, 'ctrl')
        key('Home', 'ctrl', 'shift');key('Down', 'ctrl', 'shift')
        verify([0, 7, 8] if grid else range(8), mode + ' Ctrl+Shift keyboard union')
        print(f'Range selection {mode}: keyboard, mouse, shrink/reverse, Ctrl+Shift, wheel and exact copied files passed', flush=True)

    # A range click must not count as the first half of a double-click. Verify
    # this with folders: accidental opening would copy the marker, not the folder.
    folders = base / 'range-folders'
    for name in ('first', 'second'):
        (folders/name).mkdir(parents=True)
        (folders/name/'marker.txt').write_text(name)
    navigate(folders)
    click(34,12);click(52,12,'shift');click(52,12)
    key('c','ctrl');time.sleep(.15)
    target=base/'range-folder-result';target.mkdir()
    navigate(target);key('v','ctrl')
    wait_for(lambda:(target/'second/marker.txt').exists(),'range click must not accidentally open a folder')
    assert {p.name for p in target.iterdir()} == {'second'}
    assert (target/'second/marker.txt').read_text() == 'second'
