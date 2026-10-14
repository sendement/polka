# Polka

A gamepad-first file manager for Linux, written in Rust (egui).
Everything works from a controller: browsing, selection, file operations,
renaming via an on-screen keyboard. Keyboard and mouse work too.

- Classic layout: collapsible sidebar (places, pinned folders, drives), path bar, tabs
- List and grid views, dual-pane mode
- Archives work like folders (libarchive): browse zip, 7z, rar, tar.*, iso…; copy, move, rename,
  delete and create folders inside zip, 7z and tar.* (rar/iso are read-only)
- Built-in archiver: zip, tar.zst, tar.gz, tar.xz; smart extract
- Password-protected archives: zip (ZipCrypto/AES) via libarchive, 7z and rar via the `7z` tool;
  Polka asks for the password (hidden input) and retries. Passwords are kept in memory only
- Name clashes ask: keep both, replace (folders merge), skip
- Quick filter (live, as you type) and recursive search
- Settings (≡ → Settings…): highlight colour from presets or your own saved mixes, icon sizes for
  the sidebar, list and grid, interface scale, icon theme with live preview
- Top bar icons come from the icon theme (monochrome ones are recoloured to fit the UI)
- Image and video thumbnails, shared with other file managers via `~/.cache/thumbnails`
- Icons from the current icon theme, `.desktop` files and Windows `.exe`/`.dll` resources
- The on-screen keyboard is a real keyboard's main block with what Shift gives on every key (`!`
  on 1, `№` on 3 in Russian…), Caps Lock, and for the terminal Ctrl, Alt, Tab and arrows. Layouts:
  EN, RU, UK, DE (QWERTZ), FR (AZERTY), ES with dead keys for accents, Korean (두벌식, jamo build
  syllables) and Japanese kana (JIS; ゛ ゜ voice the kana before, Caps writes katakana); which ones ⇄
  goes through is chosen in the settings. (Chinese would need a dictionary IME.)
- Pinned folders can use any theme icon (Papirus colour variants included) or your own picture
  (icon picker → △ "Own picture…", then pick an image in the panes); label sizes follow icon sizes

## Built-in viewer and player

Files open in the same window, in place of the panes (each kind can be sent to its desktop app
instead: Settings → "Open in Polka"):

- Text: tree-sitter highlighting for 30 languages, themes and fonts, editing from the pad or
  keyboard, search and replace by words or regular expressions (Ctrl+H; `$1` groups); Markdown rendered as on GitHub
  (pictures from the web too, through `curl`; a switch in the settings keeps it offline) or as source;
  paste from the pad reads the system clipboard
- Pictures: zoom, pan, EXIF rotation, browsing the folder; animated X cursors with their hotspot
- Music: mp3, flac, ogg, m4a/aac, alac, wav, aiff and more (Symphonia); Opus, WMA, APE and other
  formats through `ffmpeg` if installed. Tags and cover art (embedded or `cover.jpg` next to the
  tracks), the folder as a playlist, repeat, volume. Music keeps playing in a background tab.

| Player | Gamepad | Keyboard |
|---|---|---|
| Play / pause | ✕ / A | Space, K |
| Seek 5 s (30 s) | ← → (□ held) | ← → (Shift) |
| Previous / next track | L2 / R2 | PgUp / PgDn, P / N |
| Playlist | ↑ ↓, then ✕ / A | ↑ ↓, Enter |
| Repeat: off / all / one | □ / X tap | R |
| Volume | □ / X held + ↑ ↓ | + / − |

- Video (needs `ffmpeg`): mp4, mkv, webm, avi, mov and the rest of what ffmpeg reads; sound in
  sync, audio track choice, subtitles (text ones inside the file, or `.srt`/`.ass`/`.vtt` next
  to it), full screen, the folder's videos one after another. Pauses when its tab is left.

| Video | Gamepad | Keyboard |
|---|---|---|
| Play / pause | ✕ / A, click | Space, K |
| Seek 10 s (60 s) | ← → (□ held) | ← → (Shift) |
| Volume | ↑ ↓ | ↑ ↓, + / − |
| Previous / next video | L2 / R2 | PgUp / PgDn, P / N |
| Full screen | Create / View, double click | F, F11 |
| Subtitles / audio track | □ / X tap / △ menu | S / A |

## Administrator rights, packages, scripts

Saving a system config, or copying, moving, renaming, deleting, linking or making files where only
root may: Polka asks for the sudo password (on-screen keyboard; shown as asterisks or fully
hidden, see Settings) and does it with sudo — `cp` keeps the file's owner and mode. A password
that worked is reused for ten minutes, in memory only. Files out of system folders are deleted
rather than trashed.

