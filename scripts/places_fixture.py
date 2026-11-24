"""Desktop-format inputs and native places actions, always in a private HOME/XDG."""
import json
import sqlite3
import time


def prepare(base):
    root = base / "places-source"
    root.mkdir()
    (root / "note.txt").write_text("personal places proof\n")
    (root / "old.txt").write_text("older recent file\n")
    target = base / "Imported"
    target.mkdir()
    gtk = base / "config/gtk-3.0/bookmarks"
    gtk.parent.mkdir(parents=True, exist_ok=True)
    gtk.write_text(base.as_uri() + " DUPLICATE HOME\nfile:/// DUPLICATE ROOT\n" + target.as_uri() + " Imported folder\n")
    data = base / "data"
    data.mkdir(exist_ok=True)
    (data / "recently-used.xbel").write_text(
        '<xbel xmlns:bookmark="http://www.freedesktop.org/standards/desktop-bookmarks">'
        f'<bookmark href="{(root / "old.txt").as_uri()}" visited="2026-01-01T10:00:00Z"><title>Old document</title></bookmark>'
        '<bookmark href="file:///private-hidden.txt"><info><metadata><bookmark:private/></metadata></info></bookmark></xbel>')
    stub = base / "bin/xdg-open"
    stub.parent.mkdir()
    stub.write_text('#!/usr/bin/python3\nimport os,sys\nfrom pathlib import Path\nPath(os.environ["BASALGRIM_OPEN_LOG"]).write_text(sys.argv[1])\n')
    stub.chmod(0o700)
    return root


def rows(base, section):
    path = base / "data/molten-zharr/basalgrim/places.sqlite3"
    try:
        with sqlite3.connect(f"file:{path}?mode=ro", uri=True) as db:
            return db.execute("SELECT path,label FROM places WHERE section=? ORDER BY rowid", (section,)).fetchall()
    except sqlite3.Error:
        return []


def hidden_bookmarks(base):
    path = base / "data/molten-zharr/basalgrim/places.sqlite3"
    with sqlite3.connect(f"file:{path}?mode=ro", uri=True) as db:
        return [row[0] for row in db.execute("SELECT path FROM hidden_bookmarks")]


def exercise(base, root, key, type_text, click, capture, wait_for):
    gtk = (base / "config/gtk-3.0/bookmarks").read_bytes()
    xbel = (base / "data/recently-used.xbel").read_bytes()
    # Favorite from the actual file context menu, not a direct App call.
    click(34, 10, 3)
    capture("context")
    click(36, 20)
    wait_for(lambda: len(rows(base, 1)) == 1, "context-menu add to favorites")
    click(8, 7)  # Open the shared list from the Quick Access heading.
    time.sleep(.3)
    click(48, 35)  # Add current folder.
    wait_for(lambda: len(rows(base, 0)) == 1, "mouse add bookmark")
    time.sleep(.3)
    click(47, 18)
    click(63, 35)  # Rename its label, not the directory.
    key("u", "ctrl")
    type_text("Work shelf")
    key("Return")
    wait_for(lambda: rows(base, 0)[0][1] == "Work shelf", "bookmark label persisted")
    time.sleep(.3)
    capture("bookmarks")
    click(47, 19)  # Imported bookmark: Remove must be a working mouse command.
    capture("imported")
    click(66, 34)
    wait_for(lambda: bytes(base / "Imported") in hidden_bookmarks(base), "hide imported bookmark")
    click(59, 36)  # Refresh must not resurrect it.
    time.sleep(.3)
    capture("hidden-bookmark")
    assert (base / "Imported").is_dir()
    click(63, 15)  # Favorites tab.
    time.sleep(.3)
    capture("favorites")
    click(47, 34)  # Open via private xdg-open peer.
    wait_for(lambda: (base / "opened.txt").exists(), "open favorite in external application")
    assert (base / "opened.txt").read_text() == str(root / "note.txt")
    wait_for(lambda: len(rows(base, 2)) == 1, "successful open added to recent")
    key("b", "ctrl")
    click(76, 15)
    time.sleep(.3)
    capture("recent")
    click(56, 34)  # Reveal in containing directory.
    time.sleep(.4)
    key("b", "ctrl")
    click(76, 15)
    time.sleep(.2)
    click(66, 34)  # Forget only the recent record.
    wait_for(lambda: len(rows(base, 2)) == 0, "forget recent preserves file")
    assert (root / "note.txt").read_text() == "personal places proof\n"
    assert (base / "config/gtk-3.0/bookmarks").read_bytes() == gtk
    assert (base / "data/recently-used.xbel").read_bytes() == xbel
    key("Escape")
    print("Places GUI: contextual favorite, mouse bookmark creation/label, hide imported bookmark, favorites open, successful-open recent, reveal and forget; GTK inputs preserved", flush=True)


