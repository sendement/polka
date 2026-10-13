//! Markdown shown the way GitHub renders it: parsed with pulldown-cmark,
//! drawn with egui, code blocks highlighted by tree-sitter.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Align2, Color32, CornerRadius, FontFamily, FontId, Galley, Rect, Sense, Stroke, Ui, pos2, vec2};
use pulldown_cmark::{Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use super::syntax::{self, NAMES, Span};
use super::theme::Theme;
use crate::i18n::tr;

#[derive(Clone, Debug, Default)]
pub struct Piece {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: Option<String>,
    /// (source, alt text)
    pub image: Option<(String, String)>,
}

type Inline = Vec<Piece>;

#[derive(Clone, Debug)]
pub enum Block {
    Heading { level: u8, inline: Inline, anchor: String },
    Para(Inline),
    Code { text: String, spans: Vec<Span> },
    Quote { alert: Option<BlockQuoteKind>, blocks: Vec<Block> },
    List { start: Option<u64>, items: Vec<Item> },
    Rule,
    Table { aligns: Vec<Alignment>, head: Vec<Inline>, rows: Vec<Vec<Inline>> },
}

#[derive(Clone, Debug, Default)]
pub struct Item {
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

pub struct MdDoc {
    pub blocks: Vec<Block>,
}

/// GitHub-style anchor for a heading.
pub fn slug(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

fn plain(inline: &Inline) -> String {
    inline.iter().map(|p| p.text.as_str()).collect()
}

/// `<img src="…" alt="…">` in raw HTML (centered logos in READMEs).
fn html_images(html: &str) -> Vec<Piece> {
    let re = regex::Regex::new(r#"(?is)<img\b[^>]*?\bsrc\s*=\s*["']([^"']+)["'][^>]*>"#).unwrap();
    let alt_re = regex::Regex::new(r#"(?is)\balt\s*=\s*["']([^"']*)["']"#).unwrap();
    re.captures_iter(html)
        .map(|c| {
            let alt = alt_re.captures(&c[0]).map(|a| a[1].to_string()).unwrap_or_default();
            Piece { image: Some((c[1].to_string(), alt)), ..Default::default() }
        })
        .collect()
}

fn strip_tags(html: &str) -> String {
    let re = regex::Regex::new(r"(?s)<[^>]*>").unwrap();
    re.replace_all(html, "").trim().to_string()
}

struct P<'a> {
    it: std::iter::Peekable<Parser<'a>>,
}

impl<'a> P<'a> {
    /// Blocks until the end tag that closes the current container.
    fn blocks(&mut self, item: Option<&mut Item>) -> Vec<Block> {
        let mut out = Vec::new();
        let mut item = item;
        loop {
            let Some(ev) = self.it.peek().cloned() else { break };
            match ev {
                Event::End(_) => {
                    self.it.next();
                    break;
                }
                Event::Start(Tag::Paragraph) => {
                    self.it.next();
                    let inl = self.inline();
                    out.push(Block::Para(inl));
                }
                Event::Start(Tag::Heading { level, .. }) => {
                    self.it.next();
                    let inline = self.inline();
                    let level = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    };
                    let anchor = slug(&plain(&inline));
                    out.push(Block::Heading { level, inline, anchor });
                }
                Event::Start(Tag::CodeBlock(kind)) => {
                    self.it.next();
                    let mut text = String::new();
                    while let Some(e) = self.it.next() {
                        match e {
                            Event::Text(t) => text.push_str(&t),
                            Event::End(_) => break,
                            _ => {}
                        }
                    }
                    let lang = match kind {
                        CodeBlockKind::Fenced(info) => info.split([',', ' ']).next().unwrap_or("").to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    if text.ends_with('\n') {
                        text.pop();
                    }
                    let no = std::sync::atomic::AtomicUsize::new(0);
                    let spans = syntax::by_alias(&lang).and_then(|l| syntax::highlight(l, &text, &no)).unwrap_or_default();
                    out.push(Block::Code { text, spans });
                }
                Event::Start(Tag::BlockQuote(alert)) => {
                    self.it.next();
                    let blocks = self.blocks(None);
                    out.push(Block::Quote { alert, blocks });
                }
                Event::Start(Tag::List(start)) => {
                    self.it.next();
                    let mut items = Vec::new();
                    loop {
                        match self.it.next() {
                            Some(Event::Start(Tag::Item)) => {
                                let mut it = Item::default();
                                it.blocks = self.blocks(Some(&mut it));
                                items.push(it);
                            }
                            Some(Event::End(_)) | None => break,
                            _ => {}
                        }
                    }
                    out.push(Block::List { start, items });
                }
                Event::Start(Tag::Table(aligns)) => {
                    self.it.next();
                    let (mut head, mut rows) = (Vec::new(), Vec::new());
                    loop {
                        match self.it.next() {
                            Some(Event::Start(Tag::TableHead)) => head = self.row(),
                            Some(Event::Start(Tag::TableRow)) => rows.push(self.row()),
                            Some(Event::End(TagEnd::Table)) | None => break,
                            _ => {}
                        }
                    }
                    out.push(Block::Table { aligns, head, rows });
                }
                Event::Start(Tag::HtmlBlock) => {
                    self.it.next();
                    let mut html = String::new();
                    while let Some(e) = self.it.next() {
                        match e {
                            Event::Html(t) | Event::Text(t) => html.push_str(&t),
                            Event::End(_) => break,
                            _ => {}
                        }
                    }
                    let mut inl = html_images(&html);
                    let text = strip_tags(&html);
                    if !text.is_empty() {
                        inl.push(Piece { text, ..Default::default() });
                    }
                    if !inl.is_empty() {
                        out.push(Block::Para(inl));
                    }
                }
                Event::Rule => {
                    self.it.next();
                    out.push(Block::Rule);
                }
                Event::TaskListMarker(done) => {
                    self.it.next();
                    if let Some(it) = item.as_deref_mut() {
                        it.task = Some(done);
                    }
                }
                Event::Start(Tag::FootnoteDefinition(_) | Tag::MetadataBlock(_) | Tag::DefinitionList) => {
                    self.it.next();
                    let inner = self.blocks(None);
                    out.extend(inner);
                }
                // Loose inline content (tight list items): gather a paragraph.
                _ => {
                    let inl = self.inline_until_block();
                    if !inl.is_empty() {
                        out.push(Block::Para(inl));
                    }
                }
            }
        }
        out
    }

    fn row(&mut self) -> Vec<Inline> {
        let mut cells = Vec::new();
        loop {
            match self.it.next() {
                Some(Event::Start(Tag::TableCell)) => cells.push(self.inline()),
                Some(Event::End(_)) | None => break,
                _ => {}
            }
        }
        cells
    }

    /// Inline content up to the end tag of the current element.
    fn inline(&mut self) -> Inline {
        let mut st = Style::default();
        let mut out = Vec::new();
        let mut depth = 0;
        while let Some(ev) = self.it.next() {
            match ev {
                Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::TableCell) if depth == 0 => break,
                ev => {
                    if let Event::Start(_) = ev {
                        depth += 1;
                    }
                    if let Event::End(_) = ev {
                        depth -= 1;
                    }
                    st.event(ev, &mut out);
                }
            }
        }
        out
    }

    /// Inline content until a block starts or the container ends.
    fn inline_until_block(&mut self) -> Inline {
        let mut st = Style::default();
        let mut out = Vec::new();
        let mut depth = 0;
        while let Some(ev) = self.it.peek().cloned() {
            let inline_ev = matches!(
                ev,
                Event::Text(_) | Event::Code(_) | Event::InlineHtml(_) | Event::SoftBreak | Event::HardBreak | Event::FootnoteReference(_) | Event::InlineMath(_)
            ) || matches!(&ev, Event::Start(Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } | Tag::Image { .. } | Tag::Superscript | Tag::Subscript))
                || (depth > 0 && matches!(ev, Event::End(_)));
            if !inline_ev {
                break;
            }
            self.it.next();
            if let Event::Start(_) = ev {
                depth += 1;
            }
            if let Event::End(_) = ev {
                depth -= 1;
            }
            st.event(ev, &mut out);
        }
        out
    }
}

#[derive(Default)]
struct Style {
    bold: u32,
    italic: u32,
    strike: u32,
    link: Vec<String>,
    image: Option<(String, String)>,
}

impl Style {
    fn piece(&self, text: String, code: bool) -> Piece {
        Piece { text, bold: self.bold > 0, italic: self.italic > 0, strike: self.strike > 0, code, link: self.link.last().cloned(), image: None }
    }

    fn event(&mut self, ev: Event, out: &mut Inline) {
        match ev {
            Event::Start(Tag::Strong) => self.bold += 1,
            Event::End(TagEnd::Strong) => self.bold = self.bold.saturating_sub(1),
            Event::Start(Tag::Emphasis) => self.italic += 1,
            Event::End(TagEnd::Emphasis) => self.italic = self.italic.saturating_sub(1),
            Event::Start(Tag::Strikethrough) => self.strike += 1,
            Event::End(TagEnd::Strikethrough) => self.strike = self.strike.saturating_sub(1),
            Event::Start(Tag::Link { dest_url, .. }) => self.link.push(dest_url.to_string()),
            Event::End(TagEnd::Link) => {
                self.link.pop();
            }
            Event::Start(Tag::Image { dest_url, .. }) => self.image = Some((dest_url.to_string(), String::new())),
            Event::End(TagEnd::Image) => {
                if let Some(img) = self.image.take() {
                    out.push(Piece { image: Some(img), link: self.link.last().cloned(), ..Default::default() });
                }
            }
            Event::Text(t) => match self.image.as_mut() {
                Some((_, alt)) => alt.push_str(&t),
                None => out.push(self.piece(t.to_string(), false)),
            },
            Event::Code(t) => out.push(self.piece(t.to_string(), true)),
            Event::InlineMath(t) => out.push(self.piece(t.to_string(), true)),
            Event::SoftBreak => out.push(self.piece(" ".into(), false)),
            Event::HardBreak => out.push(self.piece("\n".into(), false)),
            Event::InlineHtml(h) => {
                let h = h.to_lowercase();
                if h.starts_with("<br") {
                    out.push(self.piece("\n".into(), false));
                } else if h.starts_with("<img") {
                    out.extend(html_images(&h));
                } else if h.starts_with("<b>") || h.starts_with("<strong") {
                    self.bold += 1;
                } else if h.starts_with("</b>") || h.starts_with("</strong") {
                    self.bold = self.bold.saturating_sub(1);
                }
            }
            Event::FootnoteReference(r) => out.push(self.piece(format!("[{r}]"), false)),
            _ => {}
        }
    }
}

pub fn parse(text: &str) -> MdDoc {
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_GFM
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
    let mut p = P { it: Parser::new_ext(text, opts).peekable() };
    MdDoc { blocks: p.blocks(None) }
}

// ---------------------------------------------------------------- drawing

/// What a click in the document asks for.
pub enum Click {
    Link(String),
}

pub struct Ctx<'a> {
    pub theme: &'a Theme,
    pub base: f32,
    pub mono: f32,
    pub dir: &'a Path,
    /// Heading anchors → y offset in the content.
    pub anchors: &'a mut HashMap<String, f32>,
    pub top: f32,
    pub click: Option<Click>,
}

fn code_bg(t: &Theme) -> Color32 {
    t.bg.lerp_to_gamma(t.fg, 0.1)
}

fn job_for(inline: &Inline, c: &Ctx, size: f32, color: Color32, bold_all: bool, width: f32) -> (egui::text::LayoutJob, Vec<(usize, usize, String)>) {
    let mut job = egui::text::LayoutJob::default();
    let mut links = Vec::new();
    let mut chars = 0;
    for p in inline.iter().filter(|p| p.image.is_none()) {
        let family = if p.bold || bold_all { FontFamily::Name("bold".into()) } else { FontFamily::Proportional };
        let mut fmt = egui::TextFormat { font_id: FontId::new(size, family), color, italics: p.italic, ..Default::default() };
        if p.code {
            fmt.font_id = FontId::new(size * 0.88, FontFamily::Name("viewer".into()));
            fmt.background = code_bg(c.theme);
        }
        if p.strike {
            fmt.strikethrough = Stroke::new(1.0, color);
        }
        if let Some(l) = &p.link {
            fmt.color = c.theme.link;
            fmt.underline = Stroke::new(1.0, c.theme.link.gamma_multiply(0.5));
            let n = p.text.chars().count();
            links.push((chars, chars + n, l.clone()));
        }
        chars += p.text.chars().count();
        job.append(&p.text, 0.0, fmt);
    }
    job.wrap.max_width = width;
    (job, links)
}

/// Paint text with clickable links.
fn text(ui: &mut Ui, c: &mut Ctx, inline: &Inline, size: f32, color: Color32, bold: bool) {
    let width = ui.available_width();
    let (job, links) = job_for(inline, c, size, color, bold, width);
    if job.text.is_empty() {
        return;
    }
    let galley: Arc<Galley> = ui.painter().layout_job(job);
    let (rect, resp) = ui.allocate_exact_size(galley.size(), Sense::click());
    ui.painter().galley(rect.min, galley.clone(), color);
    if links.is_empty() {
        return;
    }
    let link_at = |pos: egui::Pos2| {
        let i = galley.cursor_from_pos(pos - rect.min).index.0;
        links.iter().find(|(a, b, _)| *a <= i && i < *b).map(|(_, _, l)| l.clone())
    };
    if let Some(p) = resp.hover_pos() {
        if let Some(l) = link_at(p) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            resp.clone().on_hover_text(l);
        }
    }
    if resp.clicked() {
        if let Some(l) = resp.interact_pointer_pos().and_then(link_at) {
            c.click = Some(Click::Link(l));
        }
    }
}

