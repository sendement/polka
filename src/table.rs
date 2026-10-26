//! Tables: CSV (any separator, found by itself or chosen), TSV and the
//! spreadsheets (xlsx, xlsm, xls, xlsb, ods) read; CSV, TSV, xlsx, ods,
//! JSON, Markdown and HTML written. A simple table (a header row, then
//! data) can be cut every n rows, the header on each part, and several of
//! them with the same header (case aside, any column order) glued into one.

use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Empty,
    Text(String),
    Num(f64),
    Bool(bool),
    Date(chrono::NaiveDateTime),
    /// Text that stays text (a column chosen as such: "007" isn't 7).
    Plain(String),
}

impl Cell {
    pub fn text(&self) -> String {
        match self {
            Cell::Empty => String::new(),
            Cell::Text(s) | Cell::Plain(s) => s.clone(),
            Cell::Num(n) => num_text(*n),
            Cell::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
            Cell::Date(t) => date_text(t),
        }
    }

    /// Text that is a plain number ("12", "-3.5", not "007" or "1e5")
    /// becomes one, for the spreadsheets.
    fn typed(&self) -> Cell {
        match self {
            Cell::Text(s) => {
                let t = s.trim();
                let plain = !t.is_empty()
                    && t.len() <= 15
                    && t.bytes().all(|b| b.is_ascii_digit() || b == b'.' || b == b'-')
                    && !(t.len() > 1 && t.starts_with('0') && !t.starts_with("0."))
                    && !(t.starts_with("-0") && !t.starts_with("-0.") && t.len() > 2);
                match t.parse::<f64>() {
                    Ok(n) if plain && n.is_finite() => Cell::Num(n),
                    _ => self.clone(),
                }
            }
            c => c.clone(),
        }
    }
}

fn num_text(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { format!("{n}") }
}

/// A date alone, or with its time.
fn date_text(t: &chrono::NaiveDateTime) -> String {
    if t.time() == chrono::NaiveTime::MIN { t.format("%Y-%m-%d").to_string() } else { t.format("%Y-%m-%d %H:%M:%S").to_string() }
}

/// A number as people write it: "1 234,5", "1.234.567", "1,234.5", "+3".
fn parse_num(s: &str) -> Option<f64> {
    let t: String = s.trim().chars().filter(|c| !matches!(c, ' ' | '\u{a0}' | '\u{202f}' | '\'' | '_')).collect();
    let t = t.strip_prefix('+').unwrap_or(&t);
    let (commas, dots) = (t.matches(',').count(), t.matches('.').count());
    let t = match (commas, dots) {
        (0, 0 | 1) => t.to_string(),
        (0, _) => t.replace('.', ""),
        (1, 0) => t.replace(',', "."),
        (_, 0) => t.replace(',', ""),
        // Both: the last one is the decimal point.
        _ if t.rfind(',') > t.rfind('.') => t.replace('.', "").replace(',', "."),
        _ => t.replace(',', ""),
    };
    if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'e' | b'E')) {
        return None;
    }
    t.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// A date (maybe with its time) in the usual ways: 2024-03-01, 01.03.2024,
/// 01/03/2024 (day first, else month first), 01.03.24…
fn parse_date(s: &str) -> Option<chrono::NaiveDateTime> {
    use chrono::{NaiveDate, NaiveDateTime};
    const DAYS: [&str; 9] = ["%Y-%m-%d", "%d.%m.%Y", "%d/%m/%Y", "%m/%d/%Y", "%Y/%m/%d", "%d-%m-%Y", "%Y.%m.%d", "%d.%m.%y", "%d/%m/%y"];
    const TIMES: [&str; 6] = [" %H:%M:%S", " %H:%M", "T%H:%M:%S", "T%H:%M", " %H:%M:%S%.f", "T%H:%M:%S%.f"];
    let t = s.trim();
    // (%Y takes "24" as the year 24.)
    let sane = |d: &NaiveDate, f: &str| f.contains("%y") || chrono::Datelike::year(d) >= 1000;
    for f in DAYS {
        if let Ok(d) = NaiveDate::parse_from_str(t, f) {
            if sane(&d, f) {
                return Some(d.and_time(chrono::NaiveTime::MIN));
            }
        }
        for tm in TIMES {
            let full = format!("{f}{tm}");
            if let Ok(dt) = NaiveDateTime::parse_from_str(t.trim_end_matches('Z'), &full) {
                if sane(&dt.date(), f) {
                    return Some(dt);
                }
            }
        }
    }
    None
}

/// What a column's cells become.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColType {
    /// Plain numbers become numbers, the rest stays text.
    Auto,
    Text,
    Number,
    Date,
}