Package files are checked by what's inside (.PKGINFO, debian-binary, the rpm lead, squashfs…)
and marked with their format on the icon; a fake named like one stays a plain archive. With
"Install packages on open" (on by default) ✕ installs them; △ still opens them as folders.
△ on a package offers installing it with the system's own tool, when there is one: pacman
(`.pkg.tar.*`), flatpak (`.flatpak`, `.flatpakref`), apt / dpkg (`.deb`),
dnf / zypper / rpm (`.rpm`), snap (`.snap`); on a shell script, running it. It runs in a terminal
window that shows the command and takes the tool's questions and sudo's password; it closes when
done, and waits for Enter if something failed. Dropped items can also be linked ("Create link
here").

## Terminal

Sidebar → Terminal (or △ on a folder → "Open in terminal", ≡ → "Open terminal here") opens
shells in their own tabs inside the page. Built on alacritty's terminal core: colours, full-screen
programs (htop, btop, mc, vim, less…), mouse, scrollback; box drawing, blocks and braille are drawn
to the cell so tables and graphs join up. Settings: colour scheme, font, size, shell, cursor,
scrollback, start folder.

| Terminal | Gamepad | Keyboard |
|---|---|---|
| Arrows | D-pad | as usual |
| Type: the on-screen keyboard, straight into the shell (keys as pressed, Ctrl / Alt / Tab / arrows included; the terminal makes room for it) | ✕ / A tap | type |
| Enter | ✕ / A held | Enter |
| Erase a character | □ / X tap | Backspace |
| Mark part of the command, then erase it | □ / X held + ← → (↑ ↓: to the line's start / end), then □ / X tap | — |
| Esc | ○ / B tap | Esc |
| Leave for the sidebar (shells keep running) | ○ / B held | mouse |
| Menu: type a command, interrupt, copy, paste, launch htop/btop/… | △ / Y tap | Ctrl+C, Ctrl+Shift+C / V |
| Command history | △ / Y held | Ctrl+Shift+H |
| Quick keys for what's running (vim, nano, htop, mc, less, lazygit, tmux…) | Create / View tap | Ctrl+Shift+K |
| Saved commands | Create / View held | — |
| Scroll back / page | L2 / R2 | Shift+PgUp / PgDn |
| Choose text to copy | △ → "Choose text to copy": move, □ start, ✕ copy | drag, Ctrl+Shift+Space |
| Terminal tabs | tab strip, △ → Terminal tabs | Ctrl+Shift+T / W, Ctrl+PgUp / PgDn |

History and saved commands are put in, not run: ✕ held runs them. History merges the shell's own (fish, zsh, bash) with what was run from here. In the Flatpak the
shell runs on the host (`flatpak-spawn` and `script`), with resizes, Ctrl+C and the current folder.

## Steam

Sidebar → Steam: **Games** lists the installed games of every library (folders without a manifest
are named from Steam's app cache), **Prefixes** lists the Proton prefixes (`compatdata/<id>`) under
their games' names — non-Steam shortcuts and uninstalled games included. Going up from a game's
folder or prefix comes back to the list.

△ → "Add to Steam" on a Windows program (`.exe`, `.bat`, `.msi`) or a `.desktop` launcher puts it in
the library as a non-Steam game, with its icon (the exe's own, or the launcher's theme icon) and,
for Windows programs, Proton Experimental (unless Steam Play is on for every title). Steam keeps the
list in memory while it runs: Polka offers to restart it, or the game shows after its next start
(in the Deck's game mode only the latter). Without a Steam install none of this shows. Games and prefixes wear the icon of the game's own `.exe`
(sharper than Steam's), else Steam's.

## Disk usage

**Tools → Disk usage** in the sidebar (or △ on a folder) measures home, the current folder or a
whole disk in the background (space on disk; hard links counted once; other file systems mounted
inside are skipped) and shows it as **rings** like Filelight, **tiles** (a treemap) or a **list**.
The result stays in its tab while you browse and can be measured again in place.

| Disk usage | Gamepad | Mouse |
|---|---|---|
| Around the ring / neighbours | ← → | hover for details |
| Out / in a ring | ↓ ↑ | |
| Into a folder / show a file in the panel | ✕ / A, R2 | click |
| Up a level | ○ / B, L2 | click the middle, the path |
| Rings → tiles → list | □ / X | the buttons in the header |
| Open in the panel, new tab, measure again | △ / Y | right click |

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

Gamepad input is ignored while Polka is not the focused window. Touch screens, touchpads and pens
aren't taken for pads (on the Steam Deck the screen's touches stay touches).

**Mouse.** Click selects (Ctrl toggles, Shift selects a range), double click opens, middle
click opens a folder in a new tab, the side buttons go back / forward. Drag from empty space
(right of the names in the list, between icons in the grid) for a selection rectangle; Ctrl
adds to the selection. Drag items by their icon or name onto a folder, the other pane or a
sidebar place, then pick Move here / Copy here (the usual one is preselected: a move on the
same disk, else a copy; Ctrl or Shift held skips the question). Dropped on a program, script or
launcher they can also be opened with it. Onto the trash they're trashed. Files dragged in from other programs land
the same way, and items pulled out of the window go to other programs (Wayland; X11 takes drops
into the panel). A folded multi-volume
set is selected whole; an unfolded one volume by volume. Running operations show a progress card
(item, amount, speed, time left, cancel; also App menu → Cancel operations).

**Sidebar.** ✕ (or a click) opens a place and the sidebar keeps the focus; → or ○ goes over to
it. Places, pinned folders, **drives** (every partition that isn't the system's, mounted
or not: ✕ mounts and opens one, △ has Mount / Unmount / Eject through udisks, which asks polkit for
a password when needed) and tools. △ on any entry chooses its icon (a theme icon or your own
picture); on the trash it also empties it (after asking).

**Languages.** English, Русский, Українська, Deutsch, Español, Français, 中文, 日本語, 한국어 —
the system's by default (`LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG`), English when it isn't one
of these; Settings → Language switches on the fly. CJK text needs a CJK font (e.g. noto-fonts-cjk).

**Look.** Settings → Appearance: as the desktop (xdg-desktop-portal's colour scheme, followed
live), dark or light. Icon themes with light/dark siblings (Papirus, Breeze) switch along.

## Build

Needs Rust, libarchive and zstd (`pacman -S libarchive zstd`).

```sh
cargo build --release
./target/release/polka [DIR | FILE | ARCHIVE | file://URI]
```

Arch package: `packaging/arch/make-tarball.sh`, then `makepkg` in `packaging/arch`
(installs the binary, desktop entry and icon).
Video playback and thumbnails need `ffmpeg` (or `ffmpegthumbnailer` for thumbnails); so does
music in formats Symphonia can't decode.

Settings live in `~/.config/polka/config.toml` (pinned folders, view, sort,
`icon_theme` override, `zoom` for TV use; Ctrl+= / Ctrl+- also zoom).
`POLKA_NO_GAMEPAD=1` disables controller input.

## Flatpak

`packaging/flatpak/` builds the same program as a Flatpak (`io.github.sendement.Polka`, on
org.freedesktop.Platform 25.08 with 7-Zip built in). Inside the sandbox Polka uses your real
folders (trash, shared thumbnails, settings), the host's icon themes, starts the programs you open
on the host, and talks to UDisks2 for drives.

```sh
flatpak install flathub org.freedesktop.Sdk//25.08 org.freedesktop.Sdk.Extension.rust-stable//25.08
flatpak-builder --user --install --force-clean build-dir packaging/flatpak/io.github.sendement.Polka.yml
flatpak run io.github.sendement.Polka
```

`polka --diagnose` (or `flatpak run io.github.sendement.Polka --diagnose`) prints what Polka sees
around it — folders, icon themes, trash, disks, gamepads, whether it can start host programs — handy
for bug reports.

After changing dependencies, regenerate `packaging/flatpak/cargo-sources.json` with
[flatpak-cargo-generator](https://github.com/flatpak/flatpak-builder-tools/tree/master/cargo).

## Tests

```sh
cargo test
# offscreen UI renders (needs a GPU):
POLKA_TEST_DIR=/some/dir POLKA_SHOT_DIR=/tmp/shots cargo test -- --ignored screens
```

`tools/fakepad.py` creates a virtual Xbox pad via uinput for manual input testing
(`examples/padtest.rs` prints what Polka receives).

## License

MIT. Audio decoding uses [Symphonia](https://github.com/pdeljanov/Symphonia) (MPL-2.0).
`vendor/egui-winit` is [egui-winit](https://github.com/emilk/egui) (MIT OR Apache-2.0) with one
change: on Wayland the clipboard goes through Polka's own data device, which also does drag and
drop with other programs (see `vendor/egui-winit/POLKA-NOTICE.md`).
