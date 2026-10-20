//! Tiny i18n: English source strings, translated through a table per
//! language. The system's language by default, English if it isn't here;
//! switchable at run time from the settings.

mod de;
mod es;
mod fr;
mod hi;
mod ja;
mod kk;
mod ko;
mod pl;
mod ru;
mod uk;
mod uz;
mod zh;

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

/// (code, own name, table).
pub const LANGUAGES: &[(&str, &str, &[(&str, &str)])] = &[
    ("en", "English", &[]),
    ("ru", "Русский", ru::T),
    ("uk", "Українська", uk::T),
    ("de", "Deutsch", de::T),
    ("es", "Español", es::T),
    ("fr", "Français", fr::T),
    ("pl", "Polski", pl::T),
    ("kk", "Қазақша", kk::T),
    ("uz", "Oʻzbekcha", uz::T),
    ("hi", "हिन्दी", hi::T),
    ("zh", "中文", zh::T),
    ("ja", "日本語", ja::T),
    ("ko", "한국어", ko::T),
];

/// Index into [`LANGUAGES`] in use.
static CURRENT: AtomicUsize = AtomicUsize::new(0);

fn tables() -> &'static Vec<HashMap<&'static str, &'static str>> {
    static T: OnceLock<Vec<HashMap<&'static str, &'static str>>> = OnceLock::new();
    T.get_or_init(|| LANGUAGES.iter().map(|(_, _, t)| t.iter().copied().collect()).collect())
}

/// The language the usual POSIX variables name; English if none of ours.
pub fn detect() -> usize {
    for var in ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"] {
        let Ok(v) = std::env::var(var) else { continue };
        for item in v.split(':') {
            let code = item.split(['.', '@', '_', '-']).next().unwrap_or("").to_lowercase();
            if code.is_empty() || code == "c" || code == "posix" {
                continue;
            }
            return LANGUAGES.iter().position(|(c, _, _)| *c == code).unwrap_or(0);
        }
    }
    0
}

/// Use the language with this code; None (or one we don't have) follows
/// the system.
pub fn set(code: Option<&str>) {
    let i = code.and_then(|c| LANGUAGES.iter().position(|(l, _, _)| *l == c)).unwrap_or_else(detect);
    CURRENT.store(i, Ordering::Relaxed);
}

/// Code of the language in use.
pub fn current() -> &'static str {
    LANGUAGES[CURRENT.load(Ordering::Relaxed)].0
}

pub fn tr(s: &'static str) -> &'static str {
    tables()[CURRENT.load(Ordering::Relaxed)].get(s).copied().unwrap_or(s)
}

/// `tr` with `%n` replaced by a number.
pub fn trn(s: &'static str, n: usize) -> String {
    tr(s).replace("%n", &n.to_string())
}

/// "4 volumes" with the language's plural form.
pub fn volumes(n: usize) -> String {
    let slavic = |one: &str, few: &str, many: &str| {
        let word = match (n % 10, n % 100) {
            (1, x) if x != 11 => one,
            (2..=4, x) if !(12..=14).contains(&x) => few,
            _ => many,
        };
        format!("{n} {word}")
    };
    match current() {
        "ru" => slavic("том", "тома", "томов"),
        "uk" => slavic("том", "томи", "томів"),
        // Polish: 1 wolumin, 2–4 woluminy, 5–21 woluminów, 22 woluminy…
        "pl" => {
            let word = match (n, n % 10, n % 100) {
                (1, _, _) => "wolumin",
                (_, 2..=4, x) if !(12..=14).contains(&x) => "woluminy",
                _ => "woluminów",
            };
            format!("{n} {word}")
        }
        _ if n == 1 => tr("1 volume").to_string(),
        _ => trn("%n volumes", n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every table only translates strings the code really uses, once.
    #[test]
    fn tables_match_the_source() {
        let mut used = std::collections::HashSet::new();
        fn walk(dir: &std::path::Path, used: &mut std::collections::HashSet<String>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    if !p.ends_with("i18n") {
                        walk(&p, used);
                    }
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let s = std::fs::read_to_string(&p).unwrap();
                    for (i, _) in s.match_indices("tr(\"").chain(s.match_indices("trn(\"")) {
                        let rest = &s[i..];
                        let start = rest.find('"').unwrap() + 1;
                        let mut end = start;
                        let b = rest.as_bytes();
                        while b[end] != b'"' || b[end - 1] == b'\\' {
                            end += 1;
                        }
                        used.insert(rest[start..end].replace("\\\"", "\"").replace("\\\\", "\\"));
                    }
                }
            }
        }
        walk(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")), &mut used);
        for (code, _, t) in &LANGUAGES[1..] {
            let mut seen = std::collections::HashSet::new();
            for (k, _) in t.iter() {
                assert!(seen.insert(*k), "{code}: {k:?} twice");
            }
            let missing: Vec<_> = used.iter().filter(|u| !seen.contains(u.as_str())).collect();
            assert!(missing.is_empty(), "{code}: untranslated {missing:?}");
        }
    }
}
