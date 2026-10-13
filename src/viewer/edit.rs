//! Editing and searching text in the viewer.

use std::ops::Range;
use std::time::Instant;

use super::TextState;
use super::text::{Row, TAB, columns};

/// One undoable change: `removed` at `at` was replaced by `inserted`.
#[derive(Clone, Debug)]
struct Change {
    at: usize,
    removed: String,
    inserted: String,
    caret_before: usize,
    caret_after: usize,
    when: Instant,
}

/// Caret, selection, history.
#[derive(Default)]
pub struct Editor {
    pub caret: usize,
    /// Other end of the selection.
    pub anchor: Option<usize>,
    /// Column to aim for when moving up/down through shorter lines.
    goal_col: Option<usize>,
    undo: Vec<Change>,
    redo: Vec<Change>,
    /// Hash of the text as last saved.
    saved: u64,
    pub dirty: bool,
    /// Gamepad: the D-pad moves the caret instead of scrolling.
    pub editing: bool,
    /// Scroll the caret into view on the next frame.
    pub reveal: bool,
    pub find: Option<Find>,
}

#[derive(Default, Clone)]
pub struct Find {
    pub query: String,
    pub case: bool,
    pub regex: bool,
    pub word: bool,
    pub matches: Vec<Range<usize>>,
    pub current: Option<usize>,
    pub error: Option<String>,
    /// Typing goes into the query (keyboard).
    pub typing: bool,
    /// What `matches` was computed for: (query, flags, doc generation).
    computed: Option<(String, [bool; 3], u64)>,
}

pub const MAX_MATCHES: usize = 100_000;

fn hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn prev_char(s: &str, at: usize) -> usize {
    s[..at].char_indices().next_back().map_or(0, |(i, _)| i)
}

fn next_char(s: &str, at: usize) -> usize {
    s[at..].chars().next().map_or(at, |c| at + c.len_utf8())
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Editor {
    pub fn new(text: &str) -> Self {
        Editor { saved: hash(text), ..Default::default() }
    }

    pub fn selection(&self) -> Option<Range<usize>> {
        let a = self.anchor?;
        (a != self.caret).then(|| a.min(self.caret)..a.max(self.caret))
    }

    pub fn mark_saved(&mut self, text: &str) {
        self.saved = hash(text);
        self.dirty = false;
    }
}

impl Find {
    /// Recompute matches if the query, flags or text changed.
    pub fn update(&mut self, text: &str, generation: u64) {
        let key = (self.query.clone(), [self.case, self.regex, self.word], generation);
        if self.computed.as_ref() == Some(&key) {
            return;
        }
        self.computed = Some(key);
        self.matches.clear();
        self.error = None;
        if self.query.is_empty() {
            self.current = None;
            return;
        }
        let mut pat = if self.regex { self.query.clone() } else { regex::escape(&self.query) };
        if self.word {
            pat = format!(r"\b(?:{pat})\b");
        }
        match regex::RegexBuilder::new(&pat).case_insensitive(!self.case).multi_line(true).size_limit(1 << 24).build() {
            Ok(re) => {
                for m in re.find_iter(text).filter(|m| !m.is_empty()).take(MAX_MATCHES) {
                    self.matches.push(m.range());
                }
            }
            Err(e) => self.error = Some(e.to_string().lines().last().unwrap_or("").trim().to_string()),
        }
        if self.matches.is_empty() {
            self.current = None;
        } else if self.current.is_none_or(|c| c >= self.matches.len()) {
            self.current = Some(0);
        }
    }

    /// First match at or after `from` (wrapping).
    pub fn nearest(&self, from: usize) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        Some(self.matches.partition_point(|m| m.start < from) % self.matches.len())
    }
}

impl TextState {
    pub fn editable(&self) -> bool {
        !self.doc.lossy && !self.doc.truncated && !self.read_only
    }

    /// Visual row holding byte `at`.
    pub fn row_of(&self, at: usize) -> usize {
        let line = self.doc.line_of(at) as u32;
        let in_line = (at - self.doc.lines[line as usize].0) as u32;
        let first = self.rows.partition_point(|r| r.line < line);
        let mut i = first;
        while i + 1 < self.rows.len() && self.rows[i + 1].line == line && self.rows[i + 1].start <= in_line {
            i += 1;
        }
        i.min(self.rows.len().saturating_sub(1))
    }

