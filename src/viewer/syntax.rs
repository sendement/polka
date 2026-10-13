//! Syntax highlighting with tree-sitter. Each language is a grammar plus its
//! highlight query (some build on another one: TypeScript on JavaScript,
//! C++ on C). Code inside other code (Markdown fences, `<script>` in HTML)
//! is highlighted through injections.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

/// Capture names we colour. tree-sitter-highlight picks the longest
/// configured prefix of a capture (`function.method` → `function`).
pub const NAMES: &[&str] = &[
    "attribute",
    "boolean",
    "character",
    "comment",
    "conditional",
    "constant",
    "constant.builtin",
    "constructor",
    "delimiter",
    "embedded",
    "escape",
    "exception",
    "field",
    "function",
    "function.builtin",
    "function.macro",
    "include",
    "keyword",
    "label",
    "markup.bold",
    "markup.heading",
    "markup.italic",
    "markup.link",
    "markup.raw",
    "method",
    "module",
    "namespace",
    "number",
    "operator",
    "parameter",
    "preproc",
    "property",
    "punctuation",
    "punctuation.special",
    "repeat",
    "string",
    "string.escape",
    "string.special",
    "tag",
    "text.emphasis",
    "text.literal",
    "text.reference",
    "text.strong",
    "text.title",
    "text.uri",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
    "addition",
    "deletion",
    "diff.plus",
    "diff.minus",
];

struct Lang {
    name: &'static str,
    /// Lower-case extensions (without the dot) and whole file names.
    exts: &'static [&'static str],
    files: &'static [&'static str],
    /// Aliases used by injections (Markdown fence info strings etc.).
    aliases: &'static [&'static str],
    language: fn() -> tree_sitter::Language,
    highlights: fn() -> String,
    injections: &'static str,
    locals: &'static str,
}

macro_rules! lang {
    ($name:expr, $exts:expr, $files:expr, $aliases:expr, $lang:expr, [$($q:expr),+], $inj:expr, $loc:expr) => {
        Lang {
            name: $name,
            exts: $exts,
            files: $files,
            aliases: $aliases,
            language: || $lang.into(),
            highlights: || [$($q),+].join("\n"),
            injections: $inj,
            locals: $loc,
        }
    };
}

