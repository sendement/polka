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
    /// Text from the clipboard.
    Paste,
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
    /// Dead keys: the accent goes onto the next letter (´ + e = é).
    pub dead: &'static str,
    pub script: Script,
}

/// How a layout's keys combine as they're typed.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Script {
    Plain,
    /// Jamo build syllables (두벌식).
    Hangul,
    /// Kana: ゛ ゜ voice the one before; Caps gives katakana.
    Kana,
}

const fn lay(name: &'static str, rows: [(&'static str, &'static str); 4], dead: &'static str, script: Script) -> Layout {
    Layout { name, rows, dead, script }
}

pub const LAYOUTS: [Layout; 8] = [
    lay("EN", [("`1234567890-=", "~!@#$%^&*()_+"), ("qwertyuiop[]\\", "QWERTYUIOP{}|"), ("asdfghjkl;'", "ASDFGHJKL:\""), ("zxcvbnm,./", "ZXCVBNM<>?")], "", Script::Plain),
    lay("RU", [("ё1234567890-=", "Ё!\"№;%:?*()_+"), ("йцукенгшщзхъ\\", "ЙЦУКЕНГШЩЗХЪ/"), ("фывапролджэ", "ФЫВАПРОЛДЖЭ"), ("ячсмитьбю.", "ЯЧСМИТЬБЮ,")], "", Script::Plain),
    lay("UK", [("ґ1234567890-=", "Ґ!\"№;%:?*()_+"), ("йцукенгшщзхї\\", "ЙЦУКЕНГШЩЗХЇ/"), ("фівапролджє", "ФІВАПРОЛДЖЄ"), ("ячсмитьбю.", "ЯЧСМИТЬБЮ,")], "", Script::Plain),
    lay("DE", [("^1234567890ß´", "°!\"§$%&/()=?`"), ("qwertzuiopü+#", "QWERTZUIOPÜ*'"), ("asdfghjklöä", "ASDFGHJKLÖÄ"), ("yxcvbnm,.-", "YXCVBNM;:_")], "^´`", Script::Plain),
    lay("FR", [("²&é\"'(-è_çà)=", "~1234567890°+"), ("azertyuiop^$*", "AZERTYUIOP¨£µ"), ("qsdfghjklmù", "QSDFGHJKLM%"), ("wxcvbn,;:!", "WXCVBN?./§")], "^¨", Script::Plain),
    lay("ES", [("º1234567890'¡", "ª!\"·$%&/()=?¿"), ("qwertyuiop`+ç", "QWERTYUIOP^*Ç"), ("asdfghjklñ´", "ASDFGHJKLÑ¨"), ("zxcvbnm,.-", "ZXCVBNM;:_")], "´`^¨", Script::Plain),
    lay("한", [("`1234567890-=", "~!@#$%^&*()_+"), ("ㅂㅈㄷㄱㅅㅛㅕㅑㅐㅔ[]\\", "ㅃㅉㄸㄲㅆㅛㅕㅑㅒㅖ{}|"), ("ㅁㄴㅇㄹㅎㅗㅓㅏㅣ;'", "ㅁㄴㅇㄹㅎㅗㅓㅏㅣ:\""), ("ㅋㅌㅊㅍㅠㅜㅡ,./", "ㅋㅌㅊㅍㅠㅜㅡ<>?")], "", Script::Hangul),
    lay("あ", [("ぬふあうえおやゆよわほへろ", "ぬふぁぅぇぉゃゅょをほへー"), ("たていすかんなにらせ゛゜む", "たてぃすかんなにらせ゛「」"), ("ちとしはきくまのりれけ", "ちとしはきくまのりれけ"), ("つさそひこみもねるめ", "っさそひこみも、。・")], "", Script::Kana),
];

/// Codes the settings use (the names, but Latin for the CJK ones).
pub const CODES: [&str; 8] = ["EN", "RU", "UK", "DE", "FR", "ES", "KO", "JA"];

/// Layouts the ⇄ key goes through (indices; never empty).
static ENABLED: std::sync::RwLock<Vec<usize>> = std::sync::RwLock::new(Vec::new());

/// The layouts chosen in the settings (codes).
pub fn set_enabled(codes: &[String]) {
    let mut v: Vec<usize> = CODES.iter().enumerate().filter(|(_, c)| codes.iter().any(|x| x == *c)).map(|(i, _)| i).collect();
    if v.is_empty() {
        v.push(0);
    }
    if let Ok(mut e) = ENABLED.write() {
        *e = v;
    }
}

fn enabled() -> Vec<usize> {
    let v = ENABLED.read().map(|e| e.clone()).unwrap_or_default();
    if v.is_empty() { vec![0, 1] } else { v }
}

/// The layout after `cur` among the chosen ones.
pub fn next_layout(cur: usize) -> usize {
    let e = enabled();
    let i = e.iter().position(|&x| x == cur).map_or(0, |i| (i + 1) % e.len());
    e[i]
}

