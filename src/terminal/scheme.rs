//! Terminal colour schemes: the 16 ANSI colours, text, background, cursor.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

#[derive(Clone, Debug)]
pub struct Scheme {
    pub name: &'static str,
    pub fg: Rgb,
    pub bg: Rgb,
    pub cursor: Rgb,
    pub ansi: [Rgb; 16],
}

const fn rgb(hex: u32) -> Rgb {
    Rgb { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8 }
}

const fn ansi(c: [u32; 16]) -> [Rgb; 16] {
    let mut out = [Rgb { r: 0, g: 0, b: 0 }; 16];
    let mut i = 0;
    while i < 16 {
        out[i] = rgb(c[i]);
        i += 1;
    }
    out
}

/// "Polka" follows the app: these two, by light or dark.
const POLKA_DARK: Scheme = Scheme {
    name: "Polka",
    fg: rgb(0xdcdce0),
    bg: rgb(0x16161a),
    cursor: rgb(0x8fb8ff),
    ansi: ansi([
        0x2a2a30, 0xf2777a, 0x99cc99, 0xffcc66, 0x6699cc, 0xcc99cc, 0x66cccc, 0xd0d0d6, 0x5c5c66, 0xff8f92, 0xb5e8b5, 0xffe08a, 0x8fb8ff, 0xe0b5e0,
        0x8ae6e6, 0xffffff,
    ]),
};

const POLKA_LIGHT: Scheme = Scheme {
    name: "Polka",
    fg: rgb(0x2a2a30),
    bg: rgb(0xfafafb),
    cursor: rgb(0x2f6fdb),
    ansi: ansi([
        0x2a2a30, 0xc8262b, 0x3d8b37, 0x9a6a00, 0x2f6fdb, 0x9c3fb0, 0x1b8a8f, 0xa0a0a8, 0x6a6a74, 0xe0393e, 0x4fa548, 0xb88400, 0x4a86f0, 0xb455c9,
        0x26a3a9, 0x2a2a30,
    ]),
};

pub const PRESETS: &[Scheme] = &[
    POLKA_DARK,
    Scheme {
        name: "Tango",
        fg: rgb(0xd3d7cf),
        bg: rgb(0x1e1e1e),
        cursor: rgb(0xd3d7cf),
        ansi: ansi([
            0x2e3436, 0xcc0000, 0x4e9a06, 0xc4a000, 0x3465a4, 0x75507b, 0x06989a, 0xd3d7cf, 0x555753, 0xef2929, 0x8ae234, 0xfce94f, 0x729fcf, 0xad7fa8,
            0x34e2e2, 0xeeeeec,
        ]),
    },
    Scheme {
        name: "Solarized Dark",
        fg: rgb(0x839496),
        bg: rgb(0x002b36),
        cursor: rgb(0x93a1a1),
        ansi: ansi([
            0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5, 0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4,
            0x93a1a1, 0xfdf6e3,
        ]),
    },
    Scheme {
        name: "Solarized Light",
        fg: rgb(0x657b83),
        bg: rgb(0xfdf6e3),
        cursor: rgb(0x586e75),
        ansi: ansi([
            0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5, 0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4,
            0x93a1a1, 0xfdf6e3,
        ]),
    },
    Scheme {
        name: "Dracula",
        fg: rgb(0xf8f8f2),
        bg: rgb(0x282a36),
        cursor: rgb(0xf8f8f2),
        ansi: ansi([
            0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2, 0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df,
            0xa4ffff, 0xffffff,
        ]),
    },
    Scheme {
        name: "Gruvbox",
        fg: rgb(0xebdbb2),
        bg: rgb(0x282828),
        cursor: rgb(0xebdbb2),
        ansi: ansi([
            0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984, 0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b,
            0x8ec07c, 0xebdbb2,
        ]),
    },
    Scheme {
        name: "Nord",
        fg: rgb(0xd8dee9),
        bg: rgb(0x2e3440),
        cursor: rgb(0xd8dee9),
        ansi: ansi([
            0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0, 0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead,
            0x8fbcbb, 0xeceff4,
        ]),
    },
    Scheme {
        name: "Catppuccin Mocha",
        fg: rgb(0xcdd6f4),
        bg: rgb(0x1e1e2e),
        cursor: rgb(0xf5e0dc),
        ansi: ansi([
            0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de, 0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7,
            0x94e2d5, 0xa6adc8,
        ]),
    },
];

/// The scheme called `name` (Polka in the app's light or dark).
pub fn resolve(name: &str, dark: bool) -> Scheme {
    if name == "Polka" || !PRESETS.iter().any(|s| s.name == name) {
        return if dark { POLKA_DARK } else { POLKA_LIGHT };
    }
    PRESETS.iter().find(|s| s.name == name).cloned().unwrap_or(POLKA_DARK)
}

fn dim(c: Rgb) -> Rgb {
    Rgb { r: (c.r as u16 * 2 / 3) as u8, g: (c.g as u16 * 2 / 3) as u8, b: (c.b as u16 * 2 / 3) as u8 }
}

impl Scheme {
    /// Colour by alacritty's index: 0–15 ANSI, 16–255 the xterm cube and
    /// greys, then the named ones (text, background, cursor, dim…).
    pub fn rgb(&self, i: usize) -> Rgb {
        match i {
            0..=15 => self.ansi[i],
            16..=231 => {
                let i = i - 16;
                let v = |x: usize| if x == 0 { 0 } else { (55 + x * 40) as u8 };
                Rgb { r: v(i / 36), g: v(i / 6 % 6), b: v(i % 6) }
            }
            232..=255 => {
                let g = (8 + (i - 232) * 10) as u8;
                Rgb { r: g, g, b: g }
            }
            256 => self.fg,
            257 => self.bg,
            258 => self.cursor,
            259..=266 => dim(self.ansi[i - 259]),
            267 => self.fg,
            _ => dim(self.fg),
        }
    }

    /// A cell's colour, with what the program changed (OSC 4/10/11).
    pub fn color(&self, c: Color, overrides: &Colors, bold: bool, dimmed: bool) -> Rgb {
        let idx = match c {
            Color::Spec(rgb) => return if dimmed { dim(rgb) } else { rgb },
            Color::Indexed(i) => {
                let i = i as usize;
                // Bold makes the first eight bright, as most terminals do.
                if bold && i < 8 { i + 8 } else { i }
            }
            Color::Named(n) => {
                let mut i = n as usize;
                if bold && i < 8 {
                    i += 8;
                } else if dimmed && i < 8 {
                    i = NamedColor::DimBlack as usize + i;
                } else if dimmed && i == NamedColor::Foreground as usize {
                    i = NamedColor::DimForeground as usize;
                }
                i
            }
        };
        overrides[idx].unwrap_or_else(|| self.rgb(idx))
    }
}

pub fn names() -> Vec<&'static str> {
    PRESETS.iter().map(|s| s.name).collect()
}

pub fn c32(c: Rgb) -> egui::Color32 {
    egui::Color32::from_rgb(c.r, c.g, c.b)
}
