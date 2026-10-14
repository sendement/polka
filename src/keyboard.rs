//! The on-screen keyboard's keys: a real keyboard's main block (no F keys,
//! no numpad) in several layouts, with what Shift gives on every key.

use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OskKey {
    Char(char),
    Backspace,
    Tab,
    Caps,
    Enter,
    Shift,
    Ctrl,
    Alt,
    Lang,
    Space,
    Left,
    Up,
    Down,
    Right,
    Ok,
}

/// A key's place: a character of the layout's row, or a fixed key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Ch(usize),
    Key(OskKey),
}

pub struct Layout {
    pub name: &'static str,
    /// Per row: the keys as they are, and with Shift.
    pub rows: [(&'static str, &'static str); 4],
}

pub const LAYOUTS: [Layout; 3] = [
    Layout {
        name: "EN",
        rows: [
            ("`1234567890-=", "~!@#$%^&*()_+"),
            ("qwertyuiop[]\\", "QWERTYUIOP{}|"),
            ("asdfghjkl;'", "ASDFGHJKL:\""),
            ("zxcvbnm,./", "ZXCVBNM<>?"),
        ],
    },
    Layout {
        name: "RU",
        rows: [
            ("ё1234567890-=", "Ё!\"№;%:?*()_+"),
            ("йцукенгшщзхъ\\", "ЙЦУКЕНГШЩЗХЪ/"),
            ("фывапролджэ", "ФЫВАПРОЛДЖЭ"),
            ("ячсмитьбю.", "ЯЧСМИТЬБЮ,"),
        ],
    },
    Layout {
        name: "UK",
        rows: [
            ("ґ1234567890-=", "Ґ!\"№;%:?*()_+"),
            ("йцукенгшщзхї\\", "ЙЦУКЕНГШЩЗХЇ/"),
            ("фівапролджє", "ФІВАПРОЛДЖЄ"),
            ("ячсмитьбю.", "ЯЧСМИТЬБЮ,"),
        ],
    },
];

/// Rows of the keyboard: what each key is and how wide (in key widths).
pub fn row(r: usize) -> Vec<(Slot, f32)> {
    let chars = |n: usize| (0..n).map(|i| (Slot::Ch(i), 1.0));
    let k = |key: OskKey, w: f32| (Slot::Key(key), w);
    match r {
        0 => chars(13).chain([k(OskKey::Backspace, 2.0)]).collect(),
        1 => [k(OskKey::Tab, 1.5)].into_iter().chain(chars(12)).chain([(Slot::Ch(12), 1.5)]).collect(),
        2 => [k(OskKey::Caps, 1.75)].into_iter().chain(chars(11)).chain([k(OskKey::Enter, 2.25)]).collect(),
        3 => [k(OskKey::Shift, 2.25)].into_iter().chain(chars(10)).chain([k(OskKey::Shift, 2.75)]).collect(),
        _ => vec![
            k(OskKey::Ctrl, 1.25),
            k(OskKey::Alt, 1.25),
            k(OskKey::Lang, 1.5),
            k(OskKey::Space, 4.5),
            k(OskKey::Left, 1.0),
            k(OskKey::Up, 1.0),
            k(OskKey::Down, 1.0),
            k(OskKey::Right, 1.0),
            k(OskKey::Ok, 2.5),
        ],
    }
}

pub const ROWS: usize = 5;

/// The character a key types: Shift gives the other symbol; letters follow
/// Caps Lock too (Shift with Caps gives lower case back).
pub fn char_at(layout: usize, r: usize, i: usize, shift: bool, caps: bool) -> Option<char> {
    let (base, shifted) = LAYOUTS.get(layout)?.rows.get(r).copied()?;
    let b = base.chars().nth(i)?;
    let s = shifted.chars().nth(i)?;
    let upper = if b.is_alphabetic() { shift != caps } else { shift };
    Some(if upper { s } else { b })
}

/// The other character on a symbol key (shown small in its corner).
pub fn alt_at(layout: usize, r: usize, i: usize, shift: bool) -> Option<char> {
    let (base, shifted) = LAYOUTS.get(layout)?.rows.get(r).copied()?;
    let b = base.chars().nth(i)?;
    if b.is_alphabetic() {
        return None;
    }
    if shift { Some(b) } else { shifted.chars().nth(i) }
}

/// Middle of key `i` of row `r`, in key widths from the left.
pub fn center(r: usize, i: usize) -> f32 {
    let keys = row(r);
    keys[..i].iter().map(|(_, w)| w).sum::<f32>() + keys.get(i).map_or(0.0, |(_, w)| w / 2.0)
}

/// The key under the same spot one row up or down.
pub fn nearest(r: usize, x: f32) -> usize {
    (0..row(r).len()).min_by(|a, b| (center(r, *a) - x).abs().total_cmp(&(center(r, *b) - x).abs())).unwrap_or(0)
}

/// The layout the last keyboard was left in.
static LAST: AtomicUsize = AtomicUsize::new(0);

pub fn last_layout() -> usize {
    LAST.load(Ordering::Relaxed) % LAYOUTS.len()
}

pub fn set_last_layout(i: usize) {
    LAST.store(i % LAYOUTS.len(), Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_fit() {
        for l in &LAYOUTS {
            for (r, (base, shifted)) in l.rows.iter().enumerate() {
                let need = row(r).iter().filter(|(s, _)| matches!(s, Slot::Ch(_))).count();
                assert_eq!(base.chars().count(), need, "{} row {r}", l.name);
                assert_eq!(shifted.chars().count(), need, "{} row {r} shifted", l.name);
            }
        }
        for r in 0..ROWS {
            let w: f32 = row(r).iter().map(|(_, w)| w).sum();
            assert!((w - 15.0).abs() < 0.01, "row {r} is {w} wide");
        }
        assert_eq!(char_at(0, 0, 1, true, false), Some('!'));
        assert_eq!(char_at(1, 0, 3, true, false), Some('№'));
        assert_eq!(char_at(0, 1, 0, false, true), Some('Q'));
        assert_eq!(char_at(0, 1, 0, true, true), Some('q'));
        assert_eq!(char_at(0, 0, 1, false, true), Some('1'));
    }
}
