//! "Open with…": the programs that take the file's type (the usual one
//! first), all of them on request, and the choice remembered as the
//! system's default for the type (xdg-mime, so other programs follow it).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use freedesktop_desktop_entry::{DesktopEntry, get_languages_from_env};

use crate::app::{App, Overlay};
use crate::i18n::tr;
use crate::input::Action;

#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    /// The desktop file's id ("org.gnome.Loupe.desktop").
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub icon: Option<String>,
    pub mimes: Vec<String>,
}

pub struct OpenWithDlg {
    pub files: Vec<PathBuf>,
    pub mime: String,
    /// What the type is called ("PNG image"), for the remember row.
    pub type_name: String,
    /// Those for the type, the usual one first; or all of them.
    pub list: Vec<Program>,
    pub fitting: usize,
    pub all: bool,
    pub default: Option<String>,
    pub cursor: usize,
    pub remember: bool,
    pub scrolled: Option<usize>,
}

/// Folders with desktop files: the user's first (theirs win), then the
/// system's, Flatpak's exports too.
fn app_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(d) = dirs::data_dir() {
        v.push(d.join("applications"));
        v.push(d.join("flatpak/exports/share/applications"));
    }
    let sys = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    v.extend(sys.split(':').filter(|s| !s.is_empty()).map(|s| Path::new(s).join("applications")));
    v.push("/var/lib/flatpak/exports/share/applications".into());
    if let Some(extra) = std::env::var_os("POLKA_APPS_DIR") {
        v.insert(0, extra.into());
    }
    v
}

/// Every program a person would pick (shown ones, not Polka itself).
pub fn programs() -> Vec<Program> {
    let locales = get_languages_from_env();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for dir in app_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "desktop")).collect();
        files.sort();
        for path in files {
            let id = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if !seen.insert(id.clone()) {
                continue;
            }
            let Ok(de) = DesktopEntry::from_path(&path, Some(&locales)) else { continue };
            if de.no_display() || de.hidden() || de.exec().is_none() || id.to_lowercase().contains("polka") {
                continue;
            }
            let Some(name) = de.name(&locales).map(|n| n.to_string()) else { continue };
            let mimes = de.mime_type().map(|m| m.iter().map(|s| s.to_string()).collect()).unwrap_or_default();
            out.push(Program { id, name, icon: de.icon().map(str::to_string), path, mimes });
        }
    }
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

/// It takes this type (exactly, "image/*", or plain text for any text).
fn takes(p: &Program, mime: &str) -> bool {
    let group = mime.split('/').next().unwrap_or("");
    p.mimes.iter().any(|m| m == mime || *m == format!("{group}/*") || (group == "text" && m == "text/plain"))
}

