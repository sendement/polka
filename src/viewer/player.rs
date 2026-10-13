//! The app side of the music player: gamepad, keyboard, the △ menu and
//! moving on to the next track in every tab (also ones not shown).

use egui::{Event, Key};

use super::Content;
use super::audio::{AudioState, Repeat};
use crate::app::{App, Cmd, Menu, Overlay, item};
use crate::i18n::tr;
use crate::input::Action;

const SEEK: f32 = 5.0;
const SEEK_LONG: f32 = 30.0;
const VOLUME_STEP: f32 = 0.05;

impl App {
    fn audio_mut(&mut self) -> Option<&mut AudioState> {
        match self.tabs.get_mut(self.tab)?.viewer.as_mut().map(|v| &mut v.content) {
            Some(Content::Audio(a)) => Some(a),
            _ => None,
        }
    }

    pub fn audio_shown(&self) -> bool {
        matches!(self.tabs.get(self.tab).and_then(|t| t.viewer.as_ref()).map(|v| &v.content), Some(Content::Audio(_)))
    }

    /// Gamepad in the player. False: not the player (or not its button).
    pub fn viewer_audio_input(&mut self, a: Action) -> bool {
        if !self.audio_shown() {
            return false;
        }
        let ctx = self.ctx.clone();
        let vol = self.cfg.viewer.volume;
        let hold = self.edit_hold.is_some();
        if hold && matches!(a, Action::Up | Action::Down | Action::Left | Action::Right) {
            self.edit_hold = Some(true);
        }
        let Some(st) = self.audio_mut() else { return false };
        match a {
            // □ held: ↑↓ volume, ←→ long seek.
            Action::Up | Action::Down if hold => {
                self.audio_cmd(Cmd::AudioVolume(if a == Action::Up { 1 } else { -1 }));
            }
            Action::Left | Action::Right if hold => st.seek_by(if a == Action::Right { SEEK_LONG } else { -SEEK_LONG }),
            Action::Up => {
                st.cursor = st.cursor.saturating_sub(1);
                st.reveal = true;
            }
            Action::Down => {
                st.cursor = (st.cursor + 1).min(st.tracks.len() - 1);
                st.reveal = true;
            }
            Action::Left | Action::Right => st.seek_by(if a == Action::Right { SEEK } else { -SEEK }),
            Action::Accept => {
                if st.cursor == st.index {
                    st.toggle();
                } else {
                    let i = st.cursor;
                    st.play_index(i, vol, &ctx);
                    self.audio_moved();
                }
            }
            Action::LeftTrigger | Action::RightTrigger => {
                self.audio_cmd(if a == Action::RightTrigger { Cmd::AudioNext } else { Cmd::AudioPrev });
            }
            Action::Home => st.seek_frac(0.0),
            Action::Select => self.edit_hold = Some(false),
            _ => return false,
        }
        true
    }

    /// □ released in the player: a tap (no D-pad meanwhile) cycles repeat.
    pub fn audio_select_released(&mut self) {
        if self.edit_hold.take() == Some(false) {
            self.audio_cmd(Cmd::AudioRepeat);
        }
    }

    /// Keys with their own meaning in the player; the rest go on as usual.
    pub fn audio_keyboard(&mut self, events: Vec<Event>) -> Vec<Event> {
        let mut rest = Vec::new();
        for ev in events {
            let used = match &ev {
                Event::Key { key, pressed: true, modifiers: m, .. } if !m.command && !m.alt => match key {
                    Key::Space | Key::K => self.audio_cmd(Cmd::AudioToggle),
                    Key::Plus | Key::Equals => self.audio_cmd(Cmd::AudioVolume(1)),
                    Key::Minus => self.audio_cmd(Cmd::AudioVolume(-1)),
                    Key::R => self.audio_cmd(Cmd::AudioRepeat),
                    Key::N => self.audio_cmd(Cmd::AudioNext),
                    Key::P => self.audio_cmd(Cmd::AudioPrev),
                    Key::ArrowLeft | Key::ArrowRight if m.shift => {
                        if let Some(st) = self.audio_mut() {
                            st.seek_by(if *key == Key::ArrowRight { SEEK_LONG } else { -SEEK_LONG });
                        }
                        true
                    }
                    _ => false,
                },
                _ => false,
            };
            if !used {
                rest.push(ev);
            }
        }
        rest
    }

