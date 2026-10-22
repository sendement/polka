//! A touch screen. egui-winit turns the first finger into a mouse (pressed
//! at once, so a drag would select or drag files); here that's undone and
//! the finger's moves read as gestures instead:
//!
//! - a tap: a click (the file list opens on it, or on two taps, as set);
//!   two taps quickly at one place: a double tap;
//! - a finger moved up or down: scrolling, gliding on after a flick;
//! - held still half a second: let go, it's a right click (the menu);
//!   moved on, it picks up what's under it (a mouse drag);
//! - a quick swipe sideways: back / forward (from the left edge: the
//!   sidebar);
//! - two fingers apart or together: bigger or smaller icons;
//! - put on a grip (the sidebar's edge): pressed at once, a mouse drag.

use std::time::{Duration, Instant};

use egui::{Event, Modifiers, MouseWheelUnit, PointerButton, Pos2, TouchPhase, Vec2};

/// Held this long without moving: a long press.
pub const HOLD: Duration = Duration::from_millis(500);
/// Moved this far (points): no tap, no long press.
const SLOP: f32 = 12.0;
/// A swipe: this far sideways, this quickly.
const SWIPE: f32 = 100.0;
const SWIPE_TIME: Duration = Duration::from_millis(600);
/// A second tap this soon, this near: a double tap.
const DOUBLE_TIME: Duration = Duration::from_millis(450);
const DOUBLE_NEAR: f32 = 40.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    /// Down, not yet moved nor held.
    Pending,
    Scroll,
    Swipe,
    /// Held still past HOLD.
    Held,
    /// Held, then moved: a mouse drag.
    Drag,
    /// A second finger came: neither tap nor scroll.
    Pinch,
}

struct Press {
    id: u64,
    start: Pos2,
    at: Instant,
    last: Pos2,
    last_t: Instant,
    /// Points per second, smoothed (for the glide).
    vel: Vec2,
    mode: Mode,
}

/// What the gestures ask of the app.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gesture {
    Back,
    Forward,
    /// From the left edge, towards the middle.
    Sidebar,
    /// Fingers apart (> 1) or together (< 1), by so much since last asked.
    Pinch(f32),
}

#[derive(Default)]
pub struct Touch {
    press: Option<Press>,
    fingers: Vec<u64>,
    /// Scrolling on after the finger left: velocity, last step.
    glide: Option<(Vec2, Instant, Pos2)>,
    /// A click's release, sent a frame after its press (and whether it
    /// was a double tap's second).
    release: Option<(Pos2, PointerButton, bool)>,
    /// The click this frame came from a tap (the file list opens on it).
    pub tapped: bool,
    /// …the second of a double tap.
    pub double: bool,
    /// The last tap (for a double tap).
    last_tap: Option<(Instant, Pos2)>,
    /// Grips: a finger put on one drags it right away (set every frame).
    pub grips: Vec<egui::Rect>,
    /// Two fingers: their distance when last measured.
    pinch: Option<f32>,
    finger_pos: Vec<(u64, Pos2)>,
}

fn wheel(delta: Vec2) -> Event {
    Event::MouseWheel { unit: MouseWheelUnit::Point, delta, phase: TouchPhase::Move, modifiers: Modifiers::NONE }
}

fn button(pos: Pos2, button: PointerButton, pressed: bool) -> Event {
    Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE }
}

impl Touch {
    /// Something's under way that needs frames (a hold to time, a glide,
    /// a release to send).
    pub fn busy(&self) -> bool {
        self.glide.is_some() || self.release.is_some() || self.press.as_ref().is_some_and(|p| matches!(p.mode, Mode::Pending | Mode::Held))
    }

    /// Where a finger rests, and how far towards a long press (0..1).
    pub fn holding(&self) -> Option<(Pos2, f32)> {
        let p = self.press.as_ref()?;
        match p.mode {
            Mode::Pending => Some((p.last, (p.at.elapsed().as_secs_f32() / HOLD.as_secs_f32()).min(1.0))),
            Mode::Held => Some((p.last, 1.0)),
            _ => None,
        }
    }