pub const COL_TYPES: [ColType; 4] = [ColType::Auto, ColType::Text, ColType::Number, ColType::Date];

impl ColType {
    pub fn label(self) -> &'static str {
        crate::i18n::tr(match self {
            ColType::Auto => "Automatic",
            ColType::Text => "Text",
            ColType::Number => "Number",
            ColType::Date => "Date",
        })
    }

    /// The cell as this column has it (what doesn't read as one stays).
    pub fn apply(self, c: &Cell) -> Cell {
        match (self, c) {
            (ColType::Text, Cell::Text(s)) => Cell::Plain(s.clone()),
            (ColType::Number, Cell::Text(s)) => parse_num(s).map(Cell::Num).unwrap_or_else(|| c.clone()),
            (ColType::Date, Cell::Text(s)) => parse_date(s).map(Cell::Date).unwrap_or_else(|| c.clone()),
            _ => c.clone(),
        }
    }

    /// The cell as it'll be written (Auto's numbers too: for a preview).
    pub fn shown(self, c: &Cell) -> Cell {
        if self == ColType::Auto { c.typed() } else { self.apply(c) }
    }

    /// Whether the cell reads as this column wants (empty ones do).
    pub fn fits(self, c: &Cell) -> bool {
        match self {
            ColType::Number => matches!(self.apply(c), Cell::Num(_) | Cell::Empty),
            ColType::Date => matches!(self.apply(c), Cell::Date(_) | Cell::Empty),
            _ => true,
        }
    }
}

/// Give a table's columns their types (its header row aside).
pub fn shape(s: &mut Sheet, cols: &[ColType]) {
    if cols.iter().all(|c| *c == ColType::Auto) {
        return;
    }
    let skip = usize::from(s.header().is_some());
    for r in s.rows.iter_mut().skip(skip) {
        for (c, t) in r.iter_mut().zip(cols) {
            *c = t.apply(c);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<Cell>>,
}

impl Sheet {
    /// Its header, when it's a simple table: a first row of distinct, non-
    /// empty text (not numbers), and data under it.
    pub fn header(&self) -> Option<Vec<String>> {
        let first = self.rows.first()?;
        if self.rows.len() < 2 || first.is_empty() {
            return None;
        }
        let names: Vec<String> = first.iter().map(|c| match c {
            Cell::Text(s) if !s.trim().is_empty() && !matches!(c.typed(), Cell::Num(_)) => Some(s.trim().to_string()),
            _ => None,
        }).collect::<Option<_>>()?;
        let mut seen: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
        seen.sort();
        seen.dedup();
        (seen.len() == names.len()).then_some(names)
    }
}

// ---------------------------------------------------------------- separators

/// A CSV file's separator: found by itself, or chosen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Delim {
    #[default]
    Auto,
    Comma,
    Semicolon,
    Tab,
    Pipe,
}

pub const DELIMS: [Delim; 5] = [Delim::Auto, Delim::Comma, Delim::Semicolon, Delim::Tab, Delim::Pipe];

impl Delim {
    pub fn byte(self) -> Option<u8> {
        match self {
            Delim::Auto => None,
            Delim::Comma => Some(b','),
            Delim::Semicolon => Some(b';'),
            Delim::Tab => Some(b'\t'),
            Delim::Pipe => Some(b'|'),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Delim::Auto => crate::i18n::tr("Automatic"),
            Delim::Comma => ", ",
            Delim::Semicolon => "; ",
            Delim::Tab => "Tab",
            Delim::Pipe => "| ",
        }
    }

    pub fn of(b: u8) -> Delim {
        match b {
            b';' => Delim::Semicolon,
            b'\t' => Delim::Tab,
            b'|' => Delim::Pipe,
            _ => Delim::Comma,
        }
    }
}

/// The separator a CSV text most likely uses: the one that splits its
/// first lines into the same number of fields, the most of them (quotes
/// heeded).
pub fn detect(text: &str) -> Delim {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).take(20).collect();
    let mut best = (b',', 0usize, false);
    for d in [b',', b';', b'\t', b'|'] {
        let counts: Vec<usize> = lines.iter().map(|l| split_count(l, d)).collect();
        let Some(&first) = counts.first() else { continue };
        let even = counts.iter().all(|c| *c == first);
        if first > 1 && ((even && !best.2) || (even == best.2 && first > best.1)) {
            best = (d, first, even);
        }
    }
    Delim::of(best.0)
}

fn split_count(line: &str, d: u8) -> usize {
    let mut quoted = false;
    let mut n = 1;
    for b in line.bytes() {
        if b == b'"' {
            quoted = !quoted;
        } else if b == d && !quoted {
            n += 1;
        }
    }
    n
}

// ---------------------------------------------------------------- reading