/// Which layouts to start with: English and the interface's own.
pub fn default_codes(ui_lang: &str) -> Vec<String> {
    let own = match ui_lang {
        "ru" => "RU",
        "uk" => "UK",
        "de" => "DE",
        "fr" => "FR",
        "es" => "ES",
        "ko" => "KO",
        "ja" => "JA",
        _ => "",
    };
    let mut v = vec!["EN".to_string()];
    if !own.is_empty() {
        v.push(own.into());
    }
    v
}

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
            k(OskKey::Space, 3.5),
            k(OskKey::Paste, 1.5),
            k(OskKey::Left, 1.0),
            k(OskKey::Up, 1.0),
            k(OskKey::Down, 1.0),
            k(OskKey::Right, 1.0),
            k(OskKey::Ok, 2.0),
        ],
    }
}

pub const ROWS: usize = 5;

/// The character a key types: Shift gives the other symbol; letters follow
/// Caps Lock too (Shift with Caps gives lower case back).
pub fn char_at(layout: usize, r: usize, i: usize, shift: bool, caps: bool) -> Option<char> {
    let l = LAYOUTS.get(layout)?;
    let (base, shifted) = l.rows.get(r).copied()?;
    let b = base.chars().nth(i)?;
    let s = shifted.chars().nth(i)?;
    if l.script == Script::Kana {
        let c = if shift { s } else { b };
        return Some(if caps { katakana(c) } else { c });
    }
    let upper = if b.is_alphabetic() && b.to_uppercase().next() != Some(b) { shift != caps } else { shift };
    Some(if upper { s } else { b })
}

/// Hiragana as katakana.
pub fn katakana(c: char) -> char {
    match c {
        'ぁ'..='ゖ' => char::from_u32(c as u32 + 0x60).unwrap_or(c),
        _ => c,
    }
}

/// The other character on a symbol key (shown small in its corner).
pub fn alt_at(layout: usize, r: usize, i: usize, shift: bool) -> Option<char> {
    let (base, shifted) = LAYOUTS.get(layout)?.rows.get(r).copied()?;
    let b = base.chars().nth(i)?;
    let s = shifted.chars().nth(i)?;
    // Letters show their case on the key itself; kana and jamo alike.
    if b.is_alphabetic() && (b.to_uppercase().next() != Some(b) || b == s) {
        return None;
    }
    if shift { Some(b) } else { Some(s) }
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
    let l = LAST.load(Ordering::Relaxed) % LAYOUTS.len();
    // Still one of the chosen ones?
    let e = enabled();
    if e.contains(&l) { l } else { e[0] }
}

pub fn set_last_layout(i: usize) {
    LAST.store(i % LAYOUTS.len(), Ordering::Relaxed);
}

// ---------------------------------------------------------------- typing

/// What the character just typed does to the one before it (typed just
/// before, in the same layout): None leaves it, Some(text) replaces it.
pub fn compose(layout: usize, prev: char, c: char) -> Option<String> {
    let l = LAYOUTS.get(layout)?;
    if l.dead.contains(prev) {
        return accent(prev, c).map(String::from);
    }
    match l.script {
        Script::Hangul => hangul(prev, c),
        Script::Kana => voiced(prev, c).map(String::from),
        Script::Plain => None,
    }
}

/// An accent (dead key) on a letter.
fn accent(dead: char, c: char) -> Option<char> {
    let (from, to) = match dead {
        '´' | '\'' => ("aeiouyAEIOUY", "áéíóúýÁÉÍÓÚÝ"),
        '`' => ("aeiouAEIOU", "àèìòùÀÈÌÒÙ"),
        '^' => ("aeiouAEIOU", "âêîôûÂÊÎÔÛ"),
        '¨' => ("aeiouyAEIOU", "äëïöüÿÄËÏÖÜ"),
        '~' => ("anoANO", "ãñõÃÑÕ"),
        _ => return None,
    };
    let i = from.chars().position(|x| x == c)?;
    to.chars().nth(i)
}

/// ゛ / ゜ on the kana before (hiragana or katakana).
fn voiced(prev: char, c: char) -> Option<char> {
    let kata = ('ァ'..='ヶ').contains(&prev);
    let h = if kata { char::from_u32(prev as u32 - 0x60)? } else { prev };
    let v = match c {
        '゛' if "かきくけこさしすせそたちつてとはひふへほ".contains(h) => char::from_u32(h as u32 + 1)?,
        '゛' if h == 'う' => 'ゔ',
        '゜' if "はひふへほ".contains(h) => char::from_u32(h as u32 + 2)?,
        _ => return None,
    };
    Some(if kata { katakana(v) } else { v })
}

const CHO: &str = "ㄱㄲㄴㄷㄸㄹㅁㅂㅃㅅㅆㅇㅈㅉㅊㅋㅌㅍㅎ";
const JUNG: &str = "ㅏㅐㅑㅒㅓㅔㅕㅖㅗㅘㅙㅚㅛㅜㅝㅞㅟㅠㅡㅢㅣ";
/// Final consonants (index 0: none).
const JONG: &str = " ㄱㄲㄳㄴㄵㄶㄷㄹㄺㄻㄼㄽㄾㄿㅀㅁㅂㅄㅅㅆㅇㅈㅊㅋㅌㅍㅎ";

