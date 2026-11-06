# Local winit extension

Based on winit 0.30.13 (Apache-2.0, bundled LICENSE). X11 windows may delegate
XDND to a protocol worker through a validated `XdndProxy` property. Forward
Enter/Position/Drop/Leave messages arriving at such a window to that proxy,
preserving the original target window and data. GTK 4.14 sends later motion
messages to the original window after its first proxy lookup; forwarding keeps
the operation in the application's worker and avoids a second, inconsistent
Copy/Private handler in winit. Windows without a valid proxy retain upstream
behavior. No new public API or Wayland behavior is introduced.
