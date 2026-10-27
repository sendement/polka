//! The status bar's figures past the item count: the disk's space used and
//! free, and how many folders, pictures, videos, archives and documents
//! the folder holds (each as set in View → Status bar).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::vfs::Entry;

/// Which figures show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shown {
    pub space: bool,
    pub folders: bool,
    pub pictures: bool,
    pub videos: bool,
    pub archives: bool,
    pub pdfs: bool,
    pub documents: bool,
    pub sheets: bool,
    pub slides: bool,
}

impl Default for Shown {
    fn default() -> Self {
        Shown { space: true, folders: true, pictures: true, videos: true, archives: true, pdfs: true, documents: true, sheets: true, slides: true }
    }
}

/// The figures, in the bar's order (and the menu's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Figure {
    Space,
    Folders,
    Pictures,
    Videos,
    Archives,
    Pdfs,
    Documents,
    Sheets,
    Slides,
}

pub const FIGURES: [Figure; 9] = [Figure::Space, Figure::Folders, Figure::Pictures, Figure::Videos, Figure::Archives, Figure::Pdfs, Figure::Documents, Figure::Sheets, Figure::Slides];

impl Shown {
    pub fn get(&self, f: Figure) -> bool {
        match f {
            Figure::Space => self.space,
            Figure::Folders => self.folders,
            Figure::Pictures => self.pictures,
            Figure::Videos => self.videos,
            Figure::Archives => self.archives,
            Figure::Pdfs => self.pdfs,
            Figure::Documents => self.documents,
            Figure::Sheets => self.sheets,
            Figure::Slides => self.slides,
        }
    }

    pub fn flip(&mut self, f: Figure) {
        let v = match f {
            Figure::Space => &mut self.space,
            Figure::Folders => &mut self.folders,
            Figure::Pictures => &mut self.pictures,
            Figure::Videos => &mut self.videos,
            Figure::Archives => &mut self.archives,
            Figure::Pdfs => &mut self.pdfs,
            Figure::Documents => &mut self.documents,
            Figure::Sheets => &mut self.sheets,
            Figure::Slides => &mut self.slides,
        };
        *v = !*v;
    }
}

