egui-winit 0.34.3, from the published crate (MIT OR Apache-2.0).

Local change in `State::on_keyboard_input`: emit the original pressed Key
event, including physical key and event-local modifiers, before Cut/Copy/Paste.
Upstream consumes those keys, including Ctrl+V when the OS clipboard is empty.
Basalgrim needs them for its configurable file commands and shortcut capture.
The semantic events remain available for text paste; the GUI dispatches file
commands only through Key, so an operation cannot run twice.

For Unicode logical characters, physical fallback is limited to ASCII character
keys. Virtual/remapped keyboards may place Cyrillic text on Escape/Enter/function
scancodes; dispatching those fallback commands cancels or submits an editor before
its Text event arrives. Logical named keys still work, and non-Latin layouts keep
their physical letter shortcuts (Ctrl+C/X/V, etc.). The Wayland bulk-rename smoke
types `Фото` through wtype to cover this case.

Recheck this patch when updating eframe/egui. Native regression:
`python3 scripts/smoke_gui.py target/debug/basalgrim`.
`python3 scripts/smoke_gui_wayland.py target/debug/basalgrim --batch-rename-only`.
