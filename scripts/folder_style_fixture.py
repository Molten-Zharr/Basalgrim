"""Folder styling through real property controls on X11, Wayland and a PTY."""
import json
import sqlite3
import time

from PIL import Image
from details_fixture import Fixture as DesktopFixture
from places_fixture import rows
from views_fixture import wait

class Fixture(DesktopFixture):
    def __init__(self, base, view='icons'):
        super().__init__(base)
        self.file.unlink()
        self.folder = self.root / 'Проекты'
        self.folder.mkdir()
        for name in ('Справочники', 'Шаблоны'):
            (self.root / name).mkdir()
        self.image = base / 'green icon.png'
        Image.new('RGBA', (24, 24), (21, 223, 103, 255)).save(self.image)
        settings = base / 'config/molten-zharr/basalgrim/settings.json'
        settings.parent.mkdir(parents=True, exist_ok=True)
        settings.write_text(json.dumps(dict(view=view, preview=False, sidebar_mode='hidden',
            theme='molten-zharr', icon_mode='images', thumbnails=False, font_size=16)))

    def stored(self):
        db = self.base / 'data/molten-zharr/basalgrim/places.sqlite3'
        if not db.exists():
            return None
        with sqlite3.connect(f'file:{db}?mode=ro', uri=True) as con:
            row = con.execute('SELECT style FROM folder_styles').fetchone()
        return json.loads(row[0]) if row else None

def favorite_menu(f, key, right_click, grab):
    # The places worker reads its catalog every three seconds. Inspect the open
    # menu itself across two reads, including the space around its last row.
    for add in (True, False):
        right_click(10, 11)
        key('End')
        time.sleep(.2)
        region = (10 * 8, 11 * 16, 50 * 8, 30 * 16)
        expected = grab().crop(region).tobytes()
        deadline = time.monotonic() + 6.2
        while time.monotonic() < deadline:
            assert grab().crop(region).tobytes() == expected, 'open folder menu changed during refresh'
            time.sleep(.01)
        key('Return')
        wanted = [bytes(f.folder)] if add else []
        wait(lambda: [r[0] for r in rows(f.base, 1)] == wanted, 'contextual favorite toggle')


def exercise(f, key, text, click, shot, cells=(170,53)):
    w,h = cells
    sx,sy = (w-min(w,56))//2+1, (h-min(h,14))//2+1
    px,py = (w-min(w,76))//2+1, (h-min(h,22))//2+1
    def open_style(check_properties=False):
        key('Return','alt')
        time.sleep(.35)
        if check_properties:
            general = shot('properties')
            key('F4')
            time.sleep(.2)
            permissions = shot('permissions')
            if isinstance(general, str):
                assert 'Оформление' in general and 'Права' in general, general
                assert 'Сохранить' in permissions, permissions
                for label in ('Права F4', 'Оформление F2', 'Сохранить · Ctrl+S'):
                    assert label not in general and label not in permissions, label
            key('F4')
            time.sleep(.2)
        click(px+17, py+min(h,22)-4)
        time.sleep(.25)

    open_style(check_properties=True)
    click(sx+1, sy+3)  # Club preset.
    click(sx+11, sy+5)  # Green swatch.
    click(sx+8, sy+6)
    text('Работа')
    shot('editor')
    click(sx+3, sy+min(h,14)-3)
    wait(lambda: f.stored() and f.stored()['tag'] == 'Работа', 'saved tag')
    assert f.stored()['icon'] == '♣' and f.stored()['color'] == [94,183,126]
    time.sleep(.25)
    screen = shot('symbol')
    if isinstance(screen, str):
        assert '♣' in screen and '#Работа' in screen, screen

    open_style()
    text('Ж')
    key('Return')
    wait(lambda: f.stored()['icon'] == 'Ж', 'typed symbol')
    time.sleep(.25)
    open_style()
    text(str(f.image))
    key('Return')
    wait(lambda: f.stored()['icon'] == str(f.image), 'imported PNG')
    f.image.unlink()
    time.sleep(.35)
    screen = shot('image')
    if not isinstance(screen, str):
        pixels = screen.convert('RGB').crop((0, 8*16, 220, 16*16))
        assert sum(p == (21,223,103) for p in pixels.getdata()) > 128, 'imported icon pixels'
    assert list(f.folder.iterdir()) == []

def reset(f, key, click, shot, cells=(170,53)):
    w,h = cells
    sx,sy = (w-min(w,56))//2+1, (h-min(h,14))//2+1
    screen = shot('restart')
    if isinstance(screen, str):
        assert '#Работа' in screen, screen
    else:
        assert (21,223,103) in screen.convert('RGB').crop((0,8*16,220,16*16)).getdata()
    key('Return','alt')
    time.sleep(.3)
    key('F2')
    time.sleep(.2)
    click(sx+15,sy+min(h,14)-3)
    key('Return')
    wait(lambda: f.stored() is None, 'reset folder decoration')
    print('Folder style: presets, Unicode tag, imported PNG, restart and reset passed',flush=True)
