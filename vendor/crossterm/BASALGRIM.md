# Local Crossterm extension

Based on Crossterm 0.29.0 (MIT, bundled LICENSE). The opt-in `osc` feature adds
`Event::Osc(Vec<u8>)` for Kitty's OSC 72 drag-and-drop protocol. Both Unix readers
frame packets across reads, cap stored data at 8192 bytes and discard oversized
or unrelated OSC packets through the terminator. Protocol bytes never become
file commands or editable text. A split ESC prefix has a bounded 10 ms framing
wait, which also preserves standalone Escape with blocking event reads.
Existing keys, mouse events and bracketed paste
use the upstream parser. Basalgrim owns protocol negotiation and validation.
