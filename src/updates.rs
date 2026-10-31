//! Polka's own versions: the releases on GitHub, newer and older, any of
//! them installed (an update, or going back) the way this copy came, in a
//! terminal window like any package. A Flatpak goes through Flatpak and its
//! repository (`flatpak update`, `--commit` for an older one, which the
//! repository keeps); a version from before the repository kept them comes
//! from its bundle instead (said first: Flatpak then no longer follows the
//! repository). The Arch package is downloaded and installed with pacman.
//! Versions before this section came don't have it: also said first.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{Align2, FontId, Rect, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::app::{App, Overlay};
use crate::i18n::tr;
use crate::input::Action;

/// The first version with this section.
pub const SINCE: Version = (0, 44, 0);

pub type Version = (u32, u32, u32);

const RELEASES: &str = "https://api.github.com/repos/sendement/polka/releases?per_page=100";
/// Polka's own Flatpak repository (its versions.json: each version's commit).
const SITE: &str = "https://sendement.github.io/polka";
const APP_ID: &str = "io.github.sendement.Polka";

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: Version,
    pub tag: String,
    /// Published (YYYY-MM-DD).
    pub date: String,
    pub notes: String,
    /// Its page on GitHub.
    pub page: String,
    /// Its file for this copy's way of installing (if it has one).
    pub asset: Option<Asset>,
    /// Its commit in the Flatpak repository (when it's kept there).
    pub commit: Option<String>,
}

/// Where this Flatpak comes from: the repository's remote (by name, if one
/// points at it) and whether it was installed from there.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Flat {
    pub remote: Option<String>,
    pub from_repo: bool,
}

/// What installing a version takes.
#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    /// A command, as it is.
    Run(String),
    /// Its file downloaded first, then installed.
    File,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// How this copy is installed (and so how another version is).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    Flatpak,
    Pacman,
    /// Neither: the release page is opened instead.
    Manual,
}

impl Way {
    pub fn here() -> Way {
        if let Ok(w) = std::env::var("POLKA_INSTALL_WAY") {
            return match w.as_str() {
                "flatpak" => Way::Flatpak,
                "pacman" => Way::Pacman,
                _ => Way::Manual,
            };
        }
        if crate::sandbox::flatpak() {
            Way::Flatpak
        } else if !crate::terminal::installed(&["pacman"]).is_empty() {
            Way::Pacman
        } else {
            Way::Manual
        }
    }

    fn name(self) -> &'static str {
        match self {
            Way::Flatpak => "Flatpak",
            Way::Pacman => tr("Arch package"),
            Way::Manual => tr("installed by hand"),
        }
    }

    /// Its file among a release's.
    fn fits(self, name: &str) -> bool {
        match self {
            Way::Flatpak => name.ends_with(".flatpak"),
            Way::Pacman => name.ends_with(".pkg.tar.zst"),
            Way::Manual => false,
        }
    }
}

pub fn parse_version(s: &str) -> Option<Version> {
    let mut it = s.trim().trim_start_matches('v').split('.').map(|p| p.parse::<u32>().ok());
    let v = (it.next()??, it.next()??, it.next()??);
    it.next().is_none().then_some(v)
}

pub fn current() -> Version {
    parse_version(env!("CARGO_PKG_VERSION")).unwrap_or_default()
}

pub fn shown(v: Version) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
}

/// The releases, newest first (drafts and pre-releases aside).
pub fn parse(json: &str, way: Way) -> Result<Vec<Release>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let list = v.as_array().ok_or_else(|| v.get("message").and_then(|m| m.as_str()).unwrap_or("not a list").to_string())?;
    let mut out: Vec<Release> = list
        .iter()
        .filter(|r| !r["draft"].as_bool().unwrap_or(false) && !r["prerelease"].as_bool().unwrap_or(false))
        .filter_map(|r| {
            let tag = r["tag_name"].as_str()?.to_string();
            let version = parse_version(&tag)?;
            let asset = r["assets"].as_array().into_iter().flatten().find(|a| a["name"].as_str().is_some_and(|n| way.fits(n))).map(|a| Asset {
                name: a["name"].as_str().unwrap_or_default().to_string(),
                url: a["browser_download_url"].as_str().unwrap_or_default().to_string(),
                size: a["size"].as_u64().unwrap_or(0),
            });
            Some(Release {
                version,
                tag,
                date: r["published_at"].as_str().unwrap_or_default().chars().take(10).collect(),
                notes: r["body"].as_str().unwrap_or_default().replace("\r\n", "\n").trim().to_string(),
                page: r["html_url"].as_str().unwrap_or_default().to_string(),
                asset,
                commit: None,
            })
        })
        .collect();
    out.sort_by(|a, b| b.version.cmp(&a.version));
    Ok(out)
}

