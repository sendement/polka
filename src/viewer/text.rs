//! Text documents: loaded once, highlighted in the background, drawn as
//! fixed-height visual rows (lines, or pieces of wrapped lines) so only the
//! visible part is ever laid out.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::syntax::{self, Span};

/// Bigger files are shown cut off at this size.
pub const MAX_BYTES: u64 = 16 * 1024 * 1024;
/// Highlighting stops being worth it past this.
const MAX_HIGHLIGHT: usize = 4 * 1024 * 1024;
/// Tabs are drawn this wide.
pub const TAB: usize = 4;

type Repaint = Arc<dyn Fn() + Send + Sync>;
/// Highlighting result for a document generation.
type HlSlot = Arc<Mutex<Option<(u64, Vec<Vec<Span>>)>>>;

/// Re-highlight once typing pauses this long.
const REHIGHLIGHT_AFTER: Duration = Duration::from_millis(250);

pub struct TextDoc {
    /// The file's text as is (line breaks included, `\r\n` kept).
    pub text: String,
    /// Byte range of each line, without the line break.
    pub lines: Vec<(usize, usize)>,
    pub lang: Option<&'static str>,
    /// Colour spans per line (line-relative byte ranges); empty until the
    /// first highlighting ends, approximately shifted while editing.
    pub spans: Vec<Vec<Span>>,
    /// Bumped by every edit.
    pub generation: u64,
    /// Has highlighting of some generation finished?
    pub highlighted: bool,
    hl: HlSlot,
    hl_started: Option<u64>,
    edited_at: Option<Instant>,
    repaint: Repaint,
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
    pub fn load(path: &Path, name: &str, repaint: impl Fn() + Send + Sync + 'static) -> std::io::Result<Self> {
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
        let first = text.lines().next().unwrap_or("");
        let lang = syntax::detect(name).or_else(|| syntax::detect_shebang(first));
        let mut doc = TextDoc {
            text,
            lines: Vec::new(),
            lang,
            spans: Vec::new(),
            generation: 0,
            highlighted: false,
            hl: Arc::new(Mutex::new(None)),
            hl_started: None,
            edited_at: None,
            repaint: Arc::new(repaint),
            truncated,
            lossy,
            crlf: false,
            widest: 0,
            cancel: Arc::new(AtomicUsize::new(0)),
        };
        doc.reindex();
        doc.crlf = doc.lines.iter().any(|&(_, e)| doc.text.as_bytes().get(e) == Some(&b'\r'));
        doc.spans = vec![Vec::new(); doc.lines.len()];
        doc.start_highlight();
        Ok(doc)
    }

    /// Recompute line ranges and the widest line.
    fn reindex(&mut self) {
        let text = &self.text;
        let mut lines = Vec::with_capacity(self.lines.len().max(16));
        let mut start = 0;
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                let end = if i > start && text.as_bytes()[i - 1] == b'\r' { i - 1 } else { i };
                lines.push((start, end));
                start = i + 1;
            }
        }
        lines.push((start, text.len()));
        self.widest = lines.iter().map(|&(a, b)| columns(&text[a..b])).max().unwrap_or(0);
        self.lines = lines;
    }

    fn start_highlight(&mut self) {
        let Some(lang) = self.lang.filter(|_| self.text.len() <= MAX_HIGHLIGHT) else { return };
        // A newer edit makes the running one pointless.
        self.cancel.store(1, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicUsize::new(0));
        let generation = self.generation;
        self.hl_started = Some(generation);
        let (text, lines, out, cancel, repaint) = (self.text.clone(), self.lines.clone(), self.hl.clone(), self.cancel.clone(), self.repaint.clone());
        std::thread::Builder::new()
            .name("polka-highlight".into())
            .spawn(move || {
                if let Some(spans) = syntax::highlight(lang, &text, &cancel) {
                    *out.lock().unwrap() = Some((generation, per_line(&spans, &lines)));
                    repaint();
                }
            })
            .ok();
    }

    /// Take finished highlighting; start a new one once edits settle.
    /// Returns how long to wait before calling again, if anything is pending.
    pub fn poll_highlight(&mut self) -> Option<Duration> {
        if let Some((generation, spans)) = self.hl.lock().unwrap().take() {
            if generation == self.generation {
                self.spans = spans;
                self.highlighted = true;
            }
        }
        if self.hl_started == Some(self.generation) || self.lang.is_none() {
            return None;
        }
        let since = self.edited_at.map_or(REHIGHLIGHT_AFTER, |t| t.elapsed());
        if since >= REHIGHLIGHT_AFTER {
            self.start_highlight();
            None
        } else {
            Some(REHIGHLIGHT_AFTER - since)
        }
    }

    /// Line holding byte `at`.
    pub fn line_of(&self, at: usize) -> usize {
        self.lines.partition_point(|&(s, _)| s <= at).saturating_sub(1)
    }

    /// Replace `range` with `with`. Colours of untouched text are kept
    /// (shifted) until highlighting catches up.
    pub fn replace(&mut self, range: std::ops::Range<usize>, with: &str) {
        let (l0, l1) = (self.line_of(range.start), self.line_of(range.end));
        let line_start = self.lines[l0].0;
        let old_lines = l1 - l0 + 1;
        self.text.replace_range(range.clone(), with);
        self.reindex();
        let new_l1 = self.line_of(range.start + with.len());
        let new_lines = new_l1 - l0 + 1;
        let delta = with.len() as isize - (range.end - range.start) as isize;
        if old_lines == 1 && new_lines == 1 {
            // Inside one line: shift the spans after the edit.
            let (a, b) = (range.start - line_start, range.end - line_start);
            for s in self.spans[l0].iter_mut() {
                if s.start >= b {
                    s.start = (s.start as isize + delta) as usize;
                    s.end = (s.end as isize + delta) as usize;
                } else if s.end > a {
                    s.end = (s.end as isize + delta).max(s.start as isize) as usize;
                }
            }
            self.spans[l0].retain(|s| s.end > s.start);
        } else {
            let keep_first = self.spans.get(l0).cloned().unwrap_or_default();
            let cut = range.start - line_start;
            let first: Vec<Span> = keep_first.into_iter().filter(|s| s.end <= cut).collect();
            let fill = std::iter::once(first).chain(std::iter::repeat_n(Vec::new(), new_lines - 1));
            self.spans.splice(l0..(l0 + old_lines).min(self.spans.len()), fill);
        }
        self.spans.resize(self.lines.len(), Vec::new());
        self.generation += 1;
        self.edited_at = Some(Instant::now());
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
        // The empty line after the last break is a line too (for the caret).
        assert_eq!(d.lines.len(), 3);
        assert!(d.crlf);
        assert_eq!(d.line(0), "one two three four");
        assert_eq!(d.widest, 18);
        let r = rows(&d, Some(10));
        let text: Vec<&str> = r.iter().map(|r| &d.line(r.line as usize)[r.start as usize..r.end as usize]).collect();
        assert_eq!(text, ["one two ", "three four", "\tx", ""]);
        assert_eq!(rows(&d, None).len(), 3);
        // A long word is cut.
        let d = doc("abcdefghijklmnopqrstuvwxyz", "b.txt");
        assert_eq!(rows(&d, Some(10)).len(), 3);
    }

    #[test]
    fn highlights_by_line() {
        let d = doc("fn a() {}\n// c\n", "x.rs");
        assert_eq!(d.lang, Some("Rust"));
        let mut d = d;
        for _ in 0..200 {
            d.poll_highlight();
            if d.highlighted {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(d.highlighted);
        let spans = &d.spans;
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