fn host_out(args: &[&str]) -> Option<String> {
    if cfg!(test) {
        return None;
    }
    let mut c = Command::new(args[0]);
    c.args(&args[1..]);
    let out = crate::sandbox::host(c).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

/// The file's type: what the system says (it looks inside), else by name.
pub fn mime_of(path: &Path) -> String {
    host_out(&["xdg-mime", "query", "filetype", &crate::sandbox::host_path(path).to_string_lossy()])
        .or_else(|| mime_guess::from_path(path).first().map(|m| m.essence_str().to_string()))
        .unwrap_or_else(|| "application/octet-stream".into())
}

impl OpenWithDlg {
    pub fn new(files: Vec<PathBuf>) -> Self {
        let mime = mime_of(&files[0]);
        let default = host_out(&["xdg-mime", "query", "default", &mime]);
        let type_name = mime_guess::get_mime_extensions_str(&mime).and_then(|e| e.first()).map(|e| e.to_uppercase()).unwrap_or_else(|| mime.clone());
        let mut d = OpenWithDlg { files, mime, type_name, list: Vec::new(), fitting: 0, all: false, default, cursor: 0, remember: false, scrolled: None };
        d.fill();
        // Nothing takes it: all of them straight away.
        if d.fitting == 0 {
            d.all = true;
            d.fill();
        }
        d
    }

    /// The list: the fitting ones (the usual one first), then with "all"
    /// the rest.
    fn fill(&mut self) {
        let every = programs();
        let (mut fit, rest): (Vec<Program>, Vec<Program>) = every.into_iter().partition(|p| takes(p, &self.mime));
        if let Some(d) = &self.default {
            if let Some(i) = fit.iter().position(|p| p.id == *d) {
                let p = fit.remove(i);
                fit.insert(0, p);
            }
        }
        self.fitting = fit.len();
        self.list = fit;
        if self.all {
            self.list.extend(rest);
        }
        self.cursor = self.cursor.min(self.list.len().saturating_sub(1));
        self.scrolled = None;
    }
}

impl App {
    /// "Open with…" on the file shown, or the chosen ones.
    pub fn open_with_ask(&mut self) {
        let files: Vec<PathBuf> = match &self.cur_tab().viewer {
            Some(v) => vec![v.path.clone()],
            None => self.local_targets().into_iter().filter(|p| !p.is_dir()).collect(),
        };
        if files.is_empty() {
            return;
        }
        self.overlay = Some(Overlay::OpenWith(Box::new(OpenWithDlg::new(files))));
    }

    /// Open with the one under the cursor (remembered, if asked).
    pub fn open_with_go(&mut self) {
        let Some(Overlay::OpenWith(d)) = self.overlay.take() else { return };
        let Some(p) = d.list.get(d.cursor) else { return };
        if d.remember {
            let mut c = Command::new("xdg-mime");
            c.args(["default", &p.id, &d.mime]);
            let ok = crate::sandbox::host(c).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
            if !ok && !cfg!(test) {
                self.toast(format!("{}: xdg-mime", tr("Failed")), true);
            }
        }
        if let Err(e) = crate::launch::open_with(&p.path, &d.files) {
            self.toast(format!("{}: {e}", tr("Failed")), true);
        }
    }

    pub fn open_with_input(&mut self, a: Action) {
        let Some(Overlay::OpenWith(d)) = self.overlay.as_mut() else { return };
        let n = d.list.len();
        match a {
            Action::Up => d.cursor = d.cursor.saturating_sub(1),
            Action::Down => d.cursor = (d.cursor + 1).min(n.saturating_sub(1)),
            Action::LeftTrigger => d.cursor = d.cursor.saturating_sub(8),
            Action::RightTrigger => d.cursor = (d.cursor + 8).min(n.saturating_sub(1)),
            Action::Select => d.remember = !d.remember,
            Action::Actions => {
                d.all = !d.all;
                d.fill();
            }
            Action::Accept => self.open_with_go(),
            Action::Back | Action::Menu => self.overlay = None,
            _ => {}
        }
    }
}

pub enum OwEv {
    Pick(usize),
    Open(usize),
    Remember,
    All,
    Go,
    Cancel,
}

pub fn draw(ui: &mut Ui, d: &mut OpenWithDlg, icons: &mut crate::icons::Icons) -> Option<OwEv> {
    let mut ev = None;
    let v = ui.visuals().clone();
    ui.set_min_width(460.0);
    ui.label(egui::RichText::new(tr("Open with…")).strong().size(18.0));
    let what = match d.files.as_slice() {
        [one] => one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        many => crate::i18n::trn("%n items", many.len()),
    };
    ui.label(egui::RichText::new(format!("{what} · {}", d.mime)).weak());
    ui.add_space(6.0);
    let scroll_to = (d.scrolled != Some(d.cursor)).then_some(d.cursor);
    d.scrolled = Some(d.cursor);
    egui::ScrollArea::vertical().id_salt("open-with").max_height(340.0).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        if d.list.is_empty() {
            ui.label(egui::RichText::new(tr("Nothing found")).weak());
        }
        for (i, p) in d.list.iter().enumerate() {
            if d.all && i == d.fitting && i > 0 {
                ui.separator();
            }
            let (rect, r) = ui.allocate_exact_size(vec2(440.0, 36.0), Sense::click());
            if i == d.cursor {
                ui.painter().rect(rect, 6.0, crate::view::tint(&v, 0.45), Stroke::new(1.5, v.selection.bg_fill), StrokeKind::Inside);
            } else if r.hovered() {
                ui.painter().rect_filled(rect, 6.0, v.widgets.hovered.weak_bg_fill);
            }
            let ir = Rect::from_center_size(pos2(rect.min.x + 22.0, rect.center().y), Vec2::splat(24.0));
            let icon = p.icon.as_deref().and_then(|ic| {
                let path = Path::new(ic);
                if path.is_absolute() { path.exists().then(|| crate::icons::uri(path)) } else { icons.named(&[ic, "application-x-executable"], 32) }
            });
            if let Some(u) = icon {
                egui::Image::new(u).paint_at(ui, ir);
            }
            let usual = d.default.as_deref() == Some(p.id.as_str());
            let label = if usual { format!("{}  ·  {}", p.name, tr("usual")) } else { p.name.clone() };
            let g = crate::view::elided(ui, &label, FontId::proportional(15.0), v.text_color(), rect.width() - 52.0, 1);
            ui.painter().galley(pos2(rect.min.x + 42.0, rect.center().y - g.size().y / 2.0), g, Color32::WHITE);
            if scroll_to == Some(i) {
                r.scroll_to_me(None);
            }
            if r.double_clicked() {
                ev = Some(OwEv::Open(i));
            } else if r.clicked() {
                ev = Some(OwEv::Pick(i));
            }
        }
    });
    ui.add_space(6.0);
    // Remember it; show every program.
    for (on, label, e) in [(d.remember, format!("{} ({})", tr("Always open this type with it"), d.type_name), OwEv::Remember), (d.all, tr("All programs").to_string(), OwEv::All)] {
        let (rect, r) = ui.allocate_exact_size(vec2(440.0, 30.0), Sense::click());
        let b = Rect::from_center_size(pos2(rect.min.x + 14.0, rect.center().y), Vec2::splat(18.0));
        ui.painter().rect_stroke(b, 4.0, Stroke::new(1.5, v.strong_text_color()), StrokeKind::Inside);
        if on {
            ui.painter().text(b.center(), Align2::CENTER_CENTER, "✔", FontId::proportional(14.0), v.strong_text_color());
        }
        ui.painter().text(pos2(rect.min.x + 32.0, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(14.0), v.text_color());
        if r.clicked() {
            ev = Some(e);
        }
    }
    ui.add_space(4.0);
    // (In a row: laid out right to left in the window itself, it would
    // take all the height there is.)
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(tr("Cancel")).clicked() {
                ev = Some(OwEv::Cancel);
            }
            if ui.add_enabled(!d.list.is_empty(), egui::Button::new(egui::RichText::new(tr("Open")).strong())).clicked() {
                ev = Some(OwEv::Go);
            }
        });
    });
    ev
}

pub fn event(app: &mut App, ev: OwEv) {
    let Some(Overlay::OpenWith(d)) = app.overlay.as_mut() else { return };
    match ev {
        OwEv::Pick(i) => d.cursor = i,
        OwEv::Open(i) => {
            d.cursor = i;
            app.open_with_go();
        }
        OwEv::Remember => d.remember = !d.remember,
        OwEv::All => {
            d.all = !d.all;
            d.fill();
        }
        OwEv::Go => app.open_with_go(),
        OwEv::Cancel => app.overlay = None,
    }
}