fn site() -> String {
    std::env::var("POLKA_FLATPAK_SITE").unwrap_or_else(|_| SITE.into())
}

fn get(url: &str) -> Result<String, String> {
    let out = std::process::Command::new("curl")
        .args(["-fsSL", "--max-time", "20", "-H", "Accept: application/vnd.github+json", "-A", concat!("Polka/", env!("CARGO_PKG_VERSION"))])
        .arg(url)
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() { tr("No connection").to_string() } else { err });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The repository's versions.json: [{"version": "0.44.0", "commit": "…"}].
pub fn parse_commits(json: &str) -> Vec<(Version, String)> {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    v.as_array().into_iter().flatten().filter_map(|e| Some((parse_version(e["version"].as_str()?)?, e["commit"].as_str()?.to_string()))).collect()
}

/// Asked of the host's flatpak.
fn flatpak_says(args: &[&str]) -> String {
    let mut c = std::process::Command::new("flatpak");
    c.args(args);
    crate::sandbox::host(c).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

/// Where this Flatpak comes from.
fn flat() -> Flat {
    let origin = flatpak_says(&["info", "--show-origin", APP_ID]).trim().to_string();
    let site = site();
    let remote = flatpak_says(&["remotes", "--columns=name,url"]).lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let (name, url) = (it.next()?, it.next()?);
        url.trim_end_matches('/').starts_with(&site).then(|| name.to_string())
    });
    Flat { from_repo: remote.as_deref().is_some_and(|r| r == origin), remote }
}

/// From GitHub (or a stand-in, for tests: POLKA_RELEASES_URL); for a
/// Flatpak, with each version's commit and where it comes from.
fn fetch(way: Way) -> Result<(Vec<Release>, Option<Flat>), String> {
    let url = std::env::var("POLKA_RELEASES_URL").unwrap_or_else(|_| RELEASES.into());
    let mut list = parse(&get(&url)?, way)?;
    if way != Way::Flatpak {
        return Ok((list, None));
    }
    // (No versions.json yet: every version comes from its file.)
    let commits = get(&format!("{}/versions.json", site())).map(|j| parse_commits(&j)).unwrap_or_default();
    for r in &mut list {
        r.commit = commits.iter().find(|c| c.0 == r.version).map(|c| c.1.clone());
    }
    Ok((list, Some(flat())))
}

/// How a version installs: a Flatpak through Flatpak (from the repository,
/// first put back there if it came from elsewhere), else from its file.
pub fn plan(way: Way, r: &Release, newest: bool, flat: Option<&Flat>) -> Plan {
    let Some(f) = flat.filter(|_| way == Way::Flatpak) else { return Plan::File };
    let scope = flatpak_scope();
    if !newest && r.commit.is_none() {
        return Plan::File;
    }
    let back = if f.from_repo {
        None
    } else {
        // (Its remote if there is one, else the repository's flatpakref.)
        Some(match &f.remote {
            Some(name) => format!("flatpak install {scope} --reinstall {} {APP_ID}//stable", crate::packages::quote(name)),
            None => format!("flatpak install {scope} --reinstall {}", crate::packages::quote(&format!("{}/polka.flatpakref", site()))),
        })
    };
    let to = match (&r.commit, newest) {
        (_, true) if back.is_some() => None,
        (_, true) => Some(format!("flatpak update {scope} {APP_ID}")),
        (Some(c), false) => Some(format!("flatpak update {scope} --commit={c} {APP_ID}")),
        (None, false) => None,
    };
    Plan::Run([back, to].into_iter().flatten().collect::<Vec<_>>().join(" && "))
}

/// Where downloaded versions wait to be installed.
fn download_dir() -> Option<PathBuf> {
    let d = if cfg!(test) { crate::vfs::temp_root().join("updates") } else { dirs::cache_dir()?.join("polka/updates") };
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

/// Fetched next to where it ends (a partial file isn't taken for it).
fn download(a: &Asset, to: &Path) -> Result<(), String> {
    let part = to.with_extension("part");
    let out = std::process::Command::new("curl").args(["-fsSL", "--max-time", "900", "-o"]).arg(&part).arg(&a.url).output().map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&part);
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    std::fs::rename(&part, to).map_err(|e| e.to_string())
}