fn idx(set: &str, c: char) -> Option<u32> {
    set.chars().position(|x| x == c).map(|i| i as u32)
}

fn syllable(cho: u32, jung: u32, jong: u32) -> char {
    char::from_u32(0xAC00 + (cho * 21 + jung) * 28 + jong).unwrap_or(' ')
}

/// Two vowels that make one (ㅗ + ㅏ = ㅘ).
fn vowel_pair(a: char, b: char) -> Option<char> {
    Some(match (a, b) {
        ('ㅗ', 'ㅏ') => 'ㅘ',
        ('ㅗ', 'ㅐ') => 'ㅙ',
        ('ㅗ', 'ㅣ') => 'ㅚ',
        ('ㅜ', 'ㅓ') => 'ㅝ',
        ('ㅜ', 'ㅔ') => 'ㅞ',
        ('ㅜ', 'ㅣ') => 'ㅟ',
        ('ㅡ', 'ㅣ') => 'ㅢ',
        _ => return None,
    })
}

/// Two finals that make one (ㄹ + ㄱ = ㄺ), and back.
const FINAL_PAIRS: [(char, char, char); 11] = [
    ('ㄱ', 'ㅅ', 'ㄳ'),
    ('ㄴ', 'ㅈ', 'ㄵ'),
    ('ㄴ', 'ㅎ', 'ㄶ'),
    ('ㄹ', 'ㄱ', 'ㄺ'),
    ('ㄹ', 'ㅁ', 'ㄻ'),
    ('ㄹ', 'ㅂ', 'ㄼ'),
    ('ㄹ', 'ㅅ', 'ㄽ'),
    ('ㄹ', 'ㅌ', 'ㄾ'),
    ('ㄹ', 'ㅍ', 'ㄿ'),
    ('ㄹ', 'ㅎ', 'ㅀ'),
    ('ㅂ', 'ㅅ', 'ㅄ'),
];

/// A jamo typed after `prev`: the syllable it builds (maybe two, when a
/// final consonant moves on to start the next one).
fn hangul(prev: char, c: char) -> Option<String> {
    let vowel = idx(JUNG, c).is_some();
    // A lone consonant and a vowel: a syllable.
    if let (Some(cho), Some(jung)) = (idx(CHO, prev), idx(JUNG, c)) {
        return Some(syllable(cho, jung, 0).to_string());
    }
    let code = prev as u32;
    if !(0xAC00..=0xD7A3).contains(&code) {
        return None;
    }
    let s = code - 0xAC00;
    let (cho, jung, jong) = (s / 588, (s % 588) / 28, s % 28);
    let jung_c = JUNG.chars().nth(jung as usize)?;
    if jong == 0 {
        if vowel {
            let v = vowel_pair(jung_c, c)?;
            return Some(syllable(cho, idx(JUNG, v)?, jong).to_string());
        }
        let f = idx(JONG, c).filter(|&f| f > 0)?;
        return Some(syllable(cho, jung, f).to_string());
    }
    let jong_c = JONG.chars().nth(jong as usize)?;
    if vowel {
        // The final starts the next syllable (the second half of a pair).
        let (keep, moved) = match FINAL_PAIRS.iter().find(|(_, _, p)| *p == jong_c) {
            Some((a, b, _)) => (idx(JONG, *a)?, *b),
            None => (0, jong_c),
        };
        let next = syllable(idx(CHO, moved)?, idx(JUNG, c)?, 0);
        return Some(format!("{}{next}", syllable(cho, jung, keep)));
    }
    let pair = FINAL_PAIRS.iter().find(|(a, b, _)| *a == jong_c && *b == c)?.2;
    Some(syllable(cho, jung, idx(JONG, pair)?).to_string())
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
        // Typing builds characters.
        let ko = CODES.iter().position(|c| *c == "KO").unwrap();
        let ja = CODES.iter().position(|c| *c == "JA").unwrap();
        let de = CODES.iter().position(|c| *c == "DE").unwrap();
        let typed = |l: usize, keys: &str| {
            let mut out: Vec<char> = Vec::new();
            for c in keys.chars() {
                match out.last().copied().and_then(|p| compose(l, p, c)) {
                    Some(r) => {
                        out.pop();
                        out.extend(r.chars());
                    }
                    None => out.push(c),
                }
            }
            out.into_iter().collect::<String>()
        };
        assert_eq!(typed(ko, "ㅎㅏㄴㄱㅡㄹ"), "한글");
        assert_eq!(typed(ko, "ㅇㅣㄹㄱㅇㅓ"), "읽어");
        assert_eq!(typed(ko, "ㄱㅗㅏ"), "과");
        assert_eq!(typed(ja, "か゛は゜"), "がぱ");
        assert_eq!(typed(de, "´e^a"), "éâ");
        assert_eq!(katakana('か'), 'カ');
        assert_eq!(char_at(ja, 1, 4, false, true), Some('カ'));
    }
}