    /// Column of byte `at` within its visual row.
    pub fn col_of(&self, at: usize) -> usize {
        let r = self.rows[self.row_of(at)];
        let line = self.doc.line(r.line as usize);
        let in_line = at - self.doc.lines[r.line as usize].0;
        columns(&line[r.start as usize..in_line.max(r.start as usize)])
    }

    /// Byte offset at `col` in visual row `row`.
    pub fn at_col(&self, row: usize, col: usize) -> usize {
        let r: Row = self.rows[row.min(self.rows.len() - 1)];
        let line = self.doc.line(r.line as usize);
        let base = self.doc.lines[r.line as usize].0;
        let seg = &line[r.start as usize..r.end as usize];
        let mut c = 0;
        for (i, ch) in seg.char_indices() {
            let w = if ch == '\t' { TAB } else { 1 };
            // Nearer the glyph's left edge: before it; else after it.
            if col < c + w.div_ceil(2) {
                return base + r.start as usize + i;
            }
            if col < c + w {
                return base + r.start as usize + i + ch.len_utf8();
            }
            c += w;
        }
        // Past the end: the end of the row (before the wrap point's space).
        let wrapped = (r.end as usize) < line.len();
        let end = r.end as usize;
        base + if wrapped { prev_char(line, end).max(r.start as usize) } else { end }
    }

    fn set_caret(&mut self, at: usize, select: bool) {
        if select {
            self.ed.anchor.get_or_insert(self.ed.caret);
        } else {
            self.ed.anchor = None;
        }
        self.ed.caret = at.min(self.doc.text.len());
        self.ed.reveal = true;
    }

    /// Caret movement; `select` extends the selection.
    pub fn move_caret(&mut self, m: Move, select: bool) {
        let text = &self.doc.text;
        let c = self.ed.caret;
        // Collapsing a selection with ←/→ goes to its edge.
        if !select {
            if let Some(sel) = self.ed.selection() {
                match m {
                    Move::Left => return self.set_caret(sel.start, false),
                    Move::Right => return self.set_caret(sel.end, false),
                    _ => {}
                }
            }
        }
        let vertical = matches!(m, Move::Up | Move::Down | Move::PageUp | Move::PageDown);
        if !vertical {
            self.ed.goal_col = None;
        }
        let line = self.doc.line_of(c);
        let (ls, le) = self.doc.lines[line];
        let to = match m {
            Move::Left => {
                if c == ls && line > 0 { self.doc.lines[line - 1].1 } else { prev_char(text, c) }
            }
            Move::Right => {
                if c == le && line + 1 < self.doc.lines.len() { self.doc.lines[line + 1].0 } else { next_char(text, c).min(le.max(c)) }
            }
            Move::WordLeft => {
                let mut i = c;
                while i > 0 && !is_word(text[..i].chars().next_back().unwrap()) {
                    i = prev_char(text, i);
                }
                while i > 0 && is_word(text[..i].chars().next_back().unwrap()) {
                    i = prev_char(text, i);
                }
                i
            }
            Move::WordRight => {
                let mut i = c;
                while i < text.len() && !is_word(text[i..].chars().next().unwrap()) {
                    i = next_char(text, i);
                }
                while i < text.len() && is_word(text[i..].chars().next().unwrap()) {
                    i = next_char(text, i);
                }
                i
            }
            Move::Home => {
                // First press: after the indentation; second: column 0.
                let indent = ls + text[ls..le].len() - text[ls..le].trim_start().len();
                if c == indent { ls } else { indent }
            }
            Move::End => le,
            Move::DocStart => 0,
            Move::DocEnd => text.len(),
            Move::Up | Move::Down | Move::PageUp | Move::PageDown => {
                let row = self.row_of(c);
                let goal = match self.ed.goal_col {
                    Some(g) => g,
                    None => self.col_of(c),
                };
                self.ed.goal_col = Some(goal);
                let page = self.page_rows.max(1);
                let target = match m {
                    Move::Up => row.checked_sub(1),
                    Move::Down => (row + 1 < self.rows.len()).then_some(row + 1),
                    Move::PageUp => Some(row.saturating_sub(page)),
                    _ => Some((row + page).min(self.rows.len() - 1)),
                };
                match target {
                    Some(r) => self.at_col(r, goal),
                    None if matches!(m, Move::Up) => 0,
                    None => text.len(),
                }
            }
        };
        let goal = self.ed.goal_col;
        self.set_caret(to, select);
        self.ed.goal_col = goal;
    }