/// A Flatpak installed for the user, or for everyone.
fn flatpak_scope() -> &'static str {
    let info = std::fs::read_to_string("/.flatpak-info").unwrap_or_default();
    if info.lines().any(|l| l.starts_with("app-path=") && l.contains("/.local/share/flatpak/")) { "--user" } else { "--system" }
}

/// How the downloaded file installs (over this version, either way).
pub fn install_cmd(way: Way, file: &Path) -> Option<String> {
    let p = crate::packages::quote(&crate::sandbox::host_path(file).to_string_lossy());
    match way {
        Way::Flatpak => Some(format!("flatpak install {} --reinstall {p}", flatpak_scope())),
        Way::Pacman => crate::packages::command(crate::packages::Kind::Pacman, file).map(|c| c.0),
        Way::Manual => None,
    }
}

// ---------------------------------------------------------------- the dialog

pub enum Load {
    Loading,
    Failed(String),
    Ready(Vec<Release>),
}

enum Msg {
    Listed(Result<(Vec<Release>, Option<Flat>), String>),
    Downloaded(Version, Result<PathBuf, String>),
}

pub struct VersionsDlg {
    pub way: Way,
    pub load: Load,
    /// A Flatpak's origin (once listed).
    pub flat: Option<Flat>,
    pub cursor: usize,
    /// A version without this section, asked about once already (the
    /// second press installs it).
    pub armed: Option<Version>,
    /// Being downloaded: the version, its file (growing), its size.
    pub fetching: Option<(Version, PathBuf, u64)>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

impl VersionsDlg {
    fn releases(&self) -> &[Release] {
        match &self.load {
            Load::Ready(v) => v,
            _ => &[],
        }
    }

    pub fn plan(&self, r: &Release) -> Plan {
        let newest = self.releases().first().is_some_and(|n| n.version == r.version);
        plan(self.way, r, newest, self.flat.as_ref())
    }

    /// What to know before installing it (it's asked again then): no way
    /// back from inside it, or Flatpak no longer following the repository.
    pub fn warning(&self, r: &Release) -> Option<&'static str> {
        let leaves = self.way == Way::Flatpak && self.plan(r) == Plan::File;
        match (r.version < SINCE, leaves) {
            (true, true) => Some(tr("This version has no versions section and isn't kept in the repository: it comes from its file, and coming back means installing a release from GitHub by hand")),
            (true, false) => Some(tr("This version has no versions section: to come back, you'd install a release from GitHub by hand")),
            (false, true) => Some(tr("Not kept in the repository: it comes from its file, and flatpak update then no longer follows the repository")),
            (false, false) => None,
        }
    }
}

pub enum VEv {
    Pick(usize),
    Install(usize),
}

impl App {
    pub fn versions_open(&mut self) {
        let way = Way::here();
        let (tx, rx) = channel();
        let t = tx.clone();
        std::thread::spawn(move || {
            let _ = t.send(Msg::Listed(fetch(way)));
        });
        self.overlay = Some(Overlay::Versions(Box::new(VersionsDlg { way, load: Load::Loading, flat: None, cursor: 0, armed: None, fetching: None, tx, rx })));
    }

