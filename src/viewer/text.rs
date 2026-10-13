//! Text documents: loaded once, highlighted in the background, drawn as
//! fixed-height visual rows (lines, or pieces of wrapped lines) so only the
//! visible part is ever laid out.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use super::syntax::{self, Span};

/// Bigger files are shown cut off at this size.
pub const MAX_BYTES: u64 = 16 * 1024 * 1024;
/// Highlighting stops being worth it past this.
const MAX_HIGHLIGHT: usize = 4 * 1024 * 1024;
/// Tabs are drawn this wide.
pub const TAB: usize = 4;

pub struct TextDoc {
    pub text: String,
    /// Byte range of each line, without the line break.
    pub lines: Vec<(usize, usize)>,
    pub lang: Option<&'static str>,
    /// Colour spans per line (line-relative byte ranges), once ready.
    pub spans: Arc<OnceLock<Vec<Vec<Span>>>>,
    pub truncated: bool,
    /// Not valid UTF-8: shown with replacement characters.
    pub lossy: bool,
    pub crlf: bool,
    /// Longest line in display columns.
    pub widest: usize,
    cancel: Arc<AtomicUsize>,
}

impl Drop for TextDoc {
    fn drop(&mut self) {
        self.cancel.store(1, Ordering::Relaxed);
    }
}

/// Display columns of a piece of line (tabs count as [`TAB`]).
pub fn columns(s: &str) -> usize {
    s.chars().map(|c| if c == '\t' { TAB } else { 1 }).sum()
}

/// Does this look like text? (no NULs, mostly valid UTF-8 at the start)
pub fn sniff(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let mut buf = vec![0u8; 8192];
    let Ok(n) = f.read(&mut buf) else { return false };
    let buf = &buf[..n];
    if buf.contains(&0) {
        return false;
    }
    match std::str::from_utf8(buf) {
        Ok(_) => true,
        // A multi-byte char cut by the buffer end is fine.
        Err(e) => e.error_len().is_none() && e.valid_up_to() + 4 >= n,
    }
}

impl TextDoc {
    /// Read `path` (shown as `name`) and start highlighting it.
    pub fn load(path: &Path, name: &str, repaint: impl Fn() + Send + 'static) -> std::io::Result<Self> {
        use std::io::Read;
        let f = std::fs::File::open(path)?;
        let size = f.metadata()?.len();
        let mut bytes = Vec::with_capacity(size.min(MAX_BYTES) as usize);
        f.take(MAX_BYTES).read_to_end(&mut bytes)?;
        let truncated = size > MAX_BYTES;
        let (text, lossy) = match String::from_utf8(bytes) {
            Ok(s) => (s, false),
            Err(e) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), true),
        };
        let text = text.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(text);
        let mut lines = Vec::new();
        let mut start = 0;
        let mut crlf = false;
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                let end = if i > start && text.as_bytes()[i - 1] == b'\r' {
                    crlf = true;
                    i - 1
                } else {
                    i
                };
                lines.push((start, end));
                start = i + 1;
            }
        }
        if start < text.len() || lines.is_empty() {
            lines.push((start, text.len()));
        }
        let widest = lines.iter().map(|&(a, b)| columns(&text[a..b])).max().unwrap_or(0);
        let first = text.lines().next().unwrap_or("");
        let lang = syntax::detect(name).or_else(|| syntax::detect_shebang(first));
        let doc = TextDoc {
            text,
            lines,
            lang,
            spans: Arc::new(OnceLock::new()),
            truncated,
            lossy,
            crlf,
            widest,
            cancel: Arc::new(AtomicUsize::new(0)),
        };
        if let Some(lang) = lang.filter(|_| doc.text.len() <= MAX_HIGHLIGHT) {
            let (text, lines, out, cancel) = (doc.text.clone(), doc.lines.clone(), doc.spans.clone(), doc.cancel.clone());
            std::thread::Builder::new()
                .name("polka-highlight".into())
                .spawn(move || {
                    if let Some(spans) = syntax::highlight(lang, &text, &cancel) {
                        let _ = out.set(per_line(&spans, &lines));
                        repaint();
                    }
                })
                .ok();
        }
        Ok(doc)
    }

    pub fn line(&self, i: usize) -> &str {
        let (a, b) = self.lines[i];
        &self.text[a..b]
    }
}