def pin_entry(base, root, drag, wait_for):
    folder = root / "Pinned"
    folder.mkdir()
    time.sleep(.8)  # Let the directory watcher put the new folder in the first row.
    drag((34, 10), (9, 7))
    wait_for(lambda: (bytes(folder), "Pinned") in rows(base, 0), "drag folder onto Quick Access")
    assert folder.is_dir()


def prepare_quick_access_scene(base):
    path = base / "config/molten-zharr/basalgrim/settings.json"
    settings = json.loads(path.read_text())
    settings["theme"] = "light"
    path.write_text(json.dumps(settings))
    entries = {"DOWNLOAD": "Downloads", "DOCUMENTS": "Documents", "MUSIC": "Music", "PICTURES": "Pictures"}
    for folder in [*entries.values(), "projects"]:
        (base / folder).mkdir(exist_ok=True)
    (base / "config/user-dirs.dirs").write_text("".join(
        f'XDG_{key}_DIR="$HOME/{folder}"\n' for key, folder in entries.items()
    ))


def open_pin_after_restart(base, root, click, hover, key, type_text, drag, capture, wait_for):
    # Two rows per standard location, with seven locations in this private HOME.
    click(8, 25)  # Saved Pinned link below Work shelf.
    time.sleep(.3)
    key("F7")
    type_text("ZedMono")
    key("Return")
    folder = root / "Pinned/ZedMono"
    wait_for(folder.is_dir, "saved sidebar link opens its folder")
    sentinel = folder / "keep.txt"
    sentinel.write_text("keep")
    for y, remaining in [(25, 1), (23, 0)]:
        hover(8, y)
        time.sleep(.2)
        click(21, y)
        wait_for(lambda: len(rows(base, 0)) == remaining, "unpin directly from the sidebar")
        time.sleep(.2)

    def shot(name):
        hover(4, 7)  # Keep hover out of the measured rows.
        time.sleep(.2)
        return capture(name).convert("RGB")

    before = shot("empty")
    drag((34, 10), (9, 7))
    wait_for(lambda: rows(base, 0) == [(bytes(folder), "ZedMono")], "pin first bookmark")
    time.sleep(.3)
    pinned = shot("first-pin")
    standards = (8, 9*16, 23*8, 22*16)  # Seven rows, up to and including the filesystem root.
    assert before.crop(standards).tobytes() == pinned.crop(standards).tobytes(), "standard rows moved"
    hover(8, 23)
    time.sleep(.2)
    highlighted = capture("hover-pin").convert("RGB")
    cross = (21*8, 23*16, 22*8, 24*16)
    assert highlighted.crop(cross).tobytes() != pinned.crop(cross).tobytes(), "missing hover control"
    assert highlighted.crop(standards).tobytes() == pinned.crop(standards).tobytes()
    click(21, 23)
    wait_for(lambda: not rows(base, 0), "unpin ZedMono")
    time.sleep(.3)
    after = shot("unpinned")
    sidebar = (8, 9*16, 23*8, 39*16)
    assert before.crop(sidebar).tobytes() == after.crop(sidebar).tobytes(), "sidebar layout did not return"
    assert sentinel.read_text() == "keep"
    hover(8, 11)
    time.sleep(.2)
    click(21, 11)
    wait_for(lambda: bytes(base / "Downloads") in hidden_bookmarks(base), "unpin standard Downloads")
    time.sleep(.3)
    return shot("without-downloads").crop(sidebar)


def check_unpinned_restart(base, root, expected, screenshot, navigate, key, wait_for):
    sidebar = screenshot.convert("RGB").crop((8, 9*16, 23*8, 39*16))
    assert sidebar.tobytes() == expected.tobytes(), "unpinned folder reappeared after restart"
    assert not rows(base, 0) and (root / "Pinned/ZedMono/keep.txt").read_text() == "keep"
    downloads = base / "Downloads"
    assert downloads.is_dir() and bytes(downloads) in hidden_bookmarks(base)
    navigate(downloads)
    key("b", "ctrl", "alt")
    wait_for(lambda: bytes(downloads) not in hidden_bookmarks(base), "re-pin standard Downloads")
    assert not rows(base, 0), "restoring a standard folder created a duplicate"
    print("Quick Access GUI: hover to unpin, stable geometry, standard and personal folders hidden after restart, Downloads restored without a duplicate", flush=True)
