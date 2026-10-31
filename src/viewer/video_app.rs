//! The app side of the video player: gamepad, keyboard, the △ menu,
//! full screen.

use egui::{Event, Key};

use super::Content;
use super::video::VideoState;
use crate::app::{App, Cmd, Menu, Overlay, item};
use crate::i18n::tr;
use crate::input::Action;

const SEEK: f64 = 10.0;
const SEEK_LONG: f64 = 60.0;

impl App {
    fn video_mut(&mut self) -> Option<&mut VideoState> {
        match self.tabs.get_mut(self.tab)?.viewer.as_mut().map(|v| &mut v.content) {
            Some(Content::Video(m)) => Some(m),
            _ => None,
        }
    }

    pub fn video_shown(&self) -> bool {
        matches!(self.tabs.get(self.tab).and_then(|t| t.viewer.as_ref()).map(|v| &v.content), Some(Content::Video(_)))
    }

    /// The current tab shows a video full screen (the app's chrome hides).
    /// A video full screen, or a slide show: the screen is theirs.
    pub fn video_full(&self) -> bool {
        matches!(self.tabs.get(self.tab).and_then(|t| t.viewer.as_ref()).map(|v| &v.content), Some(Content::Video(m)) if m.full)
            || matches!(self.tabs.get(self.tab).and_then(|t| t.viewer.as_ref()).map(|v| &v.content), Some(Content::Doc(d)) if d.show)
    }

    /// Gamepad in the video player. False: not the player (or not its button).
    pub fn viewer_video_input(&mut self, a: Action) -> bool {
        if !self.video_shown() {
            return false;
        }
        let ctx = self.ctx.clone();
        let hold = self.edit_hold.is_some();
        if hold && matches!(a, Action::Left | Action::Right) {
            self.edit_hold = Some(true);
        }
        let Some(m) = self.video_mut() else { return false };
        m.show_controls();
        match a {
            Action::Accept => m.toggle(&ctx),
            Action::Left | Action::Right => {
                let d = if hold { SEEK_LONG } else { SEEK };
                m.seek_by(if a == Action::Right { d } else { -d }, &ctx);
            }
            Action::Up | Action::Down => {
                self.audio_cmd(Cmd::AudioVolume(if a == Action::Up { 1 } else { -1 }));
            }
            Action::LeftTrigger | Action::RightTrigger => {
                self.video_cmd(if a == Action::RightTrigger { Cmd::AudioNext } else { Cmd::AudioPrev });
            }
            Action::Home => m.seek(0.0, &ctx),
            Action::View => {
                self.video_cmd(Cmd::VideoFullscreen);
            }
            Action::Select => self.edit_hold = Some(false),
            // ○ leaves full screen first.
            Action::Back if m.full => m.full = false,
            _ => return false,
        }
        true
    }

    /// □ released: a tap (no seeking meanwhile) switches subtitles.
    pub fn video_select_released(&mut self) {
        if self.edit_hold.take() == Some(false) {
            self.video_cmd(Cmd::VideoSubs);
        }
    }

    /// Keys with their own meaning in the video player.
    pub fn video_keyboard(&mut self, events: Vec<Event>) -> Vec<Event> {
        let mut rest = Vec::new();
        for ev in events {
            let used = match &ev {
                Event::Key { key, pressed: true, modifiers: m, .. } if !m.command && !m.alt => {
                    if let Some(v) = self.video_mut() {
                        v.show_controls();
                    }
                    match key {
                        Key::Space | Key::K => self.video_cmd(Cmd::AudioToggle),
                        Key::F | Key::F11 => self.video_cmd(Cmd::VideoFullscreen),
                        Key::S => self.video_cmd(Cmd::VideoSubs),
                        Key::A => self.video_cmd(Cmd::VideoAudio),
                        Key::N => self.video_cmd(Cmd::AudioNext),
                        Key::P => self.video_cmd(Cmd::AudioPrev),
                        Key::Plus | Key::Equals => self.audio_cmd(Cmd::AudioVolume(1)),
                        Key::Minus => self.audio_cmd(Cmd::AudioVolume(-1)),
                        Key::ArrowLeft | Key::ArrowRight if m.shift => {
                            let ctx = self.ctx.clone();
                            if let Some(v) = self.video_mut() {
                                v.seek_by(if *key == Key::ArrowRight { SEEK_LONG } else { -SEEK_LONG }, &ctx);
                            }
                            true
                        }
                        _ => false,
                    }
                }
                _ => false,
            };
            if !used {
                rest.push(ev);
            }
        }
        rest
    }

    /// △ in the video player.
    pub fn viewer_video_menu(&mut self) {
        let Some(m) = self.video_mut() else { return };
        let name = m.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut items = vec![item(if m.playing() { tr("Pause") } else { tr("Play") }, Cmd::AudioToggle)];
        items.push(item(if m.full { tr("Leave full screen") } else { tr("Full screen") }, Cmd::VideoFullscreen));
        if m.probe.audio.len() > 1 {
            let label = &m.probe.audio[m.audio_track.min(m.probe.audio.len() - 1)].label;
            items.push(item(&format!("{}: {label}", tr("Audio track")), Cmd::VideoAudio));
        }
        if m.sub_choices().len() > 1 {
            items.push(item(&format!("{}: {}", tr("Subtitles"), m.subs.label()), Cmd::VideoSubs));
        }
        if m.tracks.len() > 1 {
            items.push(item(tr("Next video"), Cmd::AudioNext));
            items.push(item(tr("Previous video"), Cmd::AudioPrev));
        }
        items.push(item(tr("Louder"), Cmd::AudioVolume(1)));
        items.push(item(tr("Quieter"), Cmd::AudioVolume(-1)));
        items.push(item(tr("Open in another app"), Cmd::OpenExternal));
        items.push(item(tr("Close"), Cmd::CloseViewer));
        self.overlay = Some(Overlay::Menu(Menu::new(&name, items)));
    }

    /// Player commands for a video; always true ("key handled").
    pub fn video_cmd(&mut self, cmd: Cmd) -> bool {
        let ctx = self.ctx.clone();
        let Some(m) = self.video_mut() else { return true };
        m.show_controls();
        match cmd {
            Cmd::AudioToggle => m.toggle(&ctx),
            Cmd::AudioNext | Cmd::AudioPrev => {
                if m.step(if matches!(cmd, Cmd::AudioNext) { 1 } else { -1 }, &ctx) {
                    self.audio_moved();
                }
            }
            Cmd::AudioSeek(f) => {
                if let Some(d) = m.duration() {
                    m.seek(d * f.clamp(0.0, 1.0) as f64, &ctx);
                }
            }
            Cmd::VideoFullscreen => m.full = !m.full,
            Cmd::VideoAudio => {
                let msg = match m.cycle_audio(&ctx) {
                    Some(l) => format!("{}: {l}", tr("Audio track")),
                    None => tr("No other audio tracks").to_string(),
                };
                self.toast(msg, false);
            }
            Cmd::VideoSubs => {
                let msg = match m.cycle_subs(&ctx) {
                    Some(l) => format!("{}: {l}", tr("Subtitles")),
                    None => tr("No subtitles").to_string(),
                };
                self.toast(msg, false);
            }
            _ => {}
        }
        true
    }
}
