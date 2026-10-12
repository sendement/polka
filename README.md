# Polka

A gamepad-first file manager for Linux, written in Rust (egui).
Everything works from a controller: browsing, selection, file operations,
renaming via an on-screen keyboard. Keyboard and mouse work too.

- Classic layout: collapsible sidebar (places, pinned folders, drives), path bar, tabs
- List and grid views, dual-pane mode
- Archives open like folders (zip, 7z, rar, tar.*, iso… via libarchive); copy files out of them
- Built-in archiver: zip, tar.zst, tar.gz, tar.xz; smart extract
- Icons from the current icon theme, `.desktop` files and Windows `.exe`/`.dll` resources

## Controls

| Gamepad (PlayStation / Xbox) | Action | Keyboard |
|---|---|---|
| D-pad / left stick | Move | Arrows |
| ✕ / A | Open folder, run file | Enter |
| ○ / B | Back (clears selection first) | Backspace, Esc |
| □ / X | Select; hold and move to select a range | Space, Insert |
| △ / Y | Actions menu | Shift+F10 |
| Options / Menu | App menu | F10 |
| Create / View | Toggle dual pane | F3 |
| L1 / R1 | Previous / next tab | Ctrl+Tab, Ctrl+PgUp/PgDn |
| L2 / R2 | Left / right pane (dual), page up / down (single) | Tab, PgUp/PgDn |
| L3 | Collapse sidebar | Ctrl+B |
| R3 | Properties | |
| Right stick ↔ / ↕ | History back/forward / fast scroll | Alt+← → |

On-screen keyboard: ✕/A types the key, □/X erases, △/Y inserts a space, L1/R1 move
the caret, L2 toggles shift, Create/View switches layout (EN/RU), Options/Menu confirms.

Gamepad input is ignored while Polka is not the focused window.

## Build

Needs Rust and libarchive (`pacman -S libarchive`).

```sh
cargo build --release
./target/release/polka [DIR]
```

Settings live in `~/.config/polka/config.toml` (pinned folders, view, sort,
`icon_theme` override, `zoom` for TV use; Ctrl+= / Ctrl+- also zoom).
`POLKA_NO_GAMEPAD=1` disables controller input.

## Tests

```sh
cargo test
# offscreen UI renders (needs a GPU):
POLKA_TEST_DIR=/some/dir POLKA_SHOT_DIR=/tmp/shots cargo test -- --ignored screens
```

`tools/fakepad.py` creates a virtual Xbox pad via uinput for manual input testing
(`examples/padtest.rs` prints what Polka receives).

## License

MIT