fn langs() -> &'static [Lang] {
    static L: OnceLock<Vec<Lang>> = OnceLock::new();
    L.get_or_init(|| {
        vec![
            lang!("Rust", &["rs"], &[], &["rust"], tree_sitter_rust::LANGUAGE, [tree_sitter_rust::HIGHLIGHTS_QUERY], tree_sitter_rust::INJECTIONS_QUERY, ""),
            lang!("Python", &["py", "pyw", "pyi"], &[], &["python", "py"], tree_sitter_python::LANGUAGE, [tree_sitter_python::HIGHLIGHTS_QUERY], "", ""),
            lang!(
                "JavaScript",
                &["js", "mjs", "cjs", "jsx"],
                &[],
                &["javascript", "js", "jsx"],
                tree_sitter_javascript::LANGUAGE,
                [tree_sitter_javascript::JSX_HIGHLIGHT_QUERY, tree_sitter_javascript::HIGHLIGHT_QUERY],
                tree_sitter_javascript::INJECTIONS_QUERY,
                tree_sitter_javascript::LOCALS_QUERY
            ),
            lang!(
                "TypeScript",
                &["ts", "mts", "cts"],
                &[],
                &["typescript", "ts"],
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
                [tree_sitter_typescript::HIGHLIGHTS_QUERY, tree_sitter_javascript::HIGHLIGHT_QUERY],
                "",
                tree_sitter_typescript::LOCALS_QUERY
            ),
            lang!(
                "TSX",
                &["tsx"],
                &[],
                &["tsx"],
                tree_sitter_typescript::LANGUAGE_TSX,
                [tree_sitter_typescript::HIGHLIGHTS_QUERY, tree_sitter_javascript::JSX_HIGHLIGHT_QUERY, tree_sitter_javascript::HIGHLIGHT_QUERY],
                "",
                tree_sitter_typescript::LOCALS_QUERY
            ),
            lang!("JSON", &["json", "jsonc", "json5", "geojson", "webmanifest"], &[".prettierrc", ".eslintrc", "flake.lock"], &["json"], tree_sitter_json::LANGUAGE, [tree_sitter_json::HIGHLIGHTS_QUERY], "", ""),
            lang!("TOML", &["toml"], &["Cargo.lock", "uv.lock"], &["toml"], tree_sitter_toml_ng::LANGUAGE, [tree_sitter_toml_ng::HIGHLIGHTS_QUERY], "", ""),
            lang!(
                "Bash",
                &["sh", "bash", "zsh", "ksh", "ebuild", "install"],
                &["PKGBUILD", ".bashrc", ".bash_profile", ".zshrc", ".profile", "makepkg.conf", ".xinitrc"],
                &["bash", "sh", "shell", "zsh", "console"],
                tree_sitter_bash::LANGUAGE,
                [tree_sitter_bash::HIGHLIGHT_QUERY],
                "",
                ""
            ),
            lang!("C", &["c", "h"], &[], &["c"], tree_sitter_c::LANGUAGE, [tree_sitter_c::HIGHLIGHT_QUERY], "", ""),
            lang!(
                "C++",
                &["cpp", "cc", "cxx", "hpp", "hh", "hxx", "ipp", "inl"],
                &[],
                &["cpp", "c++"],
                tree_sitter_cpp::LANGUAGE,
                [tree_sitter_cpp::HIGHLIGHT_QUERY, tree_sitter_c::HIGHLIGHT_QUERY],
                "",
                ""
            ),
            lang!("C#", &["cs", "csx"], &[], &["csharp", "cs", "c#"], tree_sitter_c_sharp::LANGUAGE, [tree_sitter_c_sharp::HIGHLIGHTS_QUERY], "", ""),
            lang!("Java", &["java"], &[], &["java"], tree_sitter_java::LANGUAGE, [tree_sitter_java::HIGHLIGHTS_QUERY], "", ""),
            lang!("Go", &["go"], &[], &["go", "golang"], tree_sitter_go::LANGUAGE, [tree_sitter_go::HIGHLIGHTS_QUERY], "", ""),
            lang!("Ruby", &["rb", "rake", "gemspec"], &["Gemfile", "Rakefile"], &["ruby", "rb"], tree_sitter_ruby::LANGUAGE, [tree_sitter_ruby::HIGHLIGHTS_QUERY], "", tree_sitter_ruby::LOCALS_QUERY),
            lang!("Lua", &["lua"], &[], &["lua"], tree_sitter_lua::LANGUAGE, [tree_sitter_lua::HIGHLIGHTS_QUERY], tree_sitter_lua::INJECTIONS_QUERY, ""),
            lang!("Zig", &["zig", "zon"], &[], &["zig"], tree_sitter_zig::LANGUAGE, [tree_sitter_zig::HIGHLIGHTS_QUERY], "", ""),
            lang!("Nix", &["nix"], &[], &["nix"], tree_sitter_nix::LANGUAGE, [tree_sitter_nix::HIGHLIGHTS_QUERY], "", ""),
            lang!("HTML", &["html", "htm", "xhtml"], &[], &["html"], tree_sitter_html::LANGUAGE, [tree_sitter_html::HIGHLIGHTS_QUERY], tree_sitter_html::INJECTIONS_QUERY, ""),
            lang!("CSS", &["css", "qss"], &[], &["css"], tree_sitter_css::LANGUAGE, [tree_sitter_css::HIGHLIGHTS_QUERY], "", ""),
            lang!(
                "XML",
                &["xml", "svg", "xsd", "xsl", "xslt", "plist", "ui", "qrc", "kml", "gpx", "csproj", "fsproj", "vcxproj", "metainfo", "policy"],
                &[],
                &["xml"],
                tree_sitter_xml::LANGUAGE_XML,
                [tree_sitter_xml::XML_HIGHLIGHT_QUERY],
                "",
                ""
            ),
            lang!("YAML", &["yaml", "yml"], &[".clang-format", ".clang-tidy"], &["yaml", "yml"], tree_sitter_yaml::LANGUAGE, [tree_sitter_yaml::HIGHLIGHTS_QUERY], "", ""),
            lang!(
                "Markdown",
                &["md", "markdown", "mdx", "mkd"],
                &[],
                &["markdown", "md"],
                tree_sitter_md::LANGUAGE,
                [tree_sitter_md::HIGHLIGHT_QUERY_BLOCK],
                tree_sitter_md::INJECTION_QUERY_BLOCK,
                ""
            ),
            lang!(
                "markdown_inline",
                &[],
                &[],
                &["markdown_inline"],
                tree_sitter_md::INLINE_LANGUAGE,
                [tree_sitter_md::HIGHLIGHT_QUERY_INLINE],
                tree_sitter_md::INJECTION_QUERY_INLINE,
                ""
            ),
            lang!(
                "INI",
                &["ini", "conf", "cfg", "desktop", "service", "timer", "socket", "mount", "target", "path", "network", "netdev", "link", "inf", "reg", "editorconfig", "theme", "directory", "hook"],
                &[".gitconfig", ".editorconfig", "mimeapps.list", "pacman.conf"],
                &["ini", "dosini", "desktop", "systemd"],
                tree_sitter_ini::LANGUAGE,
                [tree_sitter_ini::HIGHLIGHTS_QUERY],
                "",
                ""
            ),
            lang!("Makefile", &["mk", "mak"], &["Makefile", "makefile", "GNUmakefile"], &["make", "makefile"], tree_sitter_make::LANGUAGE, [tree_sitter_make::HIGHLIGHTS_QUERY], "", ""),
            lang!("CMake", &["cmake"], &["CMakeLists.txt"], &["cmake"], tree_sitter_cmake::LANGUAGE, [tree_sitter_cmake::HIGHLIGHTS_QUERY], "", ""),
            lang!("SQL", &["sql"], &[], &["sql"], tree_sitter_sequel::LANGUAGE, [tree_sitter_sequel::HIGHLIGHTS_QUERY], "", ""),
            lang!("QML", &["qml", "qmltypes"], &[], &["qml", "qmljs"], tree_sitter_qmljs::LANGUAGE, [tree_sitter_qmljs::HIGHLIGHTS_QUERY], "", ""),
            lang!("GLSL", &["glsl", "vert", "frag", "geom", "comp", "tesc", "tese", "vs", "fs"], &[], &["glsl"], tree_sitter_glsl::LANGUAGE_GLSL, [tree_sitter_glsl::HIGHLIGHTS_QUERY], "", ""),
            lang!("Diff", &["diff", "patch"], &[], &["diff", "patch"], tree_sitter_diff::LANGUAGE, [tree_sitter_diff::HIGHLIGHTS_QUERY], "", ""),
        ]
    })
}