    /// Replace the selection (or insert at the caret).
    pub fn insert(&mut self, s: &str) -> bool {
        if !self.editable() {
            return false;
        }
        // Typed line breaks follow the file's style.
        let s = if self.doc.crlf { s.replace("\r\n", "\n").replace('\n', "\r\n") } else { s.replace("\r\n", "\n") };
        let range = self.ed.selection().unwrap_or(self.ed.caret..self.ed.caret);
        self.apply(range, &s, true);
        true
    }

    /// Enter with the current line's indentation carried over.
    pub fn newline(&mut self) -> bool {
        let (ls, le) = self.doc.lines[self.doc.line_of(self.ed.caret)];
        let line = &self.doc.text[ls..le];
        let indent: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let indent = &indent[..indent.len().min(self.ed.caret - ls)];
        self.insert(&format!("\n{indent}"))
    }

    /// Indent with a tab, or spaces when the file indents with spaces.
    pub fn tab(&mut self) -> bool {
        let spaces = self.doc.lines.iter().take(500).filter(|&&(a, b)| self.doc.text[a..b].starts_with("  ")).count();
        let tabs = self.doc.lines.iter().take(500).filter(|&&(a, b)| self.doc.text[a..b].starts_with('\t')).count();
        self.insert(if spaces > tabs { "    " } else { "\t" })
    }

    pub fn backspace(&mut self, word: bool) -> bool {
        if !self.editable() {
            return false;
        }
        let range = match self.ed.selection() {
            Some(r) => r,
            None => {
                let end = self.ed.caret;
                let line = self.doc.line_of(end);
                let start = if end == self.doc.lines[line].0 && line > 0 {
                    self.doc.lines[line - 1].1
                } else if word {
                    self.ed.anchor = None;
                    self.move_caret(Move::WordLeft, false);
                    let s = self.ed.caret;
                    self.ed.caret = end;
                    s
                } else {
                    prev_char(&self.doc.text, end)
                };
                start..end
            }
        };
        if range.is_empty() {
            return false;
        }
        self.apply(range, "", true);
        true
    }

    pub fn delete(&mut self, word: bool) -> bool {
        if !self.editable() {
            return false;
        }
        let range = match self.ed.selection() {
            Some(r) => r,
            None => {
                let start = self.ed.caret;
                let line = self.doc.line_of(start);
                let end = if start == self.doc.lines[line].1 && line + 1 < self.doc.lines.len() {
                    self.doc.lines[line + 1].0
                } else if word {
                    self.move_caret(Move::WordRight, false);
                    let e = self.ed.caret;
                    self.ed.caret = start;
                    e
                } else {
                    next_char(&self.doc.text, start)
                };
                start..end
            }
        };
        if range.is_empty() {
            return false;
        }
        self.apply(range, "", true);
        true
    }

    fn apply(&mut self, range: Range<usize>, with: &str, record: bool) {
        let removed = self.doc.text[range.clone()].to_string();
        let caret_before = self.ed.caret;
        self.doc.replace(range.clone(), with);
        let caret_after = range.start + with.len();
        if record {
            let now = Instant::now();
            // Typing runs merge into one undo step.
            let merged = match self.ed.undo.last_mut() {
                Some(last)
                    if removed.is_empty()
                        && last.removed.is_empty()
                        && !with.contains('\n')
                        && !last.inserted.contains('\n')
                        && last.at + last.inserted.len() == range.start
                        && now.duration_since(last.when).as_millis() < 1000 =>
                {
                    last.inserted.push_str(with);
                    last.caret_after = caret_after;
                    last.when = now;
                    true
                }
                _ => false,
            };
            if !merged {
                self.ed.undo.push(Change { at: range.start, removed, inserted: with.to_string(), caret_before, caret_after, when: now });
                if self.ed.undo.len() > 2000 {
                    self.ed.undo.remove(0);
                }
            }
            self.ed.redo.clear();
        }
        self.ed.anchor = None;
        self.ed.caret = caret_after;
        self.ed.goal_col = None;
        self.ed.reveal = true;
        self.ed.dirty = hash(&self.doc.text) != self.ed.saved;
        self.rows_for = None;
    }

