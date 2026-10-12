//! DualSense / DualShock 4 touchpad. The kernel exposes it as a separate
//! evdev device that gilrs ignores, so it's read directly:
//! swipe up/down moves the cursor, swipe left/right acts like the arrows,
//! clicking the pad toggles between the file list and the sidebar.
//!
//! The device is not readable by default: it needs the uaccess udev rule
//! shipped in packaging/ (TOUCHPAD_STATUS tells the UI).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use evdev::{AbsoluteAxisCode, EventSummary, KeyCode};

use crate::input::{Action, Input};

pub const STATUS_NONE: u8 = 0;
pub const STATUS_OK: u8 = 1;
pub const STATUS_DENIED: u8 = 2;
pub static TOUCHPAD_STATUS: AtomicU8 = AtomicU8::new(STATUS_NONE);

/// Touchpad units per cursor step (the pad is 1920×1080).
const STEP_V: i32 = 90;
const STEP_H: i32 = 260;
/// Movement before a swipe commits to an axis.
const AXIS_LOCK: i32 = 50;
const OWNER_IDLE: Duration = Duration::from_millis(800);

type Owner = Arc<Mutex<Option<(PathBuf, Instant)>>>;

pub fn spawn(tx: Sender<Input>, wake: Arc<dyn Fn() + Send + Sync>) {
    std::thread::Builder::new()
        .name("polka-touchpad".into())
        .spawn(move || {
            let open: Arc<Mutex<HashSet<PathBuf>>> = Default::default();
            let owner: Owner = Default::default();
            loop {
                let mut denied = false;
                for node in touchpad_nodes() {
                    if open.lock().unwrap().contains(&node) {
                        continue;
                    }
                    match evdev::Device::open(&node) {
                        Ok(dev) => {
                            open.lock().unwrap().insert(node.clone());
                            let (tx, wake, open, owner) = (tx.clone(), wake.clone(), open.clone(), owner.clone());
                            std::thread::spawn(move || {
                                read_device(dev, &node, &tx, &*wake, &owner);
                                open.lock().unwrap().remove(&node);
                            });
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => denied = true,
                        Err(_) => {}
                    }
                }
                let status = if !open.lock().unwrap().is_empty() {
                    STATUS_OK
                } else if denied {
                    STATUS_DENIED
                } else {
                    STATUS_NONE
                };
                TOUCHPAD_STATUS.store(status, Ordering::Relaxed);
                std::thread::sleep(Duration::from_secs(3));
            }
        })
        .expect("spawn touchpad thread");
}

/// Event nodes of Sony controller touchpads, from /proc/bus/input/devices.
fn touchpad_nodes() -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string("/proc/bus/input/devices") else { return Vec::new() };
    let mut out = Vec::new();
    for block in text.split("\n\n") {
        let name = block.lines().find_map(|l| l.strip_prefix("N: Name=")).unwrap_or("");
        if !name.contains("Wireless Controller Touchpad") {
            continue;
        }
        if let Some(h) = block.lines().find_map(|l| l.strip_prefix("H: Handlers=")) {
            for w in h.split_whitespace().filter(|w| w.starts_with("event")) {
                out.push(PathBuf::from("/dev/input").join(w));
            }
        }
    }
    out
}

#[derive(Default)]
struct Gesture {
    touching: bool,
    pos: (i32, i32),
    start: (i32, i32),
    anchor: (i32, i32),
    /// Some(true) = vertical
    axis: Option<bool>,
    began: bool,
}

fn read_device(mut dev: evdev::Device, node: &PathBuf, tx: &Sender<Input>, wake: &dyn Fn(), owner: &Owner) {
    let mut g = Gesture::default();
    let mut touch_change: Option<bool> = None;
    let mut click = false;
    let send = |a: Action| {
        let _ = tx.send(Input::Press(a));
        let _ = tx.send(Input::Release(a));
        wake();
    };
    loop {
        let Ok(events) = dev.fetch_events() else { return };
        for ev in events {
            match ev.destructure() {
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_X, v) => g.pos.0 = v,
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_Y, v) => g.pos.1 = v,
                EventSummary::Key(_, KeyCode::BTN_TOUCH, v) => touch_change = Some(v != 0),
                EventSummary::Key(_, KeyCode::BTN_LEFT, 1) => click = true,
                EventSummary::Synchronization(..) => {
                    // A frame is complete: decide what it means.
                    let active = touch_change.is_some() || click || g.touching;
                    if active && !claim(owner, node) {
                        touch_change = None;
                        click = false;
                        continue;
                    }
                    match touch_change.take() {
                        Some(true) => {
                            g.touching = true;
                            g.began = false;
                        }
                        Some(false) => {
                            g.touching = false;
                            g.axis = None;
                        }
                        None => {}
                    }
                    if click {
                        click = false;
                        g.axis = None;
                        g.began = false;
                        send(Action::TouchClick);
                        continue;
                    }
                    if g.touching {
                        if !g.began {
                            // The first frame of a touch carries its position.
                            g.began = true;
                            g.start = g.pos;
                            g.anchor = g.pos;
                            continue;
                        }
                        swipe(&mut g, &send);
                    }
                }
                _ => {}
            }
        }
    }
}

fn swipe(g: &mut Gesture, send: &impl Fn(Action)) {
    let (dx, dy) = (g.pos.0 - g.start.0, g.pos.1 - g.start.1);
    if g.axis.is_none() && dx.abs().max(dy.abs()) > AXIS_LOCK {
        g.axis = Some(dy.abs() >= dx.abs());
        g.anchor = g.pos;
    }
    match g.axis {
        Some(true) => {
            while (g.pos.1 - g.anchor.1).abs() >= STEP_V {
                let down = g.pos.1 > g.anchor.1;
                send(if down { Action::Down } else { Action::Up });
                g.anchor.1 += if down { STEP_V } else { -STEP_V };
            }
        }
        Some(false) => {
            while (g.pos.0 - g.anchor.0).abs() >= STEP_H {
                let right = g.pos.0 > g.anchor.0;
                send(if right { Action::Right } else { Action::Left });
                g.anchor.0 += if right { STEP_H } else { -STEP_H };
            }
        }
        None => {}
    }
}

/// One touchpad at a time (a controller can show up twice, e.g. through a
/// Bluetooth proxy).
fn claim(owner: &Owner, node: &PathBuf) -> bool {
    let mut o = owner.lock().unwrap();
    let now = Instant::now();
    match &*o {
        Some((n, at)) if n != node && now.duration_since(*at) < OWNER_IDLE => false,
        _ => {
            *o = Some((node.clone(), now));
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swipes_map_to_steps() {
        let sent = std::cell::RefCell::new(Vec::new());
        let send = |a: Action| sent.borrow_mut().push(a);
        let mut g = Gesture { touching: true, began: true, ..Default::default() };
        g.start = (900, 100);
        g.anchor = g.start;
        for y in (100..=500).step_by(20) {
            g.pos = (905, y);
            swipe(&mut g, &send);
        }
        // 400 units down: axis locks after 50, then 90 per step from there.
        assert!(sent.borrow().iter().all(|a| *a == Action::Down));
        assert_eq!(sent.borrow().len(), 3);
        sent.borrow_mut().clear();
        let mut g = Gesture { touching: true, began: true, start: (1500, 500), anchor: (1500, 500), ..Default::default() };
        for x in (900..=1500).rev().step_by(30) {
            g.pos = (x, 510);
            swipe(&mut g, &send);
        }
        assert_eq!(*sent.borrow(), vec![Action::Left, Action::Left]);
    }
}