    /// The frame's events, the touch screen's rewritten; and the gestures.
    pub fn filter(&mut self, events: Vec<Event>, now: Instant) -> (Vec<Event>, Vec<Gesture>) {
        let mut out = Vec::with_capacity(events.len());
        let mut gestures = Vec::new();
        self.tapped = false;
        self.double = false;
        // A tap's (or long press's) release, a frame after its press.
        // (The pointer stays over the place till then: a click needs it.)
        if let Some((pos, b, double)) = self.release.take() {
            out.push(Event::PointerMoved(pos));
            out.push(button(pos, b, false));
            out.push(Event::PointerGone);
            self.tapped = b == PointerButton::Primary;
            self.double = double;
        }
        // Held long enough?
        if let Some(p) = self.press.as_mut().filter(|p| p.mode == Mode::Pending && now.duration_since(p.at) >= HOLD) {
            p.mode = Mode::Held;
        }
        // Gliding on.
        if let Some((v, t, pos)) = self.glide {
            let dt = now.duration_since(t).as_secs_f32().min(0.05);
            let v2 = v * (-dt * 3.5).exp();
            out.push(Event::PointerMoved(pos));
            out.push(wheel(v * dt));
            self.glide = if v2.length() > 40.0 { Some((v2, now, pos)) } else { None };
            if self.glide.is_none() {
                out.push(Event::PointerGone);
            }
        }
        let touching = !self.fingers.is_empty() || events.iter().any(|e| matches!(e, Event::Touch { .. }));
        for ev in events {
            match ev {
                Event::Touch { id, phase, pos, .. } => {
                    let id = id.0;
                    self.touch(id, phase, pos, now, &mut out, &mut gestures);
                    out.push(ev);
                }
                // The mouse egui-winit made of the finger: ours instead.
                Event::PointerButton { button: PointerButton::Primary, .. } if touching => {}
                // Gone with the finger, but a glide or a click still needs
                // the place.
                Event::PointerGone if self.glide.is_some() || self.release.is_some() => {}
                e => out.push(e),
            }
        }
        // Two fingers: how much further apart.
        if self.fingers.len() >= 2 {
            let pts: Vec<Pos2> = self.fingers.iter().filter_map(|f| self.finger_pos.iter().find(|(i, _)| i == f).map(|x| x.1)).collect();
            if let [a, b, ..] = pts[..] {
                let d = a.distance(b).max(1.0);
                if let Some(before) = self.pinch {
                    if (d / before - 1.0).abs() > 0.01 {
                        gestures.push(Gesture::Pinch(d / before));
                        self.pinch = Some(d);
                    }
                } else {
                    self.pinch = Some(d);
                }
            }
        } else {
            self.pinch = None;
        }
        (out, gestures)
    }