fn image_uri(src: &str, dir: &Path) -> Option<String> {
    if src.starts_with("http://") || src.starts_with("https://") || src.starts_with("data:") {
        return None; // no network fetching
    }
    let p = src.strip_prefix("file://").map(PathBuf::from).unwrap_or_else(|| dir.join(src.split(['?', '#']).next().unwrap_or(src)));
    p.exists().then(|| format!("file://{}", p.display()))
}

/// A paragraph: text, with images on their own.
fn para(ui: &mut Ui, c: &mut Ctx, inline: &Inline) {
    if inline.iter().all(|p| p.image.is_none()) {
        text(ui, c, inline, c.base, c.theme.fg, false);
        return;
    }
    ui.horizontal_wrapped(|ui| {
        let mut run: Inline = Vec::new();
        for p in inline {
            match &p.image {
                None => run.push(p.clone()),
                Some((src, alt)) => {
                    if !run.is_empty() {
                        let r = std::mem::take(&mut run);
                        if r.iter().any(|p| !p.text.trim().is_empty()) {
                            text(ui, c, &r, c.base, c.theme.fg, false);
                        }
                    }
                    match image_uri(src, c.dir) {
                        Some(uri) => {
                            let max = ui.available_width().min(900.0);
                            let resp = ui.add(egui::Image::new(uri).max_width(max).fit_to_original_size(1.0).sense(Sense::click()));
                            if let (true, Some(l)) = (resp.clicked(), &p.link) {
                                c.click = Some(Click::Link(l.clone()));
                            }
                        }
                        None => {
                            // Remote pictures (badges…) show their alt text.
                            let label = if alt.is_empty() { src.rsplit('/').next().unwrap_or(src).to_string() } else { alt.clone() };
                            let piece = Piece { text: format!("🖼 {label}"), link: p.link.clone().or(Some(src.clone())), ..Default::default() };
                            text(ui, c, &vec![piece], c.base * 0.9, c.theme.fg, false);
                        }
                    }
                }
            }
        }
        if run.iter().any(|p| !p.text.trim().is_empty()) {
            text(ui, c, &run, c.base, c.theme.fg, false);
        }
    });
}