    pub fn undo(&mut self) -> bool {
        let Some(c) = self.ed.undo.pop() else { return false };
        self.doc.replace(c.at..c.at + c.inserted.len(), &c.removed);
        self.after_history(c.caret_before);
        self.ed.redo.push(c);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(c) = self.ed.redo.pop() else { return false };
        self.doc.replace(c.at..c.at + c.removed.len(), &c.inserted);
        self.after_history(c.caret_after);
        self.ed.undo.push(c);
        true
    }

    fn after_history(&mut self, caret: usize) {
        self.ed.anchor = None;
        self.ed.caret = caret.min(self.doc.text.len());
        self.ed.reveal = true;
        self.ed.dirty = hash(&self.doc.text) != self.ed.saved;
        self.rows_for = None;
    }

    pub fn select_all(&mut self) {
        self.ed.anchor = Some(0);
        self.ed.caret = self.doc.text.len();
    }

    pub fn selected_text(&self) -> Option<String> {
        self.ed.selection().map(|r| self.doc.text[r].to_string())
    }

    /// The word around byte `at` (for double clicks).
    pub fn word_at(&self, at: usize) -> Range<usize> {
        let t = &self.doc.text;
        let mut a = at;
        while a > 0 && is_word(t[..a].chars().next_back().unwrap()) {
            a = prev_char(t, a);
        }
        let mut b = at;
        while b < t.len() && is_word(t[b..].chars().next().unwrap()) {
            b = next_char(t, b);
        }
        a..b
    }