    fn touch(&mut self, id: u64, phase: TouchPhase, pos: Pos2, now: Instant, out: &mut Vec<Event>, gestures: &mut Vec<Gesture>) {
        match self.finger_pos.iter_mut().find(|(i, _)| *i == id) {
            Some(f) => f.1 = pos,
            None => self.finger_pos.push((id, pos)),
        }
        match phase {
            TouchPhase::Start => {
                if !self.fingers.contains(&id) {
                    self.fingers.push(id);
                }
                self.glide = None;
                if self.fingers.len() == 1 {
                    let grip = self.grips.iter().any(|g| g.contains(pos));
                    self.press = Some(Press { id, start: pos, at: now, last: pos, last_t: now, vel: Vec2::ZERO, mode: if grip { Mode::Drag } else { Mode::Pending } });
                    if grip {
                        out.push(Event::PointerMoved(pos));
                        out.push(button(pos, PointerButton::Primary, true));
                    }
                } else if let Some(p) = self.press.as_mut() {
                    // A second finger: a pinch; a drag under way lets go.
                    if p.mode == Mode::Drag {
                        out.push(button(p.last, PointerButton::Primary, false));
                    }
                    p.mode = Mode::Pinch;
                }
            }
            TouchPhase::Move => {
                let Some(p) = self.press.as_mut().filter(|p| p.id == id) else { return };
                let moved = pos - p.start;
                let dt = now.duration_since(p.last_t).as_secs_f32();
                let step = pos - p.last;
                if dt > 0.0 {
                    p.vel = p.vel * 0.6 + (step / dt) * 0.4;
                }
                match p.mode {
                    Mode::Pending if moved.length() > SLOP => {
                        p.mode = if moved.x.abs() > moved.y.abs() * 1.5 { Mode::Swipe } else { Mode::Scroll };
                        if p.mode == Mode::Scroll {
                            out.push(wheel(moved));
                        }
                    }
                    Mode::Scroll => out.push(wheel(step)),
                    Mode::Held if moved.length() > SLOP => {
                        // Picked up: a mouse pressed where the finger was.
                        p.mode = Mode::Drag;
                        out.push(Event::PointerMoved(p.start));
                        out.push(button(p.start, PointerButton::Primary, true));
                        out.push(Event::PointerMoved(pos));
                    }
                    _ => {}
                }
                p.last = pos;
                p.last_t = now;
            }
            TouchPhase::End | TouchPhase::Cancel => {
                self.fingers.retain(|f| *f != id);
                self.finger_pos.retain(|(f, _)| *f != id);
                let Some(p) = self.press.take_if(|p| p.id == id) else { return };
                let cancel = phase == TouchPhase::Cancel;
                match p.mode {
                    Mode::Pending if !cancel => {
                        // A tap: a click (its release next frame).
                        out.push(Event::PointerMoved(pos));
                        out.push(button(pos, PointerButton::Primary, true));
                        let double = self.last_tap.take().is_some_and(|(t, at)| now.duration_since(t) < DOUBLE_TIME && at.distance(pos) < DOUBLE_NEAR);
                        if !double {
                            self.last_tap = Some((now, pos));
                        }
                        self.release = Some((pos, PointerButton::Primary, double));
                    }
                    Mode::Held if !cancel => {
                        // A long press: a right click.
                        out.push(Event::PointerMoved(pos));
                        out.push(button(pos, PointerButton::Secondary, true));
                        self.release = Some((pos, PointerButton::Secondary, false));
                    }
                    Mode::Scroll if p.vel.length() > 200.0 && now.duration_since(p.last_t) < Duration::from_millis(80) => {
                        self.glide = Some((p.vel, now, pos));
                    }
                    Mode::Swipe => {
                        let moved = pos - p.start;
                        if moved.x.abs() > SWIPE && now.duration_since(p.at) < SWIPE_TIME {
                            gestures.push(if moved.x > 0.0 && p.start.x < 24.0 {
                                Gesture::Sidebar
                            } else if moved.x > 0.0 {
                                Gesture::Back
                            } else {
                                Gesture::Forward
                            });
                        }
                    }
                    Mode::Drag => out.push(button(pos, PointerButton::Primary, false)),
                    _ => {}
                }
            }
        }
    }
}

