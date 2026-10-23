egui-winit 0.34.3, from the published crate (MIT OR Apache-2.0).

Local change in `State::on_keyboard_input`: emit the original pressed Key
event, including physical key and event-local modifiers, before Cut/Copy/Paste.
Upstream consumes those keys, including Ctrl+V when the OS clipboard is empty.
Basalgrim needs them for its configurable file commands and shortcut capture.
The semantic events remain available for text paste; the GUI dispatches file
commands only through Key, so an operation cannot run twice.

Recheck this patch when updating eframe/egui. Native regression:
`python3 scripts/smoke_gui.py target/debug/basalgrim`.
