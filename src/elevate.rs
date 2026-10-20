//! Administrator rights: what isn't allowed (saving a system config,
//! moving files in /etc…) is done again through sudo, with the password
//! typed on the on-screen keyboard. The password stays in memory a while,
//! like sudo's own.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::app::{App, Osk, OskPurpose, Overlay};
use crate::i18n::tr;
use crate::ops::{After, Op, Secret};
use crate::vfs::Location;

/// How long a password that worked is reused.
const KEEP: Duration = Duration::from_secs(10 * 60);

impl App {
    /// Do `steps` as administrator: right away with a password from a few
    /// minutes ago, else after asking for it.
    pub fn elevate(&mut self, steps: Vec<Vec<OsString>>, label: &'static str, touches: Vec<Location>, after: Option<After>) {
        let op = Op::Elevated { steps, label, touches, pw: Secret(String::new()), after };
        match self.sudo_pw.clone().filter(|(_, t)| t.elapsed() < KEEP) {
            Some((pw, _)) => self.start(with_password(op, pw)),
            None => self.ask_sudo(op, false),
        }
    }

    fn ask_sudo(&mut self, op: Op, wrong: bool) {
        let title = if wrong { tr("Wrong password — administrator password (sudo)") } else { tr("Needs administrator rights — password (sudo)") };
        let mut k = Osk::new(title, "", OskPurpose::Sudo { op: Box::new(op) });
        k.secret = true;
        self.overlay = Some(Overlay::Osk(k));
    }

    /// The password typed: try it.
    pub fn sudo_typed(&mut self, op: Op, pw: String) {
        if pw.is_empty() {
            return;
        }
        let pw = Secret(pw);
        self.sudo_pw = Some((pw.clone(), Instant::now()));
        self.start(with_password(op, pw));
    }

    /// A finished job: not allowed (offer to do it as administrator), a
    /// wrong password (ask again), or done as administrator (what follows).
    /// True when it's been dealt with here.
    pub fn elevated_done(&mut self, op: &Op, result: &Result<(), String>, denied: bool) -> bool {
        match (op, result) {
            (Op::Elevated { .. }, Err(e)) if e == crate::ops::WRONG_PASSWORD => {
                self.sudo_pw = None;
                self.ask_sudo(op.clone(), true);
                true
            }
            (Op::Elevated { after: Some(After::Saved { path, text }), .. }, Ok(())) => {
                for t in &mut self.tabs {
                    if let Some(v) = t.viewer.as_mut().filter(|v| v.path == *path) {
                        if let crate::viewer::Content::Text(t) = &mut v.content {
                            t.ed.mark_saved(text);
                        }
                    }
                }
                self.toast(tr("Saved"), false);
                true
            }
            (Op::Elevated { after: Some(After::Net { id, cred, open, mount }), .. }, r) => {
                self.net_elevated(id.clone(), cred.clone(), *open, *mount, r);
                true
            }
            (Op::Elevated { .. }, _) => false,
            (op, Err(_)) if denied => match crate::ops::elevated_steps(op) {
                Some((steps, label)) => {
                    let touches = touches_of(op);
                    self.elevate(steps, label, touches, None);
                    true
                }
                None => false,
            },
            _ => false,
        }
    }

    /// A text file that couldn't be written: written to a copy, then copied
    /// over it as administrator (keeping its owner and mode).
    pub fn save_elevated(&mut self, path: &Path, text: String) {
        // Where the host sees it too (sudo runs there in a sandbox).
        let tmp = dirs::cache_dir().unwrap_or_else(crate::vfs::temp_root).join(format!("polka/save-{}", std::process::id()));
        if let Err(e) = std::fs::create_dir_all(&tmp).and_then(|_| std::fs::write(tmp.join("text"), text.as_bytes())) {
            return self.toast(format!("{}: {e}", tr("Failed")), true);
        }
        let step = vec!["cp".into(), "--".into(), tmp.join("text").into_os_string(), path.as_os_str().to_os_string()];
        let touches = path.parent().map(|p| vec![Location::Local(p.to_path_buf())]).unwrap_or_default();
        self.elevate(vec![step], tr("Saving"), touches, Some(After::Saved { path: path.to_path_buf(), text }));
    }

    /// One command as administrator in folder `dir` (rename, new folder…).
    pub fn elevate_one(&mut self, cmd: &[&std::ffi::OsStr], dir: &Path, label: &'static str) {
        let step = cmd.iter().map(|s| s.to_os_string()).collect();
        self.elevate(vec![step], label, vec![Location::Local(dir.to_path_buf())], None);
    }

    /// Links to `sources` in `dir` (a free name each), as administrator if
    /// the folder isn't ours.
    pub fn make_links(&mut self, sources: Vec<PathBuf>, dir: PathBuf) {
        let mut todo: Vec<(PathBuf, PathBuf)> = Vec::new();
        for s in sources {
            let name = s.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            todo.push((s.clone(), crate::ops::unique_path(&dir, &name)));
        }
        let mut denied = Vec::new();
        for (s, l) in &todo {
            match std::os::unix::fs::symlink(s, l) {
                Ok(()) => {}
                Err(e) if crate::ops::denied(&e) => denied.push((s.clone(), l.clone())),
                Err(e) => return self.toast(format!("{}: {e}", tr("Failed")), true),
            }
        }
        self.reload_all();
        if !denied.is_empty() {
            let steps = denied.into_iter().map(|(s, l)| vec!["ln".into(), "-s".into(), "--".into(), s.into_os_string(), l.into_os_string()]).collect();
            self.elevate(steps, tr("Creating links"), vec![Location::Local(dir)], None);
        }
    }
}

fn with_password(op: Op, pw: Secret) -> Op {
    match op {
        Op::Elevated { steps, label, touches, after, .. } => Op::Elevated { steps, label, touches, pw, after },
        other => other,
    }
}

/// The folders an operation changes.
fn touches_of(op: &Op) -> Vec<Location> {
    let mut v: Vec<Location> = Vec::new();
    match op {
        Op::Copy { sources, dest, .. } | Op::Move { sources, dest, .. } => {
            v.extend(sources.iter().filter_map(|s| s.parent().map(|(p, _)| p)));
            v.push(dest.location());
        }
        Op::Trash { paths } | Op::Delete { paths } => v.extend(paths.iter().filter_map(|p| p.parent().map(|d| Location::Local(d.to_path_buf())))),
        _ => {}
    }
    v.dedup();
    v
}
