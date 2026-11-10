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
    click(34, 11, 3)
    capture("context")
    click(36, 19)
    wait_for(lambda: len(rows(base, 1)) == 1, "context-menu add to favorites")
    key("b", "ctrl")
    time.sleep(.3)
    click(48, 35)  # Add current folder.
    wait_for(lambda: len(rows(base, 0)) == 1, "mouse add bookmark")
    time.sleep(.3)
    click(47, 16)
    click(63, 35)  # Rename its label, not the directory.
    key("u", "ctrl")
    type_text("Work shelf")
    key("Return")
    wait_for(lambda: rows(base, 0)[0][1] == "Work shelf", "bookmark label persisted")
    time.sleep(.3)
    capture("bookmarks")
    click(47, 17)  # Imported bookmark: Remove must be a working mouse command.
    capture("imported")
    click(66, 34)
    wait_for(lambda: bytes(base / "Imported") in hidden_bookmarks(base), "hide imported bookmark")
    click(59, 36)  # Refresh must not resurrect it.
    time.sleep(.3)
    capture("hidden-bookmark")
    assert (base / "Imported").is_dir()
    click(57, 15)  # Favorites tab.
    time.sleep(.3)
    capture("favorites")
    click(47, 34)  # Open via private xdg-open peer.
    wait_for(lambda: (base / "opened.txt").exists(), "open favorite in external application")
    assert (base / "opened.txt").read_text() == str(root / "note.txt")
    wait_for(lambda: len(rows(base, 2)) == 1, "successful open added to recent")
    key("b", "ctrl")
    click(68, 15)
    time.sleep(.3)
    capture("recent")
    click(56, 34)  # Reveal in containing directory.
    time.sleep(.4)
    key("b", "ctrl")
    click(68, 15)
    time.sleep(.2)
    click(66, 34)  # Forget only the recent record.
    wait_for(lambda: len(rows(base, 2)) == 0, "forget recent preserves file")
    assert (root / "note.txt").read_text() == "personal places proof\n"
    assert (base / "config/gtk-3.0/bookmarks").read_bytes() == gtk
    assert (base / "data/recently-used.xbel").read_bytes() == xbel
    key("Escape")
    print("Places GUI: contextual favorite, mouse bookmark creation/label, hide imported bookmark, favorites open, successful-open recent, reveal and forget; GTK inputs preserved", flush=True)