/// The kinds of file read as tables.
pub const READ_EXTS: &[&str] = &["csv", "tsv", "tab", "xlsx", "xlsm", "xls", "xlsb", "ods"];

pub fn is_table(name: &str) -> bool {
    READ_EXTS.contains(&ext(name).as_str())
}

pub fn is_csv(name: &str) -> bool {
    ext(name) == "csv"
}

fn ext(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default()
}

/// A text file's text: UTF-8 (BOM or not), UTF-16 with a BOM, else
/// Windows-1251 (what Excel saves CSV as on a Russian system).
fn read_text(path: &Path) -> Result<String, String> {
    decode(std::fs::read(path).map_err(|e| e.to_string())?)
}

/// Its beginning only (whole lines), for a look at a big file.
fn read_head(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut b = Vec::new();
    std::fs::File::open(path).and_then(|f| f.take(256 * 1024).read_to_end(&mut b)).map_err(|e| e.to_string())?;
    if b.len() == 256 * 1024 {
        if let Some(end) = b.iter().rposition(|c| *c == b'\n') {
            b.truncate(end + 1);
        }
    }
    decode(b)
}

fn decode(b: Vec<u8>) -> Result<String, String> {
    if let Some((enc, bom)) = encoding_rs::Encoding::for_bom(&b) {
        return Ok(enc.decode_without_bom_handling(&b[bom..]).0.into_owned());
    }
    match String::from_utf8(b) {
        Ok(s) => Ok(s),
        Err(e) => Ok(encoding_rs::WINDOWS_1251.decode(e.as_bytes()).0.into_owned()),
    }
}

fn parse_csv(text: &str, d: u8, limit: Option<usize>) -> Result<Vec<Vec<Cell>>, String> {
    let mut r = csv::ReaderBuilder::new().has_headers(false).flexible(true).delimiter(d).from_reader(text.as_bytes());
    let mut rows = Vec::new();
    for rec in r.records() {
        let rec = rec.map_err(|e| e.to_string())?;
        rows.push(rec.iter().map(|f| if f.is_empty() { Cell::Empty } else { Cell::Text(f.to_string()) }).collect());
        if limit.is_some_and(|l| rows.len() >= l) {
            break;
        }
    }
    Ok(rows)
}

/// The separator a file is read with (a TSV's is a tab).
pub fn delim_for(path: &Path, chosen: Delim) -> Result<u8, String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if matches!(ext(&name).as_str(), "tsv" | "tab") {
        return Ok(b'\t');
    }
    match chosen.byte() {
        Some(b) => Ok(b),
        None => Ok(detect(&read_head(path)?).byte().unwrap_or(b',')),
    }
}

/// Its sheets (a CSV has one, named after the file).
pub fn read(path: &Path, delim: Delim) -> Result<Vec<Sheet>, String> {
    read_some(path, delim, None)
}

/// The first rows of its first sheet (for a preview).
pub fn preview(path: &Path, delim: Delim, rows: usize) -> Result<Sheet, String> {
    let sheets = read_some(path, delim, Some(rows))?;
    sheets.into_iter().next().ok_or_else(|| "empty".into())
}

fn read_some(path: &Path, delim: Delim, limit: Option<usize>) -> Result<Vec<Sheet>, String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = crate::ops::split_ext(&name).0.to_string();
    match ext(&name).as_str() {
        "csv" | "tsv" | "tab" => {
            let text = if limit.is_some() { read_head(path)? } else { read_text(path)? };
            let d = match (ext(&name).as_str(), delim.byte()) {
                ("tsv" | "tab", _) => b'\t',
                (_, Some(b)) => b,
                _ => detect(&text).byte().unwrap_or(b','),
            };
            Ok(vec![Sheet { name: stem, rows: parse_csv(&text, d, limit)? }])
        }
        _ => {
            use calamine::Reader;
            let mut wb = calamine::open_workbook_auto(path).map_err(|e| e.to_string())?;
            let mut out = Vec::new();
            for sheet in wb.sheet_names() {
                let range = wb.worksheet_range(&sheet).map_err(|e| e.to_string())?;
                let mut rows: Vec<Vec<Cell>> = Vec::new();
                for r in range.rows() {
                    rows.push(r.iter().map(cell_of).collect());
                    if limit.is_some_and(|l| rows.len() >= l) {
                        break;
                    }
                }
                // Trailing empty cells and rows dropped.
                for r in &mut rows {
                    while r.last() == Some(&Cell::Empty) {
                        r.pop();
                    }
                }
                while rows.last().is_some_and(|r| r.is_empty()) {
                    rows.pop();
                }
                out.push(Sheet { name: sheet, rows });
                if limit.is_some() {
                    break;
                }
            }
            Ok(out)
        }
    }
}

