# egui-winit 0.36.2, patched for Polka

A copy of [egui-winit](https://github.com/emilk/egui) (MIT OR Apache-2.0,
Copyright (c) 2018-2021 Emil Ernerfeldt and the egui contributors), used
through `[patch.crates-io]` in Polka's Cargo.toml.

Two changes. In `src/clipboard.rs`: on Wayland the application may
supply the clipboard itself (`external_wayland()`, `set_external()`)
instead of egui-winit opening a clipboard data device of its own. Some
compositors (Hyprland) deliver data device events to one device per client,
so a second device would keep drops from reaching Polka's drag and drop.

In `src/lib.rs`: Ctrl+V gives an `Event::Paste` even when the clipboard has
no text (an empty one), so Polka's own clipboard of files pastes on Ctrl+V.
