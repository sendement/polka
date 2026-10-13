//! Colour themes for the text viewer.

use egui::Color32;

/// A palette; [`Theme::class_color`] maps highlight names onto it.
#[derive(Clone, Debug)]
pub struct Theme {
    pub name: &'static str,
    pub bg: Color32,
    pub fg: Color32,
    pub gutter: Color32,
    /// Background of the line with the cursor.
    pub line: Color32,
    pub comment: Color32,
    pub keyword: Color32,
    pub string: Color32,
    pub number: Color32,
    pub function: Color32,
    pub ty: Color32,
    pub builtin: Color32,
    pub property: Color32,
    pub operator: Color32,
    pub punct: Color32,
    pub tag: Color32,
    pub attribute: Color32,
    pub heading: Color32,
    pub link: Color32,
    pub escape: Color32,
    pub added: Color32,
    pub removed: Color32,
    pub macro_: Color32,
}

const fn hex(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

macro_rules! theme {
    ($name:expr; $bg:expr, $fg:expr, $gutter:expr, $line:expr; $comment:expr, $keyword:expr, $string:expr, $number:expr, $function:expr, $ty:expr, $builtin:expr, $property:expr, $operator:expr, $punct:expr, $tag:expr, $attribute:expr, $heading:expr, $link:expr, $escape:expr, $added:expr, $removed:expr, $macro_:expr) => {
        Theme {
            name: $name,
            bg: hex($bg),
            fg: hex($fg),
            gutter: hex($gutter),
            line: hex($line),
            comment: hex($comment),
            keyword: hex($keyword),
            string: hex($string),
            number: hex($number),
            function: hex($function),
            ty: hex($ty),
            builtin: hex($builtin),
            property: hex($property),
            operator: hex($operator),
            punct: hex($punct),
            tag: hex($tag),
            attribute: hex($attribute),
            heading: hex($heading),
            link: hex($link),
            escape: hex($escape),
            added: hex($added),
            removed: hex($removed),
            macro_: hex($macro_),
        }
    };
}

/// Built-in presets. The first one follows the app's own look.
pub fn presets() -> Vec<Theme> {
    vec![
        theme!("Polka"; 0x1b1b1b, 0xd4d4d4, 0x6a6a6a, 0x262626;
            0x7a8288, 0x62b1e6, 0xe3a35f, 0xc7a0f0, 0x7fd1b9, 0x6cc4e8, 0xf08aa0, 0xa8c8e8, 0x9fb8c8, 0x9a9a9a, 0x62b1e6, 0xe3c46f, 0x6cc4e8, 0x7fb4e8, 0xf0b070, 0x8bd17c, 0xf07a7a, 0x7fd1b9),
        theme!("One Dark"; 0x282c34, 0xabb2bf, 0x4b5263, 0x2c313c;
            0x5c6370, 0xc678dd, 0x98c379, 0xd19a66, 0x61afef, 0xe5c07b, 0xe06c75, 0xe06c75, 0x56b6c2, 0xabb2bf, 0xe06c75, 0xd19a66, 0xe06c75, 0x61afef, 0x56b6c2, 0x98c379, 0xe06c75, 0x56b6c2),
        theme!("Dracula"; 0x282a36, 0xf8f8f2, 0x6272a4, 0x343746;
            0x6272a4, 0xff79c6, 0xf1fa8c, 0xbd93f9, 0x50fa7b, 0x8be9fd, 0xbd93f9, 0x8be9fd, 0xff79c6, 0xf8f8f2, 0xff79c6, 0x50fa7b, 0xbd93f9, 0x8be9fd, 0xff79c6, 0x50fa7b, 0xff5555, 0x50fa7b),
        theme!("Catppuccin Mocha"; 0x1e1e2e, 0xcdd6f4, 0x585b70, 0x2a2b3c;
            0x6c7086, 0xcba6f7, 0xa6e3a1, 0xfab387, 0x89b4fa, 0xf9e2af, 0xf38ba8, 0xb4befe, 0x89dceb, 0x9399b2, 0x89b4fa, 0xf9e2af, 0xf38ba8, 0x89b4fa, 0xf5c2e7, 0xa6e3a1, 0xf38ba8, 0x94e2d5),
        theme!("Gruvbox Dark"; 0x282828, 0xebdbb2, 0x7c6f64, 0x32302f;
            0x928374, 0xfb4934, 0xb8bb26, 0xd3869b, 0x8ec07c, 0xfabd2f, 0xfe8019, 0x83a598, 0xfe8019, 0xa89984, 0xfb4934, 0xfabd2f, 0xfabd2f, 0x83a598, 0xfe8019, 0xb8bb26, 0xfb4934, 0x8ec07c),
        theme!("Nord"; 0x2e3440, 0xd8dee9, 0x4c566a, 0x3b4252;
            0x616e88, 0x81a1c1, 0xa3be8c, 0xb48ead, 0x88c0d0, 0x8fbcbb, 0x81a1c1, 0x8fbcbb, 0x81a1c1, 0xeceff4, 0x81a1c1, 0x8fbcbb, 0x88c0d0, 0x88c0d0, 0xebcb8b, 0xa3be8c, 0xbf616a, 0x5e81ac),
        theme!("Monokai"; 0x272822, 0xf8f8f2, 0x90908a, 0x3e3d32;
            0x75715e, 0xf92672, 0xe6db74, 0xae81ff, 0xa6e22e, 0x66d9ef, 0xfd971f, 0xf8f8f2, 0xf92672, 0xf8f8f2, 0xf92672, 0xa6e22e, 0xa6e22e, 0x66d9ef, 0xae81ff, 0xa6e22e, 0xf92672, 0x66d9ef),
        theme!("Solarized Light"; 0xfdf6e3, 0x586e75, 0x93a1a1, 0xeee8d5;
            0x93a1a1, 0x859900, 0x2aa198, 0xd33682, 0x268bd2, 0xb58900, 0xcb4b16, 0x268bd2, 0x859900, 0x657b83, 0x268bd2, 0xb58900, 0xcb4b16, 0x6c71c4, 0xdc322f, 0x859900, 0xdc322f, 0x6c71c4),
        theme!("GitHub Light"; 0xffffff, 0x24292f, 0x8c959f, 0xf3f5f7;
            0x6e7781, 0xcf222e, 0x0a3069, 0x0550ae, 0x8250df, 0x953800, 0x0550ae, 0x0550ae, 0xcf222e, 0x24292f, 0x116329, 0x0550ae, 0x0550ae, 0x0a3069, 0x0a3069, 0x116329, 0x82071e, 0x8250df),
    ]
}

/// The theme to draw with: "Polka" follows the app into its light look.
pub fn resolve(name: &str, dark: bool) -> Theme {
    if name == "Polka" && !dark {
        let mut t = theme!("Polka"; 0xfbfbfb, 0x24292f, 0x9a9a9a, 0xeef1f4;
            0x6e7781, 0x0b62b8, 0xa04d00, 0x7a3fc4, 0x0f7a60, 0x066a8e, 0xc0254a, 0x3a5f88, 0x4f6575, 0x5f6368, 0x0b62b8, 0x8a6400, 0x066a8e, 0x0b62b8, 0xb05a00, 0x1a7f37, 0xc0254a, 0x0f7a60);
        t.name = "Polka";
        return t;
    }
    by_name(name)
}

pub fn by_name(name: &str) -> Theme {
    let all = presets();
    all.iter().find(|t| t.name == name).cloned().unwrap_or_else(|| all[0].clone())
}

impl Theme {
    /// Colour for a highlight name from [`super::syntax::NAMES`].
    pub fn class_color(&self, name: &str) -> Color32 {
        let head = name.split('.').next().unwrap_or(name);
        match name {
            "comment" => self.comment,
            "string.escape" | "escape" | "string.special" | "character" => self.escape,
            "function.macro" | "preproc" => self.macro_,
            "function.builtin" | "variable.builtin" | "constant.builtin" | "type.builtin" | "boolean" => self.builtin,
            "punctuation.special" => self.operator,
            "markup.heading" | "text.title" => self.heading,
            "markup.link" | "text.uri" | "text.reference" => self.link,
            "markup.raw" | "text.literal" => self.string,
            "markup.bold" | "text.strong" => self.ty,
            "variable.parameter" | "parameter" => self.fg,
            "addition" | "diff.plus" => self.added,
            "deletion" | "diff.minus" => self.removed,
            _ => match head {
                "keyword" | "conditional" | "repeat" | "include" | "exception" | "label" => self.keyword,
                "string" => self.string,
                "number" | "constant" => self.number,
                "function" | "method" | "constructor" => self.function,
                "type" | "namespace" | "module" => self.ty,
                "property" | "field" => self.property,
                "operator" => self.operator,
                "punctuation" | "delimiter" => self.punct,
                "tag" => self.tag,
                "attribute" => self.attribute,
                _ => self.fg,
            },
        }
    }

    /// Colours for every class, in [`super::syntax::NAMES`] order.
    pub fn table(&self) -> Vec<Color32> {
        super::syntax::NAMES.iter().map(|n| self.class_color(n)).collect()
    }
}