fn alert_style(k: BlockQuoteKind) -> (Color32, &'static str) {
    match k {
        BlockQuoteKind::Note => (Color32::from_rgb(0x47, 0x8b, 0xe6), tr("Note")),
        BlockQuoteKind::Tip => (Color32::from_rgb(0x3f, 0xb9, 0x50), tr("Tip")),
        BlockQuoteKind::Important => (Color32::from_rgb(0xa3, 0x71, 0xf7), tr("Important")),
        BlockQuoteKind::Warning => (Color32::from_rgb(0xd2, 0x99, 0x22), tr("Warning")),
        BlockQuoteKind::Caution => (Color32::from_rgb(0xf8, 0x51, 0x49), tr("Caution")),
    }
}

fn blocks(ui: &mut Ui, c: &mut Ctx, list: &[Block], depth: usize) {
    for (i, b) in list.iter().enumerate() {
        if i > 0 {
            ui.add_space(c.base * 0.7);
        }
        block(ui, c, b, depth);
    }
}

fn block(ui: &mut Ui, c: &mut Ctx, b: &Block, depth: usize) {
    let t = c.theme;
    match b {
        Block::Heading { level, inline, anchor } => {
            let k = [2.0, 1.5, 1.25, 1.0, 0.875, 0.85][(*level as usize - 1).min(5)];
            ui.add_space(c.base * 0.6);
            let y = ui.cursor().min.y - c.top;
            c.anchors.entry(anchor.clone()).or_insert(y);
            let color = if *level == 6 { t.gutter } else { t.fg };
            text(ui, c, inline, c.base * k, color, true);
            if *level <= 2 {
                ui.add_space(c.base * 0.3);
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                ui.painter().rect_filled(r, 0.0, t.gutter.gamma_multiply(0.35));
            }
        }
        Block::Para(inline) => para(ui, c, inline),
        Block::Code { text: code, spans } => {
            let colors = t.table();
            let font = FontId::new(c.mono, FontFamily::Name("viewer".into()));
            let mut job = egui::text::LayoutJob::default();
            let mut at = 0;
            for s in spans {
                if s.start > at {
                    job.append(&code[at..s.start], 0.0, egui::TextFormat { font_id: font.clone(), color: t.fg, ..Default::default() });
                }
                if s.end > at.max(s.start) {
                    let italics = NAMES[s.class as usize] == "comment";
                    job.append(&code[at.max(s.start)..s.end], 0.0, egui::TextFormat { font_id: font.clone(), color: colors[s.class as usize], italics, ..Default::default() });
                }
                at = at.max(s.end);
            }
            job.append(&code[at..], 0.0, egui::TextFormat { font_id: font.clone(), color: t.fg, ..Default::default() });
            egui::Frame::new().fill(code_bg(t)).corner_radius(6.0).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                job.wrap.max_width = ui.available_width();
                let g = ui.painter().layout_job(job);
                let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
                ui.painter().galley(r.min, g, t.fg);
            });
        }
        Block::Quote { alert, blocks: inner } => {
            let (bar, title) = match alert {
                Some(k) => {
                    let (col, title) = alert_style(*k);
                    (col, Some(title))
                }
                None => (t.gutter.gamma_multiply(0.6), None),
            };
            let r = ui
                .horizontal_top(|ui| {
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        if let Some(title) = title {
                            ui.horizontal(|ui| {
                                // Drawn, not a glyph: fonts often lack ⓘ.
                                let (r, _) = ui.allocate_exact_size(egui::Vec2::splat(c.base * 1.2), Sense::hover());
                                let mark = if matches!(alert, Some(BlockQuoteKind::Warning | BlockQuoteKind::Caution)) { "!" } else { "i" };
                                ui.painter().circle_stroke(r.center(), c.base * 0.48, Stroke::new(1.6, bar));
                                ui.painter().text(r.center(), Align2::CENTER_CENTER, mark, FontId::new(c.base * 0.75, FontFamily::Name("bold".into())), bar);
                                ui.add_space(c.base * 0.4);
                                let piece = Piece { text: title.to_string(), ..Default::default() };
                                text(ui, c, &vec![piece], c.base, bar, true);
                            });
                            ui.add_space(c.base * 0.3);
                        }
                        // Quotes are dimmer, as on GitHub.
                        let fg = c.theme.fg;
                        let dim = Theme { fg: fg.gamma_multiply(0.75), ..c.theme.clone() };
                        let mut cc = Ctx { theme: &dim, base: c.base, mono: c.mono, dir: c.dir, anchors: c.anchors, top: c.top, click: None };
                        blocks(ui, &mut cc, inner, depth);
                        if cc.click.is_some() {
                            c.click = cc.click;
                        }
                    });
                })
                .response
                .rect;
            ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(4.0, r.height())), 2.0, bar);
        }
        Block::List { start, items } => {
            for (n, item) in items.iter().enumerate() {
                if n > 0 {
                    ui.add_space(c.base * 0.25);
                }
                ui.horizontal_top(|ui| {
                    let w = c.base * 1.8;
                    let (r, resp) = ui.allocate_exact_size(vec2(w, c.base * 1.3), Sense::hover());
                    let _ = resp;
                    let mid = pos2(r.max.x - c.base * 0.6, r.center().y);
                    match (item.task, start) {
                        (Some(done), _) => {
                            let b = Rect::from_center_size(mid, egui::Vec2::splat(c.base * 0.85));
                            ui.painter().rect(b, 3.0, if done { t.link } else { Color32::TRANSPARENT }, Stroke::new(1.2, if done { t.link } else { t.gutter }), egui::StrokeKind::Inside);
                            if done {
                                ui.painter().text(b.center(), Align2::CENTER_CENTER, "✔", FontId::proportional(c.base * 0.7), t.bg);
                            }
                        }
                        (None, Some(s)) => {
                            ui.painter().text(pos2(r.max.x - 4.0, r.center().y), Align2::RIGHT_CENTER, format!("{}.", s + n as u64), FontId::proportional(c.base), t.fg);
                        }
                        (None, None) => {
                            let rad = c.base * 0.17;
                            match depth % 3 {
                                0 => {
                                    ui.painter().circle_filled(mid, rad, t.fg);
                                }
                                1 => {
                                    ui.painter().circle_stroke(mid, rad, Stroke::new(1.2, t.fg));
                                }
                                _ => {
                                    ui.painter().rect_filled(Rect::from_center_size(mid, egui::Vec2::splat(rad * 1.8)), 0.0, t.fg);
                                }
                            }
                        }
                    }
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        blocks(ui, c, &item.blocks, depth + 1);
                    });
                });
            }
        }
        Block::Rule => {
            ui.add_space(c.base * 0.4);
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 3.0), Sense::hover());
            ui.painter().rect_filled(r, 1.0, t.gutter.gamma_multiply(0.35));
            ui.add_space(c.base * 0.4);
        }
        Block::Table { aligns, head, rows } => {
            // Measure every cell, then size the columns like a browser would.
            let border = t.gutter.gamma_multiply(0.4);
            let pad = vec2(12.0, 6.0);
            let lay = |inl: &Inline, bold: bool| -> Arc<Galley> {
                let (job, _) = job_for(inl, c, c.base, t.fg, bold, 420.0);
                ui.painter().layout_job(job)
            };
            let mut grid: Vec<Vec<Arc<Galley>>> = vec![head.iter().map(|h| lay(h, true)).collect()];
            grid.extend(rows.iter().map(|r| r.iter().map(|v| lay(v, false)).collect()));
            let ncols = grid.iter().map(|r| r.len()).max().unwrap_or(0);
            let mut widths = vec![0.0f32; ncols];
            for r in &grid {
                for (i, g) in r.iter().enumerate() {
                    widths[i] = widths[i].max(g.size().x + pad.x * 2.0);
                }
            }
            let heights: Vec<f32> = grid.iter().map(|r| r.iter().map(|g| g.size().y).fold(c.base * 1.2, f32::max) + pad.y * 2.0).collect();
            let total = vec2(widths.iter().sum(), heights.iter().sum());
            let (rect, _) = ui.allocate_exact_size(total, Sense::hover());
            let p = ui.painter();
            let mut y = rect.min.y;
            for (ri, r) in grid.iter().enumerate() {
                let h = heights[ri];
                if ri > 0 && ri % 2 == 0 {
                    p.rect_filled(Rect::from_min_size(pos2(rect.min.x, y), vec2(total.x, h)), 0.0, code_bg(t).gamma_multiply(0.6));
                }
                let mut x = rect.min.x;
                for (ci, w) in widths.iter().enumerate() {
                    let cell = Rect::from_min_size(pos2(x, y), vec2(*w, h));
                    p.rect_stroke(cell, 0.0, Stroke::new(1.0, border), egui::StrokeKind::Inside);
                    if let Some(g) = r.get(ci) {
                        let gx = match aligns.get(ci).copied().unwrap_or(Alignment::None) {
                            Alignment::Center => cell.center().x - g.size().x / 2.0,
                            Alignment::Right => cell.max.x - pad.x - g.size().x,
                            _ => cell.min.x + pad.x,
                        };
                        p.galley(pos2(gx, cell.min.y + pad.y), g.clone(), t.fg);
                    }
                    x += w;
                }
                y += h;
            }
        }
    }
}