    /// What the other threads found (each frame).
    pub fn versions_poll(&mut self) {
        let Some(Overlay::Versions(d)) = self.overlay.as_mut() else { return };
        let mut ready = None;
        while let Ok(m) = d.rx.try_recv() {
            match m {
                Msg::Listed(Ok((list, flat))) => {
                    // On the installed one (else the newest).
                    d.cursor = list.iter().position(|r| r.version == current()).unwrap_or(0);
                    d.load = Load::Ready(list);
                    d.flat = flat;
                }
                Msg::Listed(Err(e)) => d.load = Load::Failed(e),
                Msg::Downloaded(v, r) => {
                    d.fetching = None;
                    ready = Some((v, r));
                }
            }
        }
        let busy = matches!(d.load, Load::Loading) || d.fetching.is_some();
        let way = d.way;
        if let Some((v, r)) = ready {
            match r.map(|f| (install_cmd(way, &f), f)) {
                Ok((Some(cmd), f)) => {
                    self.overlay = None;
                    let title = format!("{} {} ({})", tr("Install Polka"), shown(v), way.name());
                    let dir = f.parent().map(|d| d.to_path_buf()).unwrap_or_else(|| "/".into());
                    self.open_term_window(title, &dir, &cmd);
                    self.toast(tr("Restart Polka once it's installed"), false);
                }
                Ok((None, _)) => self.toast(tr("Nothing here installs it"), true),
                Err(e) => self.toast(format!("{}: {e}", tr("Failed")), true),
            }
        }
        if busy {
            self.ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    pub fn versions_input(&mut self, a: Action) {
        let Some(Overlay::Versions(d)) = self.overlay.as_mut() else { return };
        let n = d.releases().len();
        match a {
            Action::Up | Action::Down if n > 0 => {
                let step = if a == Action::Up { -1 } else { 1 };
                d.cursor = (d.cursor as isize + step).clamp(0, n as isize - 1) as usize;
                d.armed = None;
                self.ctx.data_mut(|m| m.insert_temp(egui::Id::new("settings-reveal"), true));
            }
            Action::Accept if n > 0 => {
                let i = d.cursor;
                self.versions_install(i);
            }
            Action::Back => self.overlay = None,
            _ => {}
        }
    }

    /// Downloaded, then installed (one without this section: asked first;
    /// with no way here, its page opened).
    pub fn versions_install(&mut self, i: usize) {
        let Some(Overlay::Versions(d)) = self.overlay.as_mut() else { return };
        let Some(r) = d.releases().get(i).cloned() else { return };
        d.cursor = i;
        if d.fetching.is_some() {
            return;
        }
        let plan = d.plan(&r);
        // Nothing to install it with, or no file for it: its page.
        let asset = match (d.way, &r.asset, &plan) {
            (Way::Manual, ..) | (_, None, Plan::File) => {
                if let Err(e) = crate::launch::open(Path::new(&r.page)) {
                    self.toast(format!("{}: {e}", tr("Failed")), true);
                }
                return;
            }
            (_, a, _) => a.clone(),
        };
        // No way back from inside, or Flatpak leaving the repository: asked
        // once more.
        if d.warning(&r).is_some() && d.armed != Some(r.version) {
            d.armed = Some(r.version);
            return;
        }
        d.armed = None;
        let v = r.version;
        if let Plan::Run(cmd) = plan {
            let way = d.way;
            self.overlay = None;
            let title = format!("{} {} ({})", tr("Install Polka"), shown(v), way.name());
            let home = dirs::home_dir().unwrap_or_else(|| "/".into());
            self.open_term_window(title, &home, &cmd);
            return self.toast(tr("Restart Polka once it's installed"), false);
        }
        let Some(asset) = asset else { return };
        let Some(dir) = download_dir() else { return self.toast(tr("Failed"), true) };
        let file = dir.join(&asset.name);
        let tx = d.tx.clone();
        // (Already here, whole: used as it is.)
        if std::fs::metadata(&file).is_ok_and(|m| asset.size > 0 && m.len() == asset.size) {
            let _ = tx.send(Msg::Downloaded(v, Ok(file)));
            return;
        }
        d.fetching = Some((v, file.with_extension("part"), asset.size));
        std::thread::spawn(move || {
            let r = download(&asset, &file).map(|_| file);
            let _ = tx.send(Msg::Downloaded(v, r));
        });
    }
}

pub fn event(app: &mut App, ev: VEv) {
    match ev {
        VEv::Pick(i) => {
            if let Some(Overlay::Versions(d)) = app.overlay.as_mut() {
                if d.cursor != i {
                    d.armed = None;
                }
                d.cursor = i;
            }
        }
        VEv::Install(i) => app.versions_install(i),
    }
}

pub fn draw(ui: &mut Ui, d: &VersionsDlg) -> Option<VEv> {
    let mut ev = None;
    let v = ui.visuals().clone();
    let w = crate::view::dialog_w();
    ui.set_width(w);
    ui.label(egui::RichText::new(tr("Polka versions")).strong().size(18.0));
    let mut here = format!("{}: {} · {}", tr("Installed"), shown(current()), d.way.name());
    if let Some(f) = &d.flat {
        here += &format!(" · {}", if f.from_repo { tr("from the repository") } else { tr("not from the repository") });
    }
    ui.label(egui::RichText::new(here).color(v.weak_text_color()));
    ui.add_space(8.0);
    let list = match &d.load {
        Load::Loading => {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(tr("Looking for versions…"));
            });
            return None;
        }
        Load::Failed(e) => {
            ui.label(egui::RichText::new(format!("{}: {e}", tr("Couldn't get the versions"))).color(v.warn_fg_color));
            return None;
        }
        Load::Ready(list) => list,
    };
    if d.way == Way::Manual {
        ui.label(egui::RichText::new(tr("Not installed from a package: its page opens instead")).color(v.weak_text_color()).size(12.5));
        ui.add_space(4.0);
    }
    let max_h = (ui.ctx().content_rect().height() - 220.0).max(240.0);
    egui::ScrollArea::vertical().id_salt("versions").max_height(max_h).min_scrolled_height(max_h.min(440.0)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        for (i, r) in list.iter().enumerate() {
            let on = i == d.cursor;
            // The one under the cursor opens up: its notes, a warning.
            let notes: Vec<&str> = if on { r.notes.lines().filter(|l| !l.trim().is_empty()).take(8).collect() } else { Vec::new() };
            let mut h = 40.0;
            let note_gals: Vec<_> = notes
                .iter()
                .map(|l| {
                    let line = l.trim().trim_start_matches(['-', '*']).trim();
                    ui.painter().layout(format!("• {line}"), FontId::proportional(12.5), v.text_color(), w - 40.0)
                })
                .collect();
            h += note_gals.iter().map(|g| g.size().y + 2.0).sum::<f32>();
            let warn = d.warning(r).filter(|_| on).map(|w0| {
                let text = if d.armed == Some(r.version) { format!("{} {w0}", tr("Press again to install.")) } else { w0.to_string() };
                ui.painter().layout(text, FontId::proportional(12.5), v.warn_fg_color, w - 40.0)
            });
            if let Some(g) = &warn {
                h += g.size().y + 6.0;
            }
            let fetching = d.fetching.as_ref().filter(|f| f.0 == r.version);
            if fetching.is_some() {
                h += 16.0;
            }
            let (rect, resp) = crate::view::settings_row(ui, h, on, ("version", i));
            if resp.clicked() {
                ev = Some(if on { VEv::Install(i) } else { VEv::Pick(i) });
            }
            let top = rect.min.y + 20.0;
            ui.painter().text(pos2(rect.min.x + 14.0, top), Align2::LEFT_CENTER, shown(r.version), FontId::proportional(16.0), v.strong_text_color());
            // What it is to this copy.
            let tag = match r.version.cmp(&current()) {
                std::cmp::Ordering::Equal => Some((tr("installed"), v.selection.bg_fill)),
                std::cmp::Ordering::Greater => Some((tr("newer"), egui::Color32::from_rgb(60, 150, 90))),
                std::cmp::Ordering::Less => None,
            };
            let mut x = rect.min.x + 90.0;
            if let Some((t, c)) = tag {
                let g = ui.painter().layout_no_wrap(t.to_string(), FontId::proportional(11.5), egui::Color32::WHITE);
                let chip = Rect::from_min_size(pos2(x, top - 10.0), vec2(g.size().x + 14.0, 20.0));
                ui.painter().rect(chip, 6.0, c, Stroke::NONE, StrokeKind::Inside);
                ui.painter().galley(pos2(chip.min.x + 7.0, chip.center().y - g.size().y / 2.0), g, egui::Color32::WHITE);
                x = chip.max.x + 8.0;
            }
            if r.asset.is_none() && d.way != Way::Manual {
                ui.painter().text(pos2(x, top), Align2::LEFT_CENTER, tr("no file for this install"), FontId::proportional(11.5), v.weak_text_color());
            }
            ui.painter().text(pos2(rect.max.x - 14.0, top), Align2::RIGHT_CENTER, &r.date, FontId::proportional(12.5), v.weak_text_color());
            let mut y = rect.min.y + 38.0;
            for g in note_gals {
                let gh = g.size().y;
                ui.painter().galley(pos2(rect.min.x + 18.0, y), g, v.text_color());
                y += gh + 2.0;
            }
            if let Some(g) = warn {
                let gh = g.size().y;
                ui.painter().galley(pos2(rect.min.x + 18.0, y + 4.0), g, v.warn_fg_color);
                y += gh + 6.0;
            }
            // Downloading: how far.
            if let Some((_, part, size)) = fetching {
                let got = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
                let f = if *size > 0 { (got as f32 / *size as f32).min(1.0) } else { 0.0 };
                let bar = Rect::from_min_size(pos2(rect.min.x + 18.0, y + 6.0), vec2(w - 160.0, 6.0));
                ui.painter().rect_filled(bar, 3.0, v.extreme_bg_color);
                ui.painter().rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * f, bar.height())), 3.0, v.selection.bg_fill);
                ui.painter().text(pos2(rect.max.x - 14.0, bar.center().y), Align2::RIGHT_CENTER, format!("{} {:.0}%", tr("Downloading"), f * 100.0), FontId::proportional(11.5), v.weak_text_color());
            }
        }
    });
    ev
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_and_files() {
        assert_eq!(parse_version("v0.43.2"), Some((0, 43, 2)));
        assert_eq!(parse_version("0.44"), None);
        assert_eq!(parse_version("v1.2.3.4"), None);
        let json = r#"[
            {"tag_name": "v0.42.0", "draft": false, "prerelease": false, "published_at": "2026-10-26T12:00:00Z", "body": "- a", "html_url": "u",
             "assets": [{"name": "PKGBUILD", "browser_download_url": "x", "size": 1},
                        {"name": "polka-0.42.0-1-x86_64.pkg.tar.zst", "browser_download_url": "p", "size": 5},
                        {"name": "polka-0.42.0-x86_64.flatpak", "browser_download_url": "f", "size": 9}]},
            {"tag_name": "v0.43.0", "draft": false, "prerelease": false, "published_at": "2026-10-26T13:00:00Z", "body": "", "html_url": "u", "assets": []},
            {"tag_name": "v0.50.0", "draft": true, "prerelease": false, "published_at": "", "body": "", "html_url": "u", "assets": []}
        ]"#;
        let l = parse(json, Way::Flatpak).unwrap();
        assert_eq!(l.iter().map(|r| r.version).collect::<Vec<_>>(), vec![(0, 43, 0), (0, 42, 0)]);
        assert_eq!(l[1].asset.as_ref().map(|a| a.url.as_str()), Some("f"));
        assert_eq!(l[1].date, "2026-10-26");
        assert!(l[0].asset.is_none());
        assert_eq!(parse(json, Way::Pacman).unwrap()[1].asset.as_ref().map(|a| a.size), Some(5));
        assert_eq!(parse_commits(r#"[{"version": "0.44.0", "commit": "abc"}, {"version": "x", "commit": "y"}]"#), vec![((0, 44, 0), "abc".to_string())]);
        // GitHub's refusal comes as an object with a message.
        assert_eq!(parse(r#"{"message": "API rate limit exceeded"}"#, Way::Flatpak), Err("API rate limit exceeded".into()));
    }

    #[test]
    fn flatpak_plans() {
        let rel = |v: Version, c: Option<&str>| Release { version: v, tag: String::new(), date: String::new(), notes: String::new(), page: String::new(), asset: None, commit: c.map(String::from) };
        let repo = Flat { remote: Some("polka".into()), from_repo: true };
        let s = flatpak_scope();
        // From the repository: the newest by updating, an older one by its
        // commit, one the repository doesn't keep from its file.
        assert_eq!(plan(Way::Flatpak, &rel((0, 45, 0), Some("new")), true, Some(&repo)), Plan::Run(format!("flatpak update {s} {APP_ID}")));
        assert_eq!(plan(Way::Flatpak, &rel((0, 44, 0), Some("abc")), false, Some(&repo)), Plan::Run(format!("flatpak update {s} --commit=abc {APP_ID}")));
        assert_eq!(plan(Way::Flatpak, &rel((0, 40, 0), None), false, Some(&repo)), Plan::File);
        // From elsewhere (a file, a local build): back to the repository first.
        let file = Flat { remote: Some("polka".into()), from_repo: false };
        assert_eq!(plan(Way::Flatpak, &rel((0, 45, 0), None), true, Some(&file)), Plan::Run(format!("flatpak install {s} --reinstall 'polka' {APP_ID}//stable")));
        assert_eq!(
            plan(Way::Flatpak, &rel((0, 44, 0), Some("abc")), false, Some(&file)),
            Plan::Run(format!("flatpak install {s} --reinstall 'polka' {APP_ID}//stable && flatpak update {s} --commit=abc {APP_ID}"))
        );
        let none = Flat { remote: None, from_repo: false };
        assert!(matches!(plan(Way::Flatpak, &rel((0, 45, 0), None), true, Some(&none)), Plan::Run(c) if c.ends_with("/polka.flatpakref'")));
        // The Arch package: always its file.
        assert_eq!(plan(Way::Pacman, &rel((0, 45, 0), Some("x")), true, None), Plan::File);
    }
}
