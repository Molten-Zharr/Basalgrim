# Basalgrim

Basalgrim is a native Linux GUI/TUI file manager for Molten-Zharr.

- Rust 2024; Ratatui is the single renderer for GUI and TUI. GUI uses eframe/egui_ratatui/soft_ratatui; TUI uses Crossterm. Keep GUI dependencies behind `guitui` and terminal dependencies behind `tui`.
- Share behavior through `App`, `Action`, and `Input`. Register hit regions in the same renderer that draws their controls. Don't duplicate filesystem logic in either frontend.
- Hover is separate from selection and keyboard focus. Resolve the topmost current hit region after modal rendering; tint only its background and preserve selected backgrounds. Pointer movement must not navigate, change selection, mark files, or request previews. Clear hover when the pointer leaves or the terminal loses focus.
- Use `assets/brand/molten-zharr.json` for palette roles and the default brand colors. Appearance has four Almavorn palettes plus per-theme color overrides; build a Palette from the current Settings, without global mutable theme state. Fira Code and DejaVu Sans Mono licenses are bundled; terminal fonts remain terminal-controlled.
- Settings share one Ratatui screen. Persist all command bindings, match normalized modifiers exactly, and capture a new shortcut before dispatching actions. Keep navigation keys reserved, reject conflicts, and derive command/help labels from the active bindings. Keep the legacy JSON font_size key readable through flattened Appearance.
- Read directories, previews, and operations in workers. Validate request IDs before accepting asynchronous results.
- Never overwrite existing destinations implicitly. Keep no-replace publication, staged copies, non-following symlink copies, rejection of recursive destinations, and accurate partial-operation reports.
- Trash uses `gio trash` without permanent-delete fallback. A test must isolate its trash directory; never empty the user's trash.
- Verify with `cargo fmt --check`, strict all-feature Clippy, all-feature tests, and TUI-only tests. `cargo run --example preview` renders the real shared interface without a display.
- Keep generated `target/` and `graphify-out/` local. The shared Harness stays at its permanent path. Do not install repository hooks unless the user explicitly connects the repository.