fn cell_of(d: &calamine::Data) -> Cell {
    use calamine::Data;
    match d {
        Data::Empty | Data::Error(_) => Cell::Empty,
        Data::String(s) => Cell::Text(s.clone()),
        Data::Int(i) => Cell::Num(*i as f64),
        Data::Float(f) => Cell::Num(*f),
        Data::Bool(b) => Cell::Bool(*b),
        Data::DateTime(dt) => match dt.as_datetime() {
            Some(t) if dt.is_datetime() => Cell::Date(t),
            _ => Cell::Num(dt.as_f64()),
        },
        // (ODS has its dates so.)
        Data::DateTimeIso(s) => parse_date(s).map(Cell::Date).unwrap_or_else(|| Cell::Text(s.clone())),
        Data::DurationIso(s) => Cell::Text(s.clone()),
    }
}

// ---------------------------------------------------------------- writing

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Csv,
    Tsv,
    Xlsx,
    Ods,
    Json,
    Markdown,
    Html,
}

pub const FORMATS: [Format; 7] = [Format::Csv, Format::Xlsx, Format::Ods, Format::Tsv, Format::Json, Format::Markdown, Format::Html];

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Format::Csv => "CSV",
            Format::Tsv => "TSV",
            Format::Xlsx => "XLSX",
            Format::Ods => "ODS",
            Format::Json => "JSON",
            Format::Markdown => "Markdown",
            Format::Html => "HTML",
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Format::Csv => ".csv",
            Format::Tsv => ".tsv",
            Format::Xlsx => ".xlsx",
            Format::Ods => ".ods",
            Format::Json => ".json",
            Format::Markdown => ".md",
            Format::Html => ".html",
        }
    }

    /// Holds several sheets in one file.
    fn workbook(self) -> bool {
        matches!(self, Format::Xlsx | Format::Ods)
    }
}

/// How tables are converted.
#[derive(Clone, Debug, PartialEq)]
pub struct Opts {
    /// How CSV files are read.
    pub delim_in: Delim,
    /// How CSV is written (Auto: a comma).
    pub delim_out: Delim,
    /// Cut every so many data rows (the header on each part).
    pub split: Option<usize>,
    /// Glue them into one (same headers).
    pub merge: bool,
    /// What CSV's columns become, by position (none: all Auto).
    pub cols: Vec<ColType>,
    /// How the parts are numbered.
    pub numbering: Numbering,
}

/// How cut parts are named after the table: "a 1", "a (1)", "a_01"…
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Numbering {
    #[default]
    Space,
    Paren,
    Underscore,
    Dash,
    Part,
    OfTotal,
}

pub const NUMBERINGS: [Numbering; 6] = [Numbering::Space, Numbering::Paren, Numbering::Underscore, Numbering::Dash, Numbering::Part, Numbering::OfTotal];

impl Numbering {
    /// What follows the name for part `k` (from 1) of `total`; the bare
    /// numbers padded to sort right.
    pub fn tag(self, k: usize, total: usize) -> String {
        let w = total.to_string().len();
        match self {
            Numbering::Space => format!(" {k:0w$}"),
            Numbering::Paren => format!(" ({k:0w$})"),
            Numbering::Underscore => format!("_{k:0w$}"),
            Numbering::Dash => format!("-{k:0w$}"),
            Numbering::Part => format!(" - {}", crate::i18n::trn("part %n", k)),
            Numbering::OfTotal => format!(" ({})", crate::i18n::trn("%n of %t", k).replace("%t", &total.to_string())),
        }
    }
}

impl Default for Opts {
    fn default() -> Self {
        Opts { delim_in: Delim::Auto, delim_out: Delim::Comma, split: None, merge: false, cols: Vec::new(), numbering: Numbering::Space }
    }
}

/// Row counts to cut tables at (the settings step through them).
pub const SPLITS: [usize; 8] = [100, 500, 1000, 5000, 10_000, 50_000, 100_000, 1_000_000];