/// A finger on the screen, as the touch screen gives it (for tests).
#[cfg(test)]
pub fn finger(id: u64, phase: TouchPhase, x: f32, y: f32) -> Event {
    Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(id), phase, pos: egui::pos2(x, y), force: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    fn is_click(e: &[Event], b: PointerButton, pressed: bool) -> bool {
        e.iter().any(|e| matches!(e, Event::PointerButton { button, pressed: p, .. } if *button == b && *p == pressed))
    }

    #[test]
    fn gestures() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        // A tap: the simulated press is dropped; ours comes on the lift,
        // its release a frame later.
        let mut t = Touch::default();
        let (out, _) = t.filter(vec![finger(1, TouchPhase::Start, 100.0, 100.0), button(pos2(100.0, 100.0), PointerButton::Primary, true)], ms(0));
        assert!(!is_click(&out, PointerButton::Primary, true));
        let (out, _) = t.filter(vec![finger(1, TouchPhase::End, 101.0, 100.0), button(pos2(101.0, 100.0), PointerButton::Primary, false)], ms(120));
        assert!(is_click(&out, PointerButton::Primary, true) && !is_click(&out, PointerButton::Primary, false));
        let (out, _) = t.filter(vec![], ms(140));
        assert!(is_click(&out, PointerButton::Primary, false) && t.tapped && !t.double);
        // Again, soon: a double tap.
        t.filter(vec![finger(2, TouchPhase::Start, 105.0, 98.0)], ms(250));
        t.filter(vec![finger(2, TouchPhase::End, 105.0, 98.0)], ms(320));
        t.filter(vec![], ms(340));
        assert!(t.tapped && t.double);
        // A grip: pressed as the finger lands.
        let mut t = Touch { grips: vec![egui::Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(10.0, 100.0))], ..Default::default() };
        let (out, _) = t.filter(vec![finger(1, TouchPhase::Start, 5.0, 50.0)], ms(0));
        assert!(is_click(&out, PointerButton::Primary, true));
        let (out, _) = t.filter(vec![finger(1, TouchPhase::End, 80.0, 50.0)], ms(100));
        assert!(is_click(&out, PointerButton::Primary, false));
        // A drag up: wheel events, no press.
        let mut t = Touch::default();
        t.filter(vec![finger(1, TouchPhase::Start, 100.0, 300.0)], ms(0));
        let (out, _) = t.filter(vec![finger(1, TouchPhase::Move, 100.0, 250.0)], ms(30));
        assert!(out.iter().any(|e| matches!(e, Event::MouseWheel { delta, .. } if delta.y < -40.0)));
        assert!(!is_click(&out, PointerButton::Primary, true));
        // Held: a right click on the lift.
        let mut t = Touch::default();
        t.filter(vec![finger(1, TouchPhase::Start, 50.0, 50.0)], ms(0));
        t.filter(vec![], ms(600));
        assert!(t.holding().is_some_and(|h| h.1 >= 1.0));
        let (out, _) = t.filter(vec![finger(1, TouchPhase::End, 50.0, 50.0)], ms(650));
        assert!(is_click(&out, PointerButton::Secondary, true));
        // Held, then moved: a mouse drag from where it was.
        let mut t = Touch::default();
        t.filter(vec![finger(1, TouchPhase::Start, 50.0, 50.0)], ms(0));
        t.filter(vec![], ms(600));
        let (out, _) = t.filter(vec![finger(1, TouchPhase::Move, 90.0, 60.0)], ms(650));
        assert!(is_click(&out, PointerButton::Primary, true));
        let (out, _) = t.filter(vec![finger(1, TouchPhase::End, 200.0, 60.0)], ms(700));
        assert!(is_click(&out, PointerButton::Primary, false));
        // A quick swipe right: back; from the edge: the sidebar.
        for (x, want) in [(300.0, Gesture::Back), (5.0, Gesture::Sidebar)] {
            let mut t = Touch::default();
            t.filter(vec![finger(1, TouchPhase::Start, x, 200.0)], ms(0));
            t.filter(vec![finger(1, TouchPhase::Move, x + 60.0, 205.0)], ms(60));
            let (_, g) = t.filter(vec![finger(1, TouchPhase::End, x + 180.0, 210.0)], ms(150));
            assert_eq!(g, [want]);
        }
        // Two fingers apart.
        let mut t = Touch::default();
        t.filter(vec![finger(1, TouchPhase::Start, 100.0, 100.0), finger(2, TouchPhase::Start, 200.0, 100.0)], ms(0));
        let (_, g) = t.filter(vec![finger(2, TouchPhase::Move, 300.0, 100.0)], ms(50));
        assert!(matches!(g[..], [Gesture::Pinch(z)] if z > 1.9), "{g:?}");
    }
}

impl crate::app::App {
    /// The frame's events through the touch gestures (what the window
    /// system gives, and tests); the gestures done.
    pub fn touch_feed(&mut self, events: Vec<Event>) -> Vec<Event> {
        use crate::input::{Action, Input};
        let (events, gestures) = self.touch.filter(events, Instant::now());
        for g in gestures {
            match g {
                Gesture::Back | Gesture::Forward if self.overlay.is_none() => {
                    self.focus = crate::app::Focus::Panes;
                    self.handle(Input::Press(if g == Gesture::Back { Action::HistoryBack } else { Action::HistoryForward }));
                }
                Gesture::Sidebar if self.overlay.is_none() && self.cfg.sidebar_collapsed => self.run(crate::app::Cmd::ToggleSidebar),
                // Icons bigger or smaller (a picture shown zooms itself).
                Gesture::Pinch(z) if self.overlay.is_none() && self.cur_tab().viewer.is_none() => {
                    self.pinch *= z;
                    let d = if self.pinch > 1.3 { 1.0 } else if self.pinch < 0.77 { -1.0 } else { 0.0 };
                    if d != 0.0 {
                        self.pinch = 1.0;
                        let grid = self.cfg.view == crate::config::ViewMode::Grid;
                        let ((lo, hi, _), v) = if grid { (crate::config::GRID_ICON, &mut self.cfg.grid_icon) } else { (crate::config::LIST_ICON, &mut self.cfg.list_icon) };
                        *v = (*v + d * if grid { 24.0 } else { 8.0 }).clamp(lo, hi);
                        self.cfg.save();
                    }
                }
                _ => {}
            }
        }
        events
    }
}