/// Language for a file name, by extension or whole name.
pub fn detect(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e);
    langs()
        .iter()
        .find(|l| l.files.iter().any(|f| *f == name) || ext.is_some_and(|e| l.exts.contains(&e)))
        .map(|l| l.name)
}

/// Language from a `#!` line (scripts without an extension).
pub fn detect_shebang(first_line: &str) -> Option<&'static str> {
    let mut words = first_line.strip_prefix("#!")?.split_whitespace();
    let first = words.next()?;
    let prog = if first.ends_with("/env") { words.find(|w| !w.starts_with('-'))? } else { first };
    let base = prog.rsplit('/').next().unwrap_or(prog);
    Some(match base {
        b if b.starts_with("python") => "Python",
        "bash" | "sh" | "zsh" | "dash" | "ksh" => "Bash",
        "node" | "deno" | "bun" => "JavaScript",
        b if b.starts_with("ruby") => "Ruby",
        b if b.starts_with("lua") => "Lua",
        _ => return None,
    })
}

/// Language for a Markdown fence info string (`rust`, `py`, `sh`…).
pub fn by_alias(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return None;
    }
    find(name).map(|l| l.name).or_else(|| detect(&format!("x.{name}")))
}

fn find(name: &str) -> Option<&'static Lang> {
    let n = name.to_lowercase();
    langs().iter().find(|l| l.name.eq_ignore_ascii_case(&n) || l.aliases.contains(&n.as_str()))
}

