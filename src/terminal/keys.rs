//! Keys as bytes for the shell, and the quick key sets the pad offers for
//! popular terminal programs.

use alacritty_terminal::term::TermMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum K {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Esc,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Mods {
    pub const NONE: Mods = Mods { ctrl: false, alt: false, shift: false };
    pub const CTRL: Mods = Mods { ctrl: true, alt: false, shift: false };

    /// xterm's modifier parameter (1 = none).
    fn param(self) -> u8 {
        1 + self.shift as u8 + 2 * self.alt as u8 + 4 * self.ctrl as u8
    }
}

/// The bytes a key sends, as xterm does.
pub fn encode(k: K, m: Mods, mode: TermMode) -> Vec<u8> {
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let p = m.param();
    let csi = |tail: &str, num: Option<u8>| -> Vec<u8> {
        match (num, p) {
            (Some(n), 1) => format!("\x1b[{n}~").into_bytes(),
            (Some(n), _) => format!("\x1b[{n};{p}~").into_bytes(),
            (None, 1) if app_cursor => format!("\x1bO{tail}").into_bytes(),
            (None, 1) => format!("\x1b[{tail}").into_bytes(),
            (None, _) => format!("\x1b[1;{p}{tail}").into_bytes(),
        }
    };
    let alt = |mut b: Vec<u8>| {
        if m.alt {
            b.insert(0, 0x1b);
        }
        b
    };
    match k {
        K::Char(c) => {
            if m.ctrl {
                let lc = c.to_ascii_lowercase();
                let code = match lc {
                    'a'..='z' => Some(lc as u8 - b'a' + 1),
                    '@' | ' ' | '2' => Some(0),
                    '[' | '3' => Some(0x1b),
                    '\\' | '4' => Some(0x1c),
                    ']' | '5' => Some(0x1d),
                    '^' | '6' => Some(0x1e),
                    '_' | '-' | '7' => Some(0x1f),
                    '?' | '8' => Some(0x7f),
                    _ => None,
                };
                if let Some(b) = code {
                    return alt(vec![b]);
                }
            }
            alt(c.to_string().into_bytes())
        }
        K::Enter => alt(b"\r".to_vec()),
        K::Tab if m.shift => b"\x1b[Z".to_vec(),
        K::Tab => alt(b"\t".to_vec()),
        K::Backspace if m.ctrl => alt(vec![0x08]),
        K::Backspace => alt(vec![0x7f]),
        K::Esc => alt(vec![0x1b]),
        K::Up => csi("A", None),
        K::Down => csi("B", None),
        K::Right => csi("C", None),
        K::Left => csi("D", None),
        K::Home => csi("H", None),
        K::End => csi("F", None),
        K::Insert => csi("", Some(2)),
        K::Delete => csi("", Some(3)),
        K::PageUp => csi("", Some(5)),
        K::PageDown => csi("", Some(6)),
        K::F(n @ 1..=4) => {
            let c = [b'P', b'Q', b'R', b'S'][n as usize - 1] as char;
            if p == 1 { format!("\x1bO{c}").into_bytes() } else { format!("\x1b[1;{p}{c}").into_bytes() }
        }
        K::F(n) => {
            let code = match n {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                _ => 24,
            };
            csi("", Some(code))
        }
    }
}

/// A button in the quick keys panel.
#[derive(Clone, Copy, Debug)]
pub struct QKey {
    pub label: &'static str,
    /// What it does there, if the key alone doesn't say (see [`hint`]).
    pub hint: &'static str,
    pub send: Send,
}

#[derive(Clone, Copy, Debug)]
pub enum Send {
    Key(K, Mods),
    /// Typed as is (`:wq` and Enter for vim, …).
    Text(&'static str),
}

const fn key(label: &'static str, k: K) -> QKey {
    QKey { label, hint: "", send: Send::Key(k, Mods::NONE) }
}
const fn ctrl(label: &'static str, c: char) -> QKey {
    QKey { label, hint: "", send: Send::Key(K::Char(c), Mods::CTRL) }
}
const fn text(label: &'static str, t: &'static str) -> QKey {
    QKey { label, hint: "", send: Send::Text(t) }
}
const fn with(k: QKey, hint: &'static str) -> QKey {
    QKey { hint, ..k }
}

/// Keys every program can use.
pub const COMMON: &[QKey] = &[
    key("Tab", K::Tab),
    key("Esc", K::Esc),
    ctrl("Ctrl+C", 'c'),
    ctrl("Ctrl+D", 'd'),
    ctrl("Ctrl+Z", 'z'),
    ctrl("Ctrl+L", 'l'),
    ctrl("Ctrl+R", 'r'),
    key("⌫", K::Backspace),
    key("PgUp", K::PageUp),
    key("PgDn", K::PageDown),
    key("Home", K::Home),
    key("End", K::End),
    text("Space", " "),
    text("q", "q"),
    text("y", "y"),
    text("n", "n"),
];

const SHELL: &[QKey] = &[
    ctrl("Ctrl+A", 'a'),
    ctrl("Ctrl+E", 'e'),
    ctrl("Ctrl+U", 'u'),
    ctrl("Ctrl+W", 'w'),
    ctrl("Ctrl+K", 'k'),
    text("|", "|"),
    text("~/", "~/"),
    text("..", ".."),
    text("&&", " && "),
    text("sudo !!", "sudo !!"),
];

const FKEYS: [QKey; 10] = [
    key("F1", K::F(1)),
    key("F2", K::F(2)),
    key("F3", K::F(3)),
    key("F4", K::F(4)),
    key("F5", K::F(5)),
    key("F6", K::F(6)),
    key("F7", K::F(7)),
    key("F8", K::F(8)),
    key("F9", K::F(9)),
    key("F10", K::F(10)),
];

const VIM: &[QKey] = &[
    key("Esc", K::Esc),
    text("i", "i"),
    text(":w", ":w\r"),
    text(":q", ":q\r"),
    text(":wq", ":wq\r"),
    text(":q!", ":q!\r"),
    text("u", "u"),
    ctrl("Ctrl+R", 'r'),
    text("/", "/"),
    text("n", "n"),
    text("dd", "dd"),
    text("yy", "yy"),
    text("p", "p"),
    text("gg", "gg"),
    text("G", "G"),
    text("v", "v"),
];

const NANO: &[QKey] = &[
    with(ctrl("^O", 'o'), "save"),
    with(ctrl("^X", 'x'), "exit"),
    with(ctrl("^W", 'w'), "find"),
    with(ctrl("^K", 'k'), "cut"),
    with(ctrl("^U", 'u'), "paste"),
    with(ctrl("^G", 'g'), "help"),
    with(ctrl("^_", '_'), "line"),
    with(ctrl("^\\", '\\'), "replace"),
];

const TOP: &[QKey] = &[
    text("q", "q"),
    text("/", "/"),
    with(text("k", "k"), "kill"),
    with(text("t", "t"), "tree"),
    with(text("P", "P"), "cpu"),
    with(text("M", "M"), "mem"),
    text("Space", " "),
    with(text("h", "h"), "help"),
];

const BTOP: &[QKey] = &[
    text("q", "q"),
    with(text("m", "m"), "menu"),
    with(text("f", "f"), "filter"),
    with(text("k", "k"), "kill"),
    with(text("e", "e"), "tree"),
    with(text("o", "o"), "options"),
    key("Esc", K::Esc),
    text("+", "+"),
    text("-", "-"),
];

const LESS: &[QKey] = &[
    text("q", "q"),
    text("Space", " "),
    text("b", "b"),
    text("/", "/"),
    text("n", "n"),
    text("N", "N"),
    text("g", "g"),
    text("G", "G"),
    with(text("h", "h"), "help"),
];

const FILES: &[QKey] = &[
    text("q", "q"),
    text("h", "h"),
    text("j", "j"),
    text("k", "k"),
    text("l", "l"),
    text("Space", " "),
    text("/", "/"),
    text("y", "y"),
    text("p", "p"),
    text("d", "d"),
    text("~", "~"),
    text("?", "?"),
];

const LAZYGIT: &[QKey] = &[
    text("q", "q"),
    text("?", "?"),
    text("Space", " "),
    with(text("c", "c"), "commit"),
    with(text("P", "P"), "push"),
    with(text("p", "p"), "pull"),
    with(text("a", "a"), "all"),
    text("[", "["),
    text("]", "]"),
    key("Esc", K::Esc),
];

const TMUX: &[QKey] = &[
    ctrl("Ctrl+B", 'b'),
    text("c", "c"),
    text("n", "n"),
    text("p", "p"),
    text("d", "d"),
    text("%", "%"),
    text("\"", "\""),
    text("[", "["),
];

/// Keys for what's running (`app` is the foreground program), with a title.
pub fn for_app(app: Option<&str>) -> (&'static str, Vec<QKey>) {
    let Some(a) = app else { return ("Shell", SHELL.to_vec()) };
    match a {
        "vim" | "nvim" | "vi" | "view" | "helix" | "hx" | "kak" => ("Vim", VIM.to_vec()),
        "nano" | "pico" | "micro" => ("nano", NANO.to_vec()),
        "htop" | "top" | "atop" | "nvtop" => {
            let f = ["help", "setup", "search", "filter", "tree", "sort", "", "", "kill", "quit"];
            ("htop", TOP.iter().copied().chain(FKEYS.iter().zip(f).map(|(k, h)| with(*k, h))).collect())
        }
        "btop" | "bpytop" | "bashtop" => ("btop", BTOP.to_vec()),
        "mc" | "mcedit" => {
            let f = ["help", "menu", "view", "edit", "copy", "move", "mkdir", "delete", "menu", "quit"];
            let more = [with(key("Insert", K::Insert), "select"), with(ctrl("Ctrl+O", 'o'), "shell"), key("Tab", K::Tab)];
            ("Midnight Commander", FKEYS.iter().zip(f).map(|(k, h)| with(*k, h)).chain(more).collect())
        }
        "less" | "more" | "man" | "most" => ("less", LESS.to_vec()),
        "ranger" | "yazi" | "nnn" | "lf" | "vifm" | "broot" => ("Files", FILES.to_vec()),
        "lazygit" | "tig" | "gitui" => ("Git", LAZYGIT.to_vec()),
        "tmux" | "screen" | "zellij" => ("tmux", TMUX.to_vec()),
        _ => ("Program", FKEYS[..4].to_vec()),
    }
}

/// The hint in the interface language.
pub fn hint(h: &str) -> &'static str {
    use crate::i18n::tr;
    match h {
        "save" => tr("Save"),
        "exit" => tr("Exit"),
        "find" => tr("Find"),
        "cut" => tr("Cut"),
        "paste" => tr("Paste"),
        "help" => tr("Help"),
        "line" => tr("Go to line"),
        "replace" => tr("Replace"),
        "kill" => tr("End process"),
        "tree" => tr("Tree"),
        "cpu" => tr("By CPU"),
        "mem" => tr("By memory"),
        "menu" => tr("Menu"),
        "filter" => tr("Filter"),
        "options" => tr("Options"),
        "commit" => tr("Commit"),
        "push" => tr("Push"),
        "pull" => tr("Pull"),
        "all" => tr("All"),
        "setup" => tr("Settings"),
        "search" => tr("Search"),
        "sort" => tr("Sorting"),
        "quit" => tr("Quit"),
        "view" => tr("View"),
        "edit" => tr("Edit"),
        "copy" => tr("Copy"),
        "move" => tr("Move"),
        "mkdir" => tr("New folder"),
        "delete" => tr("Delete"),
        "select" => tr("Select"),
        "shell" => tr("Shell"),
        _ => "",
    }
}

/// Is this a program with keys of its own here?
pub fn known_app(name: &str) -> bool {
    for_app(Some(name)).0 != "Program"
}

/// Terminal programs worth a launcher entry, when installed.
pub const LAUNCHERS: &[&str] = &["htop", "btop", "nvtop", "nvim", "vim", "nano", "micro", "mc", "yazi", "ranger", "lazygit", "ncdu", "tmux"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xterm_bytes() {
        let m = TermMode::empty();
        assert_eq!(encode(K::Char('c'), Mods::CTRL, m), vec![3]);
        assert_eq!(encode(K::Up, Mods::NONE, m), b"\x1b[A");
        assert_eq!(encode(K::Up, Mods::NONE, TermMode::APP_CURSOR), b"\x1bOA");
        assert_eq!(encode(K::Left, Mods { ctrl: true, ..Mods::NONE }, m), b"\x1b[1;5D");
        assert_eq!(encode(K::F(1), Mods::NONE, m), b"\x1bOP");
        assert_eq!(encode(K::F(5), Mods::NONE, m), b"\x1b[15~");
        assert_eq!(encode(K::Delete, Mods { shift: true, ..Mods::NONE }, m), b"\x1b[3;2~");
        assert_eq!(encode(K::Char('x'), Mods { alt: true, ..Mods::NONE }, m), b"\x1bx");
        assert_eq!(encode(K::Tab, Mods { shift: true, ..Mods::NONE }, m), b"\x1b[Z");
    }
}