    /// Write the text back to `path` (atomically, keeping permissions).
    pub fn save_to(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        let tmp = path.with_file_name(format!(".{}.polka-save", path.file_name().unwrap_or_default().to_string_lossy()));
        std::fs::write(&tmp, self.doc.text.as_bytes())?;
        if let Ok(m) = std::fs::metadata(path) {
            let _ = std::fs::set_permissions(&tmp, m.permissions());
        }
        std::fs::rename(&tmp, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
        self.ed.mark_saved(&self.doc.text);
        Ok(())
    }

    /// Jump to match `i` of the search, selecting it.
    pub fn goto_match(&mut self, i: usize) {
        let Some(f) = self.ed.find.as_mut() else { return };
        let Some(m) = f.matches.get(i).cloned() else { return };
        f.current = Some(i);
        self.ed.anchor = Some(m.start);
        self.ed.caret = m.end;
        self.ed.reveal = true;
    }

    /// Next (`d` = 1) or previous (-1) match from the caret.
    pub fn find_step(&mut self, d: isize) {
        let Some(f) = self.ed.find.as_ref() else { return };
        let n = f.matches.len();
        if n == 0 {
            return;
        }
        let i = match f.current {
            // Still on the current match: step from it.
            Some(c) if self.ed.selection() == f.matches.get(c).cloned() => (c as isize + d).rem_euclid(n as isize) as usize,
            _ => {
                let from = if d > 0 { self.ed.caret } else { self.ed.selection().map_or(self.ed.caret, |s| s.start) };
                let i = f.nearest(from).unwrap_or(0);
                if d > 0 { i } else { (i as isize - 1).rem_euclid(n as isize) as usize }
            }
        };
        self.goto_match(i);
    }

    pub fn find_refresh(&mut self) {
        let generation = self.doc.generation;
        if let Some(f) = self.ed.find.as_mut() {
            f.update(&self.doc.text, generation);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    Home,
    End,
    PageUp,
    PageDown,
    DocStart,
    DocEnd,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viewer::{Content, Viewer};

    fn open(text: &str, name: &str) -> (std::path::PathBuf, Box<TextState>) {
        let dir = std::env::temp_dir().join(format!("polka-edit-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        let v = Viewer::open(p.clone(), name.into(), 0, Vec::new(), 1.0, &egui::Context::default());
        let Content::Text(mut t) = v.content else { panic!() };
        t.rows = crate::viewer::text::rows(&t.doc, None);
        t.rows_for = Some(None);
        (p, t)
    }

    fn sync(t: &mut TextState) {
        t.rows = crate::viewer::text::rows(&t.doc, None);
        t.rows_for = Some(None);
    }

    #[test]
    fn typing_moving_undo_save() {
        let (p, mut t) = open("    fn a() {}\nlet b = 1;\n", "x.rs");
        t.move_caret(Move::End, false);
        assert!(t.newline());
        sync(&mut t);
        assert!(t.insert("x"));
        assert!(t.insert("y"));
        assert_eq!(t.doc.line(1), "    xy", "indent carried over");
        assert!(t.ed.dirty);
        // Typing merges into one undo step.
        t.undo();
        sync(&mut t);
        assert_eq!(t.doc.line(1), "    ");
        t.redo();
        sync(&mut t);
        assert_eq!(t.doc.line(1), "    xy");
        // Word moves and selection.
        t.ed.caret = 0;
        t.move_caret(Move::WordRight, true);
        assert_eq!(t.selected_text().as_deref(), Some("    fn"));
        t.move_caret(Move::Down, false);
        assert_eq!(t.doc.line_of(t.ed.caret), 1);
        t.move_caret(Move::Down, false);
        t.move_caret(Move::Up, false);
        t.move_caret(Move::Home, false);
        assert_eq!(t.ed.caret, t.doc.lines[1].0 + 4, "home goes past the indent first");
        t.move_caret(Move::Home, false);
        assert_eq!(t.ed.caret, t.doc.lines[1].0);
        t.backspace(false);
        sync(&mut t);
        assert!(t.doc.text.starts_with("    fn a() {}    xy"));
        t.save_to(&p).unwrap();
        assert!(!t.ed.dirty);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), t.doc.text);
        std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }

    #[test]
    fn crlf_is_kept() {
        let (p, mut t) = open("a\r\nb\r\n", "w.txt");
        t.move_caret(Move::End, false);
        t.newline();
        assert_eq!(t.doc.text, "a\r\n\r\nb\r\n");
        std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }

    #[test]
    fn search_words_and_regex() {
        let (p, mut t) = open("Foo foo food\nbar foo\n", "s.txt");
        t.ed.find = Some(Find { query: "foo".into(), ..Default::default() });
        t.find_refresh();
        assert_eq!(t.ed.find.as_ref().unwrap().matches.len(), 4);
        t.ed.find.as_mut().unwrap().case = true;
        t.find_refresh();
        assert_eq!(t.ed.find.as_ref().unwrap().matches.len(), 3);
        t.ed.find.as_mut().unwrap().word = true;
        t.find_refresh();
        assert_eq!(t.ed.find.as_ref().unwrap().matches.len(), 2);
        let f = t.ed.find.as_mut().unwrap();
        f.word = false;
        f.regex = true;
        f.query = r"fo+d?\b".into();
        t.find_refresh();
        assert_eq!(t.ed.find.as_ref().unwrap().matches.len(), 3);
        t.ed.caret = 0;
        t.find_step(1);
        assert_eq!(t.selected_text().as_deref(), Some("foo"));
        t.find_step(1);
        assert_eq!(t.selected_text().as_deref(), Some("food"));
        t.find_step(-1);
        assert_eq!(t.selected_text().as_deref(), Some("foo"));
        // A bad pattern reports instead of matching.
        t.ed.find.as_mut().unwrap().query = "(".into();
        t.find_refresh();
        assert!(t.ed.find.as_ref().unwrap().error.is_some());
        // Matches follow edits.
        t.ed.find.as_mut().unwrap().query = "bar".into();
        t.ed.caret = 0;
        t.insert("bar ");
        t.find_refresh();
        assert_eq!(t.ed.find.as_ref().unwrap().matches.len(), 2);
        std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }
}
