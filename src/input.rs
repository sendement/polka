//! Unified input: gamepad (gilrs, background thread) and keyboard both produce
//! semantic [`Action`]s. The app decides what an action means in context.

use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use gilrs::{Axis, Button, EventType, GamepadId, Gilrs};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Up,
    Down,
    Left,
    Right,
    /// Cross / A
    Accept,
    /// Circle / B
    Back,
    /// Square / X
    Select,
    /// Triangle / Y
    Actions,
    /// Options / Menu (≡)
    Menu,
    /// Create / View (⧉)
    View,
    /// L1
    PrevTab,
    /// R1
    NextTab,
    /// L2
    LeftTrigger,
    /// R2
    RightTrigger,
    /// L3
    LeftStick,
    /// Keyboard / toolbar history
    HistoryBack,
    HistoryForward,
    Home,
    End,
}

impl Action {
    fn repeats(self) -> bool {
        matches!(
            self,
            Action::Up
                | Action::Down
                | Action::Left
                | Action::Right
                | Action::LeftTrigger
                | Action::RightTrigger
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PadKind {
    #[default]
    Xbox,
    PlayStation,
}

#[derive(Clone, Copy, Debug)]
pub enum Input {
    Press(Action),
    Release(Action),
    /// The gamepad in control changed; used for button glyphs.
    Pad(PadKind),
}

const REPEAT_DELAY: Duration = Duration::from_millis(330);
const REPEAT_SLOW: Duration = Duration::from_millis(85);
const REPEAT_FAST: Duration = Duration::from_millis(35);
/// Holding longer than this switches from slow to fast repeat.
const ACCEL_AFTER: Duration = Duration::from_millis(1500);
const STICK_ON: f32 = 0.55;
const STICK_OFF: f32 = 0.35;
/// How long another gamepad must be idle before a different one may take
/// over. Avoids double input when Steam exposes a virtual pad on top of the
/// real one.
const PAD_LOCK: Duration = Duration::from_millis(800);

struct Held {
    action: Action,
    since: Instant,
    next: Instant,
}

/// Direction of a stick on one axis pair, with hysteresis.
#[derive(Default)]
struct Stick {
    x: f32,
    y: f32,
    active: Option<Action>,
}

struct PadThread {
    tx: Sender<Input>,
    wake: Box<dyn Fn() + Send>,
    held: Vec<Held>,
    left: Stick,
    owner: Option<(GamepadId, Instant)>,
    /// The device whose stick moved last, and how many stick moves were logged.
    stick_from: String,
    stick_logged: u32,
}

/// Start reading gamepads into `tx`.
pub fn spawn_gamepad(tx: Sender<Input>, wake: impl Fn() + Send + 'static) {
    if std::env::var_os("POLKA_NO_GAMEPAD").is_some() {
        return;
    }
    std::thread::Builder::new()
        .name("polka-gamepad".into())
        .spawn(move || {
            let mut gilrs = match Gilrs::new() {
                Ok(g) => g,
                Err(e) => {
                    eprintln!("polka: gamepad support unavailable: {e}");
                    return;
                }
            };
            let mut t = PadThread {
                tx,
                wake: Box::new(wake),
                held: Vec::new(),
                left: Stick::default(),
                owner: None,
                stick_from: String::new(),
                stick_logged: 0,
            };
            let mut seen = std::collections::HashSet::new();
            loop {
                let timeout = t.next_deadline().map(|d| d.saturating_duration_since(Instant::now()));
                let ev = gilrs.next_event_blocking(Some(timeout.unwrap_or(Duration::from_millis(500))));
                // Touch screens and pens look like pads to gilrs.
                if let Some(e) = &ev {
                    if seen.insert(e.id) {
                        let p = gilrs.gamepad(e.id);
                        log(&format!(
                            "input device {:?} ({}) {:04x}:{:04x} mapping {:?}{}",
                            p.name(),
                            p.os_name(),
                            p.vendor_id().unwrap_or(0),
                            p.product_id().unwrap_or(0),
                            p.mapping_source(),
                            if not_a_pad(&p) { ": ignored, not a pad" } else { "" }
                        ));
                    }
                }
                let ev = ev.filter(|e| !not_a_pad(&gilrs.gamepad(e.id)));
                if let Some(e) = &ev {
                    if let EventType::AxisChanged(Axis::LeftStickX | Axis::LeftStickY, v, _) = e.event {
                        if v.abs() >= STICK_ON {
                            t.stick_from = gilrs.gamepad(e.id).name().to_string();
                        }
                    }
                }
                // Plugged in (or there at the start): its hints show at once.
                if let Some(e) = ev.as_ref().filter(|e| e.event == EventType::Connected) {
                    t.send(Input::Pad(pad_kind(&gilrs.gamepad(e.id))));
                }
                if let Some(ev) = ev {
                    let before = t.owner.map(|(o, _)| o);
                    t.handle(ev.id, ev.event);
                    let after = t.owner.map(|(o, _)| o);
                    if after != before {
                        if let Some(id) = after {
                            t.send(Input::Pad(pad_kind(&gilrs.gamepad(id))));
                        }
                    }
                }
                t.tick();
            }
        })
        .expect("spawn gamepad thread");
}

/// Its buttons' glyphs: PlayStation's shapes, else Xbox's letters.
fn pad_kind(pad: &gilrs::Gamepad) -> PadKind {
    let name = pad.name().to_lowercase();
    let sony = pad.vendor_id() == Some(0x054c) || ["dualsense", "dualshock", "wireless controller", "playstation", "ps5", "ps4"].iter().any(|k| name.contains(k));
    if sony { PadKind::PlayStation } else { PadKind::Xbox }
}

/// Devices gilrs takes for gamepads that aren't: a touch screen (the Steam
/// Deck's, FocalTech), touchpads (DualSense's), pens. Their position reads
/// as a stick pushed to one side and never let go.
pub fn not_a_pad(pad: &gilrs::Gamepad) -> bool {
    let name = format!("{} {}", pad.name(), pad.os_name()).to_lowercase();
    ["touch", "fts3528", "stylus", "wacom", " pen"].iter().any(|k| name.contains(k))
        || pad.vendor_id() == Some(0x2808)
        // Every pad has its ✕ / A; a screen or a pen doesn't.
        || pad.button_code(Button::South).is_none()
}

/// Where the app's log goes (unset: nowhere, as in the examples).
pub static LOG: std::sync::OnceLock<fn(&str)> = std::sync::OnceLock::new();

fn log(s: &str) {
    if let Some(f) = LOG.get() {
        f(s);
    }
}

fn map_button(b: Button) -> Option<Action> {
    Some(match b {
        Button::DPadUp => Action::Up,
        Button::DPadDown => Action::Down,
        Button::DPadLeft => Action::Left,
        Button::DPadRight => Action::Right,
        Button::South => Action::Accept,
        Button::East => Action::Back,
        Button::West => Action::Select,
        Button::North => Action::Actions,
        Button::Start => Action::Menu,
        Button::Select => Action::View,
        Button::LeftTrigger => Action::PrevTab,
        Button::RightTrigger => Action::NextTab,
        Button::LeftTrigger2 => Action::LeftTrigger,
        Button::RightTrigger2 => Action::RightTrigger,
        Button::LeftThumb => Action::LeftStick,
        _ => return None,
    })
}

impl PadThread {
    fn send(&self, i: Input) {
        let _ = self.tx.send(i);
        (self.wake)();
    }

    fn press(&mut self, a: Action) {
        if self.held.iter().any(|h| h.action == a) {
            return;
        }
        let now = Instant::now();
        if a.repeats() {
            self.held.push(Held { action: a, since: now, next: now + REPEAT_DELAY });
        } else {
            self.held.push(Held { action: a, since: now, next: now + Duration::from_secs(3600) });
        }
        self.send(Input::Press(a));
    }

    fn release(&mut self, a: Action) {
        let before = self.held.len();
        self.held.retain(|h| h.action != a);
        if self.held.len() != before {
            self.send(Input::Release(a));
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.held.iter().filter(|h| h.action.repeats()).map(|h| h.next).min()
    }

    fn tick(&mut self) {
        let now = Instant::now();
        let mut fire = Vec::new();
        for h in &mut self.held {
            if h.action.repeats() && now >= h.next {
                let fast = now.duration_since(h.since) > ACCEL_AFTER;
                h.next = now + if fast { REPEAT_FAST } else { REPEAT_SLOW };
                fire.push(h.action);
            }
        }
        for a in fire {
            self.send(Input::Press(a));
        }
    }

    fn owns(&mut self, id: GamepadId) -> bool {
        let now = Instant::now();
        match self.owner {
            Some((owner, last)) if owner != id && now.duration_since(last) < PAD_LOCK => false,
            _ => {
                if self.owner.map(|(o, _)| o) != Some(id) {
                    // Switching pads: drop whatever the old one was holding.
                    let held: Vec<_> = self.held.iter().map(|h| h.action).collect();
                    for a in held {
                        self.release(a);
                    }
                    self.left = Stick::default();
                }
                self.owner = Some((id, now));
                true
            }
        }
    }

    fn handle(&mut self, id: GamepadId, ev: EventType) {
        match ev {
            EventType::Disconnected => {
                if self.owner.map(|(o, _)| o) == Some(id) {
                    let held: Vec<_> = self.held.iter().map(|h| h.action).collect();
                    for a in held {
                        self.release(a);
                    }
                    self.owner = None;
                }
                return;
            }
            EventType::ButtonPressed(..)
            | EventType::ButtonReleased(..)
            | EventType::AxisChanged(..) => {}
            _ => return,
        }
        // Small stick noise must not steal ownership from the active pad.
        if let EventType::AxisChanged(_, v, _) = ev {
            if v.abs() < STICK_OFF && self.owner.map(|(o, _)| o) != Some(id) {
                return;
            }
        }
        if !self.owns(id) {
            return;
        }
        match ev {
            EventType::ButtonPressed(b, _) => {
                if let Some(a) = map_button(b) {
                    self.press(a);
                }
            }
            EventType::ButtonReleased(b, _) => {
                if let Some(a) = map_button(b) {
                    self.release(a);
                }
            }
            EventType::AxisChanged(axis, v, _) => match axis {
                // The right stick is deliberately unbound.
                Axis::LeftStickX => {
                    self.left.x = v;
                    self.update_stick();
                }
                Axis::LeftStickY => {
                    self.left.y = v;
                    self.update_stick();
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn update_stick(&mut self) {
        let s = &self.left;
        let (x, y) = (s.x, s.y);
        let mag = x.abs().max(y.abs());
        let threshold = if s.active.is_some() { STICK_OFF } else { STICK_ON };
        let want = if mag < threshold {
            None
        } else if x.abs() > y.abs() {
            Some(if x > 0.0 { Action::Right } else { Action::Left })
        } else {
            // gilrs: up is positive Y.
            Some(if y > 0.0 { Action::Up } else { Action::Down })
        };
        let cur = s.active;
        if cur != want {
            if let Some(c) = cur {
                self.release(c);
            }
            if let Some(w) = want {
                // A few, to tell which device moves things by itself.
                if self.stick_logged < 30 {
                    self.stick_logged += 1;
                    log(&format!("stick {w:?} from {:?} ({x:.2}, {y:.2})", self.stick_from));
                }
                self.press(w);
            }
            let s = &mut self.left;
            s.active = want;
        }
    }
}
