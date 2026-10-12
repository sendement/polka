# Polka

A gamepad-first file manager for Linux, written in Rust (egui).
Everything works from a controller: browsing, selection, file operations,
renaming via an on-screen keyboard. Keyboard and mouse work too.

- Classic layout: collapsible sidebar (places, pinned folders, drives), path bar, tabs
- List and grid views, dual-pane mode
- Archives work like folders (libarchive): browse zip, 7z, rar, tar.*, iso…; copy, move, rename,
  delete and create folders inside zip, 7z and tar.* (rar/iso are read-only)
- Built-in archiver: zip, tar.zst, tar.gz, tar.xz; smart extract
- Name clashes ask: keep both, replace (folders merge), skip
- Quick filter (live, as you type) and recursive search
- Settings (≡ → Settings…): highlight colour from presets or your own saved mixes, icon sizes for
  the sidebar, list and grid, interface scale, icon theme with live preview
- Top bar icons come from the icon theme (monochrome ones are recoloured to fit the UI)
- Image and video thumbnails, shared with other file managers via `~/.cache/thumbnails`
- Icons from the current icon theme, `.desktop` files and Windows `.exe`/`.dll` resources
- Pinned folders can use any theme icon (Papirus colour variants included) or your own picture
  (select an image, △ → "Use as pinned folder icon"); label sizes follow icon sizes

## Controls

| Gamepad (PlayStation / Xbox) | Action | Keyboard |
|---|---|---|
| D-pad / left stick | Move | Arrows |
| ✕ / A | Open folder, run file | Enter |
| ○ / B | Back (clears selection first) | Backspace, Esc |
| □ / X | Select; hold and move for a range (moving back shrinks it; arrows stay in the pane) | Space, Insert, Shift+arrows |
| △ / Y | Actions menu | Shift+F10 |
| Options / Menu | App menu | F10 |
| Create / View | Toggle dual pane | F3 |
| L1 / R1 | Previous / next tab | Ctrl+Tab, Ctrl+PgUp/PgDn |
| L2 / R2 | Left / right pane (dual), page up / down (single) | Tab, PgUp/PgDn |
| L3 | Collapse sidebar | Ctrl+B |
| — | History back / forward | Alt+← → |

The right stick and the touchpad are intentionally left unbound.
Menus: → or ✕/A opens a submenu (View, Sorting), ← or ○/B goes back; toggles keep the menu open.

Filter: app menu → Filter… or Ctrl+F (○/B clears it). Search in subfolders: app menu or Ctrl+Shift+F.

On-screen keyboard: ✕/A types the key, □/X erases, △/Y inserts a space, L1/R1 move
the caret, L2 toggles shift, Create/View switches layout (EN/RU), Options/Menu confirms.

Gamepad input is ignored while Polka is not the focused window.

## Build

Needs Rust, libarchive and zstd (`pacman -S libarchive zstd`).

```sh
cargo build --release
./target/release/polka [DIR | FILE | ARCHIVE | file://URI]
```

Arch package: `packaging/arch/make-tarball.sh`, then `makepkg` in `packaging/arch`
(installs the binary, desktop entry and icon).
Video thumbnails need `ffmpegthumbnailer`.

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