/// Draw the document into `ui` (inside a scroll area).
pub fn show(ui: &mut Ui, doc: &MdDoc, c: &mut Ctx) {
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    blocks(ui, c, &doc.blocks, 0);
    ui.add_space(c.base * 2.0);
    let _ = CornerRadius::ZERO;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_markdown() {
        let md = "# Title\n\nText **bold** [link](#title).\n\n- [x] done\n- todo\n  1. nested\n\n> [!NOTE]\n> Careful\n\n| a | b |\n|:-|-:|\n| 1 | 2 |\n\n```rust\nfn x() {}\n```\n\n<p align=\"center\"><img src=\"logo.png\" alt=\"Logo\"></p>\n";
        let d = parse(md);
        let kinds: Vec<&str> = d
            .blocks
            .iter()
            .map(|b| match b {
                Block::Heading { .. } => "h",
                Block::Para(_) => "p",
                Block::Code { .. } => "code",
                Block::Quote { .. } => "quote",
                Block::List { .. } => "list",
                Block::Rule => "hr",
                Block::Table { .. } => "table",
            })
            .collect();
        assert_eq!(kinds, ["h", "p", "list", "quote", "table", "code", "p"]);
        let Block::Heading { anchor, .. } = &d.blocks[0] else { panic!() };
        assert_eq!(anchor, "title");
        let Block::Para(p) = &d.blocks[1] else { panic!() };
        assert!(p.iter().any(|x| x.bold && x.text == "bold"));
        assert!(p.iter().any(|x| x.link.as_deref() == Some("#title")));
        let Block::List { items, .. } = &d.blocks[2] else { panic!() };
        assert_eq!(items[0].task, Some(true));
        assert!(items[1].blocks.iter().any(|b| matches!(b, Block::List { start: Some(1), .. })));
        let Block::Quote { alert, .. } = &d.blocks[3] else { panic!() };
        assert_eq!(*alert, Some(BlockQuoteKind::Note));
        let Block::Code { spans, .. } = &d.blocks[5] else { panic!() };
        assert!(!spans.is_empty(), "code block not highlighted");
        let Block::Para(img) = &d.blocks[6] else { panic!() };
        assert_eq!(img[0].image.as_ref().map(|i| i.0.as_str()), Some("logo.png"));
        assert_eq!(slug("Как это работает?"), "как-это-работает");
    }
}