/// Cut document spans into line-relative pieces.
fn per_line(spans: &[Span], lines: &[(usize, usize)]) -> Vec<Vec<Span>> {
    let mut out = vec![Vec::new(); lines.len()];
    let mut li = 0;
    for s in spans {
        while li < lines.len() && lines[li].1 < s.start {
            li += 1;
        }
        let mut l = li;
        while l < lines.len() && lines[l].0 < s.end {
            let (a, b) = lines[l];
            let (st, en) = (s.start.max(a), s.end.min(b));
            if st < en {
                out[l].push(Span { start: st - a, end: en - a, class: s.class });
            }
            l += 1;
        }
    }
    out
}

/// One row on screen: part of a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub line: u32,
    /// Byte range within the line.
    pub start: u32,
    pub end: u32,
}

/// Rows for the document at `cols` columns (None = no wrapping). Wrapping
/// breaks after the last space that fits, or mid-word if there is none.
pub fn rows(doc: &TextDoc, cols: Option<usize>) -> Vec<Row> {
    let mut out = Vec::with_capacity(doc.lines.len());
    for (li, _) in doc.lines.iter().enumerate() {
        let line = doc.line(li);
        let Some(cols) = cols.filter(|c| *c >= 8) else {
            out.push(Row { line: li as u32, start: 0, end: line.len() as u32 });
            continue;
        };
        let mut start = 0;
        loop {
            // Walk until the row is full.
            let mut col = 0;
            let mut end = line.len();
            let mut last_space = None;
            for (i, c) in line[start..].char_indices() {
                let w = if c == '\t' { TAB } else { 1 };
                if col + w > cols {
                    end = start + i;
                    break;
                }
                col += w;
                if c == ' ' || c == '\t' {
                    last_space = Some(start + i + c.len_utf8());
                }
            }
            if end < line.len() {
                if let Some(s) = last_space.filter(|s| *s > start) {
                    end = s;
                }
            }
            out.push(Row { line: li as u32, start: start as u32, end: end as u32 });
            if end >= line.len() {
                break;
            }
            start = end;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str, name: &str) -> TextDoc {
        let dir = std::env::temp_dir().join(format!("polka-text-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        let d = TextDoc::load(&p, name, || {}).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        d
    }

    #[test]
    fn lines_and_wrapping() {
        let d = doc("one two three four\r\n\tx\n", "a.txt");
        assert_eq!(d.lines.len(), 2);
        assert!(d.crlf);
        assert_eq!(d.line(0), "one two three four");
        assert_eq!(d.widest, 18);
        let r = rows(&d, Some(10));
        let text: Vec<&str> = r.iter().map(|r| &d.line(r.line as usize)[r.start as usize..r.end as usize]).collect();
        assert_eq!(text, ["one two ", "three four", "\tx"]);
        assert_eq!(rows(&d, None).len(), 2);
        // A long word is cut.
        let d = doc("abcdefghijklmnopqrstuvwxyz", "b.txt");
        assert_eq!(rows(&d, Some(10)).len(), 3);
    }

    #[test]
    fn highlights_by_line() {
        let d = doc("fn a() {}\n// c\n", "x.rs");
        assert_eq!(d.lang, Some("Rust"));
        for _ in 0..200 {
            if d.spans.get().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let spans = d.spans.get().expect("highlighted");
        assert_eq!(syntax::NAMES[spans[1][0].class as usize], "comment");
        assert_eq!((spans[1][0].start, spans[1][0].end), (0, 4));
    }

    #[test]
    fn sniffs_text() {
        let dir = std::env::temp_dir().join(format!("polka-sniff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("t"), "привет\n").unwrap();
        std::fs::write(dir.join("b"), [0u8, 1, 2, 3]).unwrap();
        assert!(sniff(&dir.join("t")));
        assert!(!sniff(&dir.join("b")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
