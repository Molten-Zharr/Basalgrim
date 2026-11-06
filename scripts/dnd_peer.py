#!/usr/bin/env python3
"""Independent GTK4 copy-only drag peer for private native smoke displays."""
import gi
gi.require_version('Gtk','4.0')
from gi.repository import Gtk,Gdk,Gio,GObject,GLib
from pathlib import Path
import shutil
import sys

source=Path(sys.argv[1]);destination=Path(sys.argv[2])
Gtk.init()
loop=GLib.MainLoop()
window=Gtk.Window(title='Basalgrim · GTK DnD peer')
window.set_default_size(800,860)
label=Gtk.Label(label='GTK file drag peer\n\nDrag from here / drop here (Copy)')
label.set_hexpand(True);label.set_vexpand(True);window.set_child(label)
drag=Gtk.DragSource();drag.set_actions(Gdk.DragAction.COPY)
def prepare(*_):
    files=[Gio.File.new_for_path(str(p)) for p in sorted(source.iterdir()) if p.is_file()]
    value=GObject.Value();value.init(Gdk.FileList);value.set_boxed(Gdk.FileList.new_from_list(files))
    return Gdk.ContentProvider.new_for_value(value)
drag.connect('prepare',prepare);label.add_controller(drag)
drag.connect('drag-end',lambda *_:print('DRAG_FINISHED',flush=True))
drop=Gtk.DropTarget.new(Gdk.FileList,Gdk.DragAction.COPY)
def receive(_target,value,_x,_y):
    paths=[p.get_path() for p in value.get_files()]
    if not paths or any(p is None or not Path(p).is_file() or (destination/Path(p).name).exists() for p in paths):return False
    for path in paths:
        with open(path,'rb') as reader, (destination/Path(path).name).open('xb') as writer:shutil.copyfileobj(reader,writer)
    print('COPIED',len(paths),flush=True)
    return True
drop.connect('drop',receive);label.add_controller(drop)
window.connect('close-request',lambda *_:loop.quit())
window.present();GLib.idle_add(lambda:print('READY',flush=True))
loop.run()