    /// △ in the player.
    pub fn viewer_audio_menu(&mut self) {
        let repeat = Repeat::from(self.cfg.viewer.repeat);
        let Some(st) = self.audio_mut() else { return };
        let playing = st.playing();
        let name = st.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut items = vec![item(if playing { tr("Pause") } else { tr("Play") }, Cmd::AudioToggle)];
        if st.tracks.len() > 1 {
            items.push(item(tr("Next track"), Cmd::AudioNext));
            items.push(item(tr("Previous track"), Cmd::AudioPrev));
        }
        let r = match repeat {
            Repeat::Off => tr("off"),
            Repeat::All => tr("all tracks"),
            Repeat::One => tr("this track"),
        };
        items.push(item(&format!("{}: {r}", tr("Repeat")), Cmd::AudioRepeat));
        items.push(item(tr("Louder"), Cmd::AudioVolume(1)));
        items.push(item(tr("Quieter"), Cmd::AudioVolume(-1)));
        items.push(item(tr("Open in another app"), Cmd::OpenExternal));
        items.push(item(tr("Close"), Cmd::CloseViewer));
        self.overlay = Some(Overlay::Menu(Menu::new(&name, items)));
    }

    /// Player commands; always true (for use as "key handled").
    pub fn audio_cmd(&mut self, cmd: Cmd) -> bool {
        let ctx = self.ctx.clone();
        let vol = self.cfg.viewer.volume;
        match cmd {
            Cmd::AudioToggle => {
                if let Some(st) = self.audio_mut() {
                    st.toggle();
                }
            }
            Cmd::AudioNext | Cmd::AudioPrev => {
                if let Some(st) = self.audio_mut() {
                    st.step(if matches!(cmd, Cmd::AudioNext) { 1 } else { -1 }, vol, &ctx);
                }
                self.audio_moved();
            }
            Cmd::AudioPlay(i) => {
                if let Some(st) = self.audio_mut() {
                    if i == st.index {
                        st.toggle();
                    } else {
                        st.play_index(i, vol, &ctx);
                    }
                }
                self.audio_moved();
            }
            Cmd::AudioRepeat => {
                self.cfg.viewer.repeat = (self.cfg.viewer.repeat + 1) % 3;
                self.cfg.save();
                let msg = match Repeat::from(self.cfg.viewer.repeat) {
                    Repeat::Off => tr("Repeat off"),
                    Repeat::All => tr("Repeat all tracks"),
                    Repeat::One => tr("Repeat this track"),
                };
                self.toast(msg, false);
            }
            Cmd::AudioVolume(d) => {
                let v = (self.cfg.viewer.volume + d as f32 * VOLUME_STEP).clamp(0.0, 1.0);
                self.audio_set_volume(v);
            }
            Cmd::AudioSeek(f) => {
                if let Some(st) = self.audio_mut() {
                    st.seek_frac(f);
                }
            }
            _ => {}
        }
        true
    }

    pub fn audio_set_volume(&mut self, v: f32) {
        let v = (v * 100.0).round() / 100.0;
        self.cfg.viewer.volume = v;
        // Every tab's player follows.
        for t in &mut self.tabs {
            if let Some(Content::Audio(a)) = t.viewer.as_mut().map(|v| &mut v.content) {
                a.set_volume(v);
            }
        }
        self.cfg.save();
    }

    /// The viewer's file (and the pane's cursor) follow the track.
    fn audio_moved(&mut self) {
        let tab = self.tab;
        self.audio_follow(tab);
    }

    fn audio_follow(&mut self, tab: usize) {
        let t = &mut self.tabs[tab];
        let Some(v) = t.viewer.as_mut() else { return };
        let Content::Audio(a) = &v.content else { return };
        let path = a.path().to_path_buf();
        v.name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        v.path = path.clone();
        let p = &mut t.panes[v.origin];
        if let Some(i) = p.entries.iter().position(|e| p.loc.entry_path(&e.name).as_deref() == Some(path.as_path())) {
            p.set_cursor(i);
        }
    }

    /// Per frame, every tab: tags arriving, tracks ending.
    pub fn tick_audio(&mut self) {
        let ctx = self.ctx.clone();
        let (repeat, vol) = (Repeat::from(self.cfg.viewer.repeat), self.cfg.viewer.volume);
        let mut playing = false;
        for tab in 0..self.tabs.len() {
            let Some(Content::Audio(a)) = self.tabs[tab].viewer.as_mut().map(|v| &mut v.content) else { continue };
            let moved = a.tick(repeat, vol, &ctx);
            playing |= a.playing();
            if moved {
                self.audio_follow(tab);
            }
        }
        if playing {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}