impl Figure {
    /// Its name in the menu.
    pub fn label(self) -> &'static str {
        use crate::i18n::tr;
        match self {
            Figure::Space => tr("Disk space"),
            Figure::Folders => tr("Folders"),
            Figure::Pictures => tr("Pictures"),
            Figure::Videos => tr("Video"),
            Figure::Archives => tr("Archives"),
            Figure::Pdfs => "PDF",
            Figure::Documents => tr("Documents"),
            Figure::Sheets => tr("Spreadsheets"),
            Figure::Slides => tr("Presentations"),
        }
    }

    /// Theme icons for it, the first found.
    pub fn icons(self) -> &'static [&'static str] {
        match self {
            // (Shown as a bar instead.)
            Figure::Space => &[],
            Figure::Folders => &["folder"],
            Figure::Pictures => &["image-x-generic", "folder-pictures"],
            Figure::Videos => &["video-x-generic", "folder-videos"],
            Figure::Archives => &["package-x-generic", "application-x-archive", "application-zip"],
            Figure::Pdfs => &["application-pdf", "x-office-document"],
            Figure::Documents => &["x-office-document", "application-vnd.oasis.opendocument.text", "text-x-generic"],
            Figure::Sheets => &["x-office-spreadsheet", "application-vnd.oasis.opendocument.spreadsheet"],
            Figure::Slides => &["x-office-presentation", "application-vnd.oasis.opendocument.presentation"],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub folders: usize,
    pub pictures: usize,
    pub videos: usize,
    pub archives: usize,
    pub pdfs: usize,
    pub documents: usize,
    pub sheets: usize,
    pub slides: usize,
}

impl Counts {
    pub fn of(entries: &[Entry]) -> Self {
        let mut c = Counts::default();
        for e in entries {
            if e.is_dir {
                c.folders += 1;
                continue;
            }
            let ext = e.name.rsplit_once('.').map_or("", |(_, x)| x);
            let is = |list: &[&str]| list.iter().any(|x| x.eq_ignore_ascii_case(ext));
            if is(crate::viewer::image::EXTS) {
                c.pictures += 1;
            } else if is(crate::viewer::video::EXTS) {
                c.videos += 1;
            } else if crate::vfs::is_archive_name(&e.name) {
                c.archives += 1;
            } else if let Some(k) = office_kind(ext) {
                *match k {
                    Figure::Pdfs => &mut c.pdfs,
                    Figure::Sheets => &mut c.sheets,
                    Figure::Slides => &mut c.slides,
                    _ => &mut c.documents,
                } += 1;
            }
        }
        c
    }

    pub fn get(&self, f: Figure) -> usize {
        match f {
            Figure::Folders => self.folders,
            Figure::Pictures => self.pictures,
            Figure::Videos => self.videos,
            Figure::Archives => self.archives,
            Figure::Pdfs => self.pdfs,
            Figure::Documents => self.documents,
            Figure::Sheets => self.sheets,
            Figure::Slides => self.slides,
            Figure::Space => 0,
        }
    }
}

/// Which document figure a file counts under, by its extension.
fn office_kind(ext: &str) -> Option<Figure> {
    Some(match ext.to_ascii_lowercase().as_str() {
        "pdf" => Figure::Pdfs,
        "docx" | "docm" | "dotx" | "doc" | "dot" | "odt" | "ott" | "rtf" | "wpd" | "pages" => Figure::Documents,
        "xlsx" | "xlsm" | "xlsb" | "xls" | "ods" | "ots" | "csv" | "tsv" | "numbers" => Figure::Sheets,
        "pptx" | "pptm" | "ppsx" | "potx" | "ppt" | "pps" | "odp" | "otp" | "key" => Figure::Slides,
        _ => return None,
    })
}

/// The disk's size and space used, looked at now and then off the UI
/// thread (a network mount can take its time answering).
#[derive(Default)]
pub struct Space {
    asked: Option<(PathBuf, Instant)>,
    got: Arc<Mutex<Option<(PathBuf, (u64, u64))>>>,
}

impl Space {
    /// (size, used) of the disk holding `dir`, as last found.
    pub fn of(&mut self, dir: &Path, ctx: &egui::Context) -> Option<(u64, u64)> {
        let stale = self.asked.as_ref().is_none_or(|(p, t)| p != dir || t.elapsed() > Duration::from_secs(5));
        if stale {
            self.asked = Some((dir.to_path_buf(), Instant::now()));
            let (dir, got, ctx) = (dir.to_path_buf(), self.got.clone(), ctx.clone());
            std::thread::spawn(move || {
                if let Some(s) = crate::usage::fs_space(&dir) {
                    if let Ok(mut g) = got.lock() {
                        *g = Some((dir, s));
                    }
                    ctx.request_repaint();
                }
            });
        }
        self.got.lock().ok()?.as_ref().filter(|(p, _)| p == dir).map(|(_, s)| *s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool) -> Entry {
        Entry { name: name.into(), is_dir, is_link: false, size: 0, mtime: None, mode: 0, tags: Vec::new() }
    }

    #[test]
    fn counts() {
        let es = [entry("Photos", true), entry("a.JPG", false), entry("b.heic", false), entry("c.mkv", false), entry("d.tar.gz", false), entry("e.txt", false), entry("f.png", true), entry("g.PDF", false), entry("h.docx", false), entry("i.odt", false), entry("j.csv", false), entry("k.pptx", false)];
        assert_eq!(Counts::of(&es), Counts { folders: 2, pictures: 2, videos: 1, archives: 1, pdfs: 1, documents: 2, sheets: 1, slides: 1 });
    }
}