fn write(path: &Path, sheets: &[Sheet], f: Format, delim: Delim) -> Result<(), String> {
    match f {
        Format::Csv | Format::Tsv => {
            let d = if f == Format::Tsv { b'\t' } else { delim.byte().unwrap_or(b',') };
            let mut w = csv::WriterBuilder::new().delimiter(d).flexible(true).from_path(path).map_err(|e| e.to_string())?;
            for r in &sheets[0].rows {
                w.write_record(r.iter().map(Cell::text)).map_err(|e| e.to_string())?;
            }
            w.flush().map_err(|e| e.to_string())
        }
        Format::Xlsx => {
            let mut wb = rust_xlsxwriter::Workbook::new();
            let mut used: Vec<String> = Vec::new();
            for s in sheets {
                let ws = wb.add_worksheet();
                ws.set_name(sheet_name(&s.name, &mut used)).map_err(|e| e.to_string())?;
                let bold = rust_xlsxwriter::Format::new().set_bold();
                let day = rust_xlsxwriter::Format::new().set_num_format("yyyy-mm-dd");
                let moment = rust_xlsxwriter::Format::new().set_num_format("yyyy-mm-dd hh:mm:ss");
                let header = s.header().is_some();
                for (r, row) in s.rows.iter().enumerate() {
                    for (c, cell) in row.iter().enumerate() {
                        let (r, c) = (r as u32, c as u16);
                        let res = match cell.typed() {
                            Cell::Empty => Ok(&mut *ws),
                            Cell::Num(n) => ws.write_number(r, c, n),
                            Cell::Bool(b) => ws.write_boolean(r, c, b),
                            Cell::Text(t) | Cell::Plain(t) if r == 0 && header => ws.write_string_with_format(r, c, t, &bold),
                            Cell::Text(t) | Cell::Plain(t) => ws.write_string(r, c, t),
                            Cell::Date(t) => {
                                use chrono::{Datelike, Timelike};
                                let at = rust_xlsxwriter::ExcelDateTime::from_ymd(t.year().clamp(1900, 9999) as u16, t.month() as u8, t.day() as u8)
                                    .and_then(|d| d.and_hms(t.hour() as u16, t.minute() as u8, t.second()));
                                match at {
                                    Ok(at) => ws.write_datetime_with_format(r, c, &at, if t.time() == chrono::NaiveTime::MIN { &day } else { &moment }),
                                    Err(_) => ws.write_string(r, c, date_text(&t)),
                                }
                            }
                        };
                        res.map_err(|e| e.to_string())?;
                    }
                }
                if header {
                    let _ = ws.set_freeze_panes(1, 0);
                }
            }
            wb.save(path).map_err(|e| e.to_string())
        }
        Format::Ods => write_ods(path, sheets),
        Format::Json => {
            let s = &sheets[0];
            let v = match s.header() {
                // Objects by the header's names…
                Some(h) => serde_json::Value::Array(
                    s.rows[1..]
                        .iter()
                        .map(|r| serde_json::Value::Object(h.iter().enumerate().map(|(i, k)| (k.clone(), json_of(r.get(i).unwrap_or(&Cell::Empty)))).collect()))
                        .collect(),
                ),
                // …else rows of values.
                None => serde_json::Value::Array(s.rows.iter().map(|r| serde_json::Value::Array(r.iter().map(json_of).collect())).collect()),
            };
            std::fs::write(path, serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
        }
        Format::Markdown => {
            let s = &sheets[0];
            let cols = s.rows.iter().map(|r| r.len()).max().unwrap_or(0).max(1);
            let esc = |t: String| t.replace('|', "\\|").replace('\n', " ");
            let line = |r: &[Cell]| format!("| {} |", (0..cols).map(|i| esc(r.get(i).map(Cell::text).unwrap_or_default())).collect::<Vec<_>>().join(" | "));
            let mut out = String::new();
            let mut rows = s.rows.iter();
            // Markdown has to have a header: the first row is it.
            if let Some(h) = rows.next() {
                out += &line(h);
                out += &format!("\n|{}\n", " --- |".repeat(cols));
            }
            for r in rows {
                out += &line(r);
                out.push('\n');
            }
            std::fs::write(path, out).map_err(|e| e.to_string())
        }
        Format::Html => {
            let mut out = String::from("<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><style>table{border-collapse:collapse}td,th{border:1px solid #999;padding:2px 6px}</style></head><body>\n");
            for s in sheets {
                let header = s.header().is_some();
                out += &format!("<h2>{}</h2>\n<table>\n", xml_esc(&s.name));
                for (i, r) in s.rows.iter().enumerate() {
                    let tag = if i == 0 && header { "th" } else { "td" };
                    out += "<tr>";
                    for c in r {
                        out += &format!("<{tag}>{}</{tag}>", xml_esc(&c.text()));
                    }
                    out += "</tr>\n";
                }
                out += "</table>\n";
            }
            out += "</body></html>\n";
            std::fs::write(path, out).map_err(|e| e.to_string())
        }
    }
}

fn json_of(c: &Cell) -> serde_json::Value {
    match c.typed() {
        Cell::Empty => serde_json::Value::Null,
        Cell::Num(n) if n.fract() == 0.0 && n.abs() < 9e15 => serde_json::Value::from(n as i64),
        Cell::Num(n) => serde_json::Value::from(n),
        Cell::Bool(b) => serde_json::Value::Bool(b),
        Cell::Text(t) | Cell::Plain(t) => serde_json::Value::String(t),
        Cell::Date(t) => serde_json::Value::String(date_text(&t)),
    }
}

/// A sheet name Excel takes: ≤ 31 characters, none of []:*?/\, unique.
fn sheet_name(name: &str, used: &mut Vec<String>) -> String {
    let base: String = name.chars().filter(|c| !"[]:*?/\\".contains(*c)).take(28).collect();
    let base = if base.trim().is_empty() { "Sheet".to_string() } else { base };
    let mut n = base.clone();
    let mut k = 2;
    while used.iter().any(|u| u.eq_ignore_ascii_case(&n)) {
        n = format!("{base} {k}");
        k += 1;
    }
    used.push(n.clone());
    n
}

fn xml_esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// An OpenDocument spreadsheet: a zip of its content and manifest.
fn write_ods(path: &Path, sheets: &[Sheet]) -> Result<(), String> {
    let mut content = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<office:document-content xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\" xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\" xmlns:style=\"urn:oasis:names:tc:opendocument:xmlns:style:1.0\" xmlns:number=\"urn:oasis:names:tc:opendocument:xmlns:datastyle:1.0\" office:version=\"1.2\">",
    );
    // Dates shown as 2024-03-01 (and 13:05:00).
    let ymd = "<number:year number:style=\"long\"/><number:text>-</number:text><number:month number:style=\"long\"/><number:text>-</number:text><number:day number:style=\"long\"/>";
    let hms = "<number:text> </number:text><number:hours number:style=\"long\"/><number:text>:</number:text><number:minutes number:style=\"long\"/><number:text>:</number:text><number:seconds number:style=\"long\"/>";
    content += &format!(
        "<office:automatic-styles><number:date-style style:name=\"N1\">{ymd}</number:date-style><number:date-style style:name=\"N2\">{ymd}{hms}</number:date-style><style:style style:name=\"day\" style:family=\"table-cell\" style:data-style-name=\"N1\"/><style:style style:name=\"moment\" style:family=\"table-cell\" style:data-style-name=\"N2\"/></office:automatic-styles><office:body><office:spreadsheet>"
    );
    let mut used = Vec::new();
    for s in sheets {
        content += &format!("<table:table table:name=\"{}\">", xml_esc(&sheet_name(&s.name, &mut used)));
        for r in &s.rows {
            content += "<table:table-row>";
            for c in r {
                content += &match c.typed() {
                    Cell::Empty => "<table:table-cell/>".to_string(),
                    Cell::Num(n) => format!("<table:table-cell office:value-type=\"float\" office:value=\"{n}\"><text:p>{}</text:p></table:table-cell>", num_text(n)),
                    Cell::Bool(b) => format!("<table:table-cell office:value-type=\"boolean\" office:boolean-value=\"{b}\"><text:p>{}</text:p></table:table-cell>", if b { "TRUE" } else { "FALSE" }),
                    Cell::Text(t) | Cell::Plain(t) => format!("<table:table-cell office:value-type=\"string\"><text:p>{}</text:p></table:table-cell>", xml_esc(&t)),
                    Cell::Date(t) => format!(
                        "<table:table-cell table:style-name=\"{}\" office:value-type=\"date\" office:date-value=\"{}\"><text:p>{}</text:p></table:table-cell>",
                        if t.time() == chrono::NaiveTime::MIN { "day" } else { "moment" },
                        t.format("%Y-%m-%dT%H:%M:%S"),
                        date_text(&t)
                    ),
                };
            }
            content += "</table:table-row>";
        }
        content += "</table:table>";
    }
    content += "</office:spreadsheet></office:body></office:document-content>\n";
    let manifest = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<manifest:manifest xmlns:manifest=\"urn:oasis:names:tc:opendocument:xmlns:manifest:1.0\" manifest:version=\"1.2\"><manifest:file-entry manifest:full-path=\"/\" manifest:media-type=\"application/vnd.oasis.opendocument.spreadsheet\"/><manifest:file-entry manifest:full-path=\"content.xml\" manifest:media-type=\"text/xml\"/></manifest:manifest>\n";
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut z = zip::ZipWriter::new(file);
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut put = |name: &str, data: &[u8], o| -> Result<(), String> {
        z.start_file(name, o).map_err(|e| e.to_string())?;
        z.write_all(data).map_err(|e| e.to_string())
    };
    // The mimetype first and uncompressed, as the format wants.
    put("mimetype", b"application/vnd.oasis.opendocument.spreadsheet", stored)?;
    put("content.xml", content.as_bytes(), deflated)?;
    put("META-INF/manifest.xml", manifest.as_bytes(), deflated)?;
    z.finish().map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------- the work

/// Its first sheet's column names (none: no header row).
pub fn header_of(path: &Path) -> Result<Option<Vec<String>>, String> {
    let sheets = read_some(path, Delim::Auto, Some(2))?;
    Ok(sheets.first().and_then(|s| s.header()))
}

/// Whether these tables can be glued: each a simple table, the same column
/// names (case aside). Else why not.
pub fn mergeable(paths: &[PathBuf], delim: Delim) -> Result<(), String> {
    let mut first: Option<(String, Vec<String>)> = None;
    for p in paths {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let sheets = read_some(p, delim, Some(2))?;
        let h = sheets.first().and_then(|s| s.header()).ok_or_else(|| format!("{name}: {}", crate::i18n::tr("no header row")))?;
        let mut key: Vec<String> = h.iter().map(|x| x.to_lowercase()).collect();
        key.sort();
        match &first {
            None => first = Some((name, key)),
            Some((n0, k0)) if *k0 != key => return Err(format!("{name} ≠ {n0}: {}", crate::i18n::tr("different columns"))),
            _ => {}
        }
    }
    Ok(())
}

/// One table out of several with the same columns: in the first one's
/// order, the others' matched by name.
fn glue(tables: Vec<Sheet>) -> Result<Sheet, String> {
    let mut it = tables.into_iter();
    let mut out = it.next().ok_or("nothing")?;
    let head = out.header().ok_or("no header")?;
    let lower: Vec<String> = head.iter().map(|h| h.to_lowercase()).collect();
    for t in it {
        let h = t.header().ok_or("no header")?;
        let map: Vec<Option<usize>> = lower.iter().map(|k| h.iter().position(|x| x.to_lowercase() == *k)).collect();
        if map.iter().any(|m| m.is_none()) || h.len() != lower.len() {
            return Err("different columns".into());
        }
        for r in t.rows.into_iter().skip(1) {
            out.rows.push(map.iter().map(|m| m.and_then(|i| r.get(i).cloned()).unwrap_or(Cell::Empty)).collect());
        }
    }
    Ok(out)
}

/// Cut into parts of `n` data rows, the header on each (if it has one).
fn cut(s: Sheet, n: usize) -> Vec<Sheet> {
    let head = s.header().map(|_| s.rows[0].clone());
    let skip = usize::from(head.is_some());
    if s.rows.len() <= n + skip {
        return vec![s];
    }
    s.rows[skip..]
        .chunks(n)
        .map(|c| {
            let mut rows: Vec<Vec<Cell>> = head.iter().cloned().collect();
            rows.extend(c.iter().cloned());
            Sheet { name: s.name.clone(), rows }
        })
        .collect()
}

/// Convert tables: each by itself, or glued into one; cut if asked. The
/// files written, next to the (first) original.
pub fn convert(paths: &[PathBuf], f: Format, o: &Opts, job: &crate::ops::Job) -> Result<Vec<PathBuf>, String> {
    let mut written = Vec::new();
    // What goes into one output: (folder, name stem, sheets).
    let mut outputs: Vec<(PathBuf, String, Vec<Sheet>)> = Vec::new();
    let stem_of = |p: &Path| crate::ops::split_ext(&p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).0.to_string();
    let dir_of = |p: &Path| p.parent().unwrap_or(Path::new("/")).to_path_buf();
    if o.merge && paths.len() > 1 {
        let mut firsts = Vec::new();
        for p in paths {
            job.now_on(p);
            let sheets = read(p, o.delim_in)?;
            firsts.push(sheets.into_iter().next().ok_or("empty")?);
        }
        let mut glued = glue(firsts)?;
        // (Columns in the first table's order: as the preview had them.)
        if paths.iter().any(|p| is_csv(&p.to_string_lossy())) {
            shape(&mut glued, &o.cols);
        }
        outputs.push((dir_of(&paths[0]), format!("{} ({})", stem_of(&paths[0]), crate::i18n::tr("merged")), vec![glued]));
    } else {
        for p in paths {
            job.now_on(p);
            let mut sheets = read(p, o.delim_in)?;
            if is_csv(&p.to_string_lossy()) {
                sheets.iter_mut().for_each(|s| shape(s, &o.cols));
            }
            outputs.push((dir_of(p), stem_of(p), sheets));
        }
    }
    for (dir, stem, sheets) in outputs {
        if job.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        // Cut: a file per part (each sheet's parts, in a workbook too).
        let several_sheets = sheets.len() > 1;
        let mut files: Vec<(String, Vec<Sheet>)> = Vec::new();
        if f.workbook() && o.split.is_none() {
            files.push((stem.clone(), sheets));
        } else {
            for s in sheets {
                let sheet_tag = if several_sheets { format!(" - {}", s.name.replace('/', "∕")) } else { String::new() };
                let parts = match o.split {
                    Some(n) => cut(s, n),
                    None => vec![s],
                };
                let many = parts.len() > 1;
                let total = parts.len();
                for (k, part) in parts.into_iter().enumerate() {
                    let tag = if many { o.numbering.tag(k + 1, total) } else { String::new() };
                    files.push((format!("{stem}{sheet_tag}{tag}"), vec![part]));
                }
            }
        }
        for (name, sheets) in files {
            let out = crate::ops::unique_path(&dir, &format!("{name}{}", f.ext()));
            if let Err(e) = write(&out, &sheets, f, o.delim_out) {
                let _ = std::fs::remove_file(&out);
                return Err(e);
            }
            written.push(out);
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Cell {
        Cell::Text(s.into())
    }

    #[test]
    fn separators() {
        assert_eq!(detect("a;b;c\n1;2,5;3\n"), Delim::Semicolon);
        assert_eq!(detect("a,b\n\"x;y\",2\n"), Delim::Comma);
        assert_eq!(detect("a\tb\tc\n1\t2\t3"), Delim::Tab);
    }

    #[test]
    fn numbers() {
        assert_eq!(t("12").typed(), Cell::Num(12.0));
        assert_eq!(t("-3.5").typed(), Cell::Num(-3.5));
        assert_eq!(t("007").typed(), t("007"));
        assert_eq!(t("0.5").typed(), Cell::Num(0.5));
        assert_eq!(t("1e5").typed(), t("1e5"));
    }

    #[test]
    fn column_types() {
        assert_eq!(parse_num("1 234,5"), Some(1234.5));
        assert_eq!(parse_num("1.234.567"), Some(1234567.0));
        assert_eq!(parse_num("1,234.5"), Some(1234.5));
        assert_eq!(parse_num("+3"), Some(3.0));
        assert_eq!(parse_num("abc"), None);
        assert_eq!(parse_num("nan"), None);
        let day = |y, m, d| chrono::NaiveDate::from_ymd_opt(y, m, d).unwrap().and_time(chrono::NaiveTime::MIN);
        assert_eq!(parse_date("2024-03-01"), Some(day(2024, 3, 1)));
        assert_eq!(parse_date("01.03.2024"), Some(day(2024, 3, 1)));
        assert_eq!(parse_date("01.03.24"), Some(day(2024, 3, 1)));
        assert_eq!(parse_date("12/31/2024"), Some(day(2024, 12, 31)));
        assert_eq!(parse_date("2024-03-01 13:05").map(|t| t.format("%H:%M").to_string()), Some("13:05".into()));
        assert_eq!(parse_date("soon"), None);
        // The header stays; "007" kept as text, numbers and dates read.
        let mut s = Sheet { name: "s".into(), rows: vec![vec![t("Id"), t("Sum"), t("When")], vec![t("007"), t("1 000,5"), t("01.03.2024")], vec![t("8"), t("x"), t("")]] };
        shape(&mut s, &[ColType::Text, ColType::Number, ColType::Date]);
        assert_eq!(s.rows[0][0], t("Id"));
        assert_eq!(s.rows[1], vec![Cell::Plain("007".into()), Cell::Num(1000.5), Cell::Date(day(2024, 3, 1))]);
        assert_eq!(s.rows[2][1], t("x"));
        assert!(!ColType::Number.fits(&t("x")) && ColType::Number.fits(&Cell::Empty));
    }

    #[test]
    fn glue_and_cut() {
        let a = Sheet { name: "a".into(), rows: vec![vec![t("Name"), t("Age")], vec![t("Ann"), t("30")]] };
        let b = Sheet { name: "b".into(), rows: vec![vec![t("age"), t("NAME")], vec![t("40"), t("Bob")], vec![t("50"), t("Cid")]] };
        let g = glue(vec![a, b]).unwrap();
        assert_eq!(g.rows, vec![vec![t("Name"), t("Age")], vec![t("Ann"), t("30")], vec![t("Bob"), t("40")], vec![t("Cid"), t("50")]]);
        let parts = cut(g, 2);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1].rows, vec![vec![t("Name"), t("Age")], vec![t("Cid"), t("50")]]);
        // No header (a number on top): cut all the same.
        let raw = Sheet { name: "r".into(), rows: vec![vec![t("1")], vec![t("2")], vec![t("3")]] };
        let parts = cut(raw, 2);
        assert_eq!(parts.iter().map(|p| p.rows.len()).collect::<Vec<_>>(), vec![2, 1]);
        assert_eq!(Numbering::Underscore.tag(3, 12), "_03");
        assert_eq!(Numbering::Paren.tag(1, 3), " (1)");
    }
}