/// Compiled configuration of a language (built once, kept forever).
fn config(name: &str) -> Option<&'static HighlightConfiguration> {
    static C: OnceLock<Mutex<HashMap<&'static str, Option<&'static HighlightConfiguration>>>> = OnceLock::new();
    let lang = find(name)?;
    let mut map = C.get_or_init(Default::default).lock().unwrap();
    *map.entry(lang.name).or_insert_with(|| {
        let q = (lang.highlights)();
        // Markdown's `inline` nodes have children; without this the inline
        // grammar would get nothing to parse.
        let inj = lang.injections.replace(
            "(#set! injection.language \"markdown_inline\"))",
            "(#set! injection.language \"markdown_inline\")\n  (#set! injection.include-children))",
        );
        match HighlightConfiguration::new((lang.language)(), lang.name, &q, &inj, lang.locals) {
            Ok(mut c) => {
                c.configure(NAMES);
                Some(&*Box::leak(Box::new(c)))
            }
            Err(e) => {
                eprintln!("polka: {} highlighting unavailable: {e}", lang.name);
                None
            }
        }
    })
}

/// A coloured byte range of the source; `class` indexes [`NAMES`].
#[derive(Clone, Copy, Debug)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub class: u8,
}

/// Highlight `src` as `lang`. Spans don't overlap and come in order; the
/// innermost capture wins.
/// Setting `cancel` to non-zero stops it early (returns None).
pub fn highlight(lang: &str, src: &str, cancel: &std::sync::atomic::AtomicUsize) -> Option<Vec<Span>> {
    let cfg = config(lang)?;
    let mut hl = Highlighter::new();
    let events = hl
        .highlight(cfg, src.as_bytes(), Some(cancel), |inj| config(inj))
        .ok()?;
    let mut stack: Vec<u8> = Vec::new();
    let mut out = Vec::new();
    for ev in events {
        match ev.ok()? {
            HighlightEvent::HighlightStart(h) => stack.push(h.0 as u8),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                if let Some(&class) = stack.last() {
                    if start < end {
                        out.push(Span { start, end, class });
                    }
                }
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every bundled query must compile against its grammar.
    #[test]
    fn all_languages_load() {
        let mut broken = Vec::new();
        for l in langs() {
            if config(l.name).is_none() {
                broken.push(l.name);
            }
        }
        assert!(broken.is_empty(), "broken: {broken:?}");
        assert_eq!(detect("main.rs"), Some("Rust"));
        assert_eq!(detect("PKGBUILD"), Some("Bash"));
        assert_eq!(detect("app.TSX"), Some("TSX"));
        assert_eq!(detect("notes.txt"), None);
        assert_eq!(detect_shebang("#!/usr/bin/env python3"), Some("Python"));
        assert_eq!(detect_shebang("#!/bin/bash -e"), Some("Bash"));
    }

    #[test]
    fn spans_and_injections() {
        let no = std::sync::atomic::AtomicUsize::new(0);
        let src = "fn main() { let s = \"hi\"; } // done";
        let spans = highlight("Rust", src, &no).unwrap();
        let class_of = |text: &str| {
            let at = src.find(text).unwrap();
            spans.iter().find(|s| s.start <= at && at < s.end).map(|s| NAMES[s.class as usize])
        };
        assert_eq!(class_of("fn"), Some("keyword"));
        assert_eq!(class_of("\"hi\""), Some("string"));
        assert_eq!(class_of("// done"), Some("comment"));
        // Rust inside a Markdown fence.
        let md = "# Title\n\n```rust\nlet x = 1;\n```\n";
        let spans = highlight("Markdown", md, &no).unwrap();
        let at = md.find("let").unwrap();
        assert!(spans.iter().any(|s| s.start <= at && at < s.end && NAMES[s.class as usize] == "keyword"), "{spans:?}");
        // Inline markup inside paragraphs.
        let md = "Some **bold** and `code`.\n";
        let spans = highlight("Markdown", md, &no).unwrap();
        let class_of = |text: &str| {
            let at = md.find(text).unwrap();
            spans.iter().find(|s| s.start <= at && at < s.end).map(|s| NAMES[s.class as usize])
        };
        assert_eq!(class_of("bold"), Some("text.strong"));
        assert_eq!(class_of("code"), Some("text.literal"));
    }
}
