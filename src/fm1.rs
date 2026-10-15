//! org.freedesktop.FileManager1 on the session bus: "Show in folder" from
//! browsers, chat programs and the like opens the folder in Polka with the
//! file under the cursor. Whoever holds the name first keeps it (Dolphin
//! doesn't let it go); Polka waits in the queue and takes over when it
//! leaves.

use std::path::PathBuf;

use crossbeam_channel::Sender;

use crate::app::Msg;

pub const NAME: &str = "org.freedesktop.FileManager1";
pub const PATH: &str = "/org/freedesktop/FileManager1";

/// What was asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum Show {
    /// Each folder in a tab.
    Folders(Vec<PathBuf>),
    /// The folders holding these, with them selected.
    Items(Vec<PathBuf>),
    /// As Items, then the first one's properties.
    Properties(Vec<PathBuf>),
}

struct Fm {
    tx: Sender<Msg>,
    ctx: egui::Context,
}

impl Fm {
    fn send(&self, uris: Vec<String>, startup_id: String, f: fn(Vec<PathBuf>) -> Show) {
        let paths: Vec<PathBuf> = uris.iter().filter_map(|u| path_of(u)).collect();
        crate::log::line(format!("FileManager1: {:?} (token {})", f(paths.clone()), if startup_id.is_empty() { "none" } else { "given" }));
        if paths.is_empty() {
            return;
        }
        let _ = self.tx.send(Msg::Show(f(paths), startup_id));
        self.ctx.request_repaint();
    }
}

#[zbus::interface(name = "org.freedesktop.FileManager1")]
impl Fm {
    fn show_folders(&self, uris: Vec<String>, startup_id: String) {
        self.send(uris, startup_id, Show::Folders);
    }

    fn show_items(&self, uris: Vec<String>, startup_id: String) {
        self.send(uris, startup_id, Show::Items);
    }

    fn show_item_properties(&self, uris: Vec<String>, startup_id: String) {
        self.send(uris, startup_id, Show::Properties);
    }
}

/// A file:// URI (or a plain absolute path) as a path; other schemes aren't
/// for a local file manager.
pub fn path_of(uri: &str) -> Option<PathBuf> {
    if uri.starts_with("file://") || uri.starts_with('/') {
        Some(crate::app::arg_path(std::ffi::OsStr::new(uri)))
    } else {
        None
    }
}

/// Folders and the names to select in each, in the order asked.
pub fn group(items: &[PathBuf]) -> Vec<(PathBuf, Vec<String>)> {
    let mut out: Vec<(PathBuf, Vec<String>)> = Vec::new();
    for p in items {
        let (Some(dir), Some(name)) = (p.parent(), p.file_name()) else { continue };
        let name = name.to_string_lossy().into_owned();
        match out.iter_mut().find(|(d, _)| d == dir) {
            Some((_, names)) => names.push(name),
            None => out.push((dir.to_path_buf(), vec![name])),
        }
    }
    out
}

/// Ask for the name and answer on it, from a thread of its own.
pub fn serve(tx: Sender<Msg>, ctx: egui::Context) {
    std::thread::Builder::new()
        .name("polka-fm1".into())
        .spawn(move || {
            if let Err(e) = run(tx, ctx) {
                crate::log::line(format!("FileManager1: {e}"));
            }
        })
        .ok();
}

fn run(tx: Sender<Msg>, ctx: egui::Context) -> zbus::Result<()> {
    use zbus::fdo::{RequestNameFlags, RequestNameReply};
    let conn = zbus::blocking::connection::Builder::session()?.serve_at(PATH, Fm { tx, ctx })?.build()?;
    // Replace a holder that allows it; else wait in line.
    let flags = RequestNameFlags::AllowReplacement | RequestNameFlags::ReplaceExisting;
    let reply = conn.request_name_with_flags(NAME, flags.into())?;
    crate::log::line(format!(
        "FileManager1: {}",
        match reply {
            RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => "answering \"Show in folder\"".to_string(),
            RequestNameReply::InQueue => format!("queued behind {}", owner(&conn).unwrap_or_else(|| "another file manager".into())),
            RequestNameReply::Exists => "taken".into(),
        }
    ));
    // The connection answers on its own threads; keep it alive.
    loop {
        std::thread::park();
    }
}

/// Who answers "Show in folder" now, for --diagnose.
pub fn current_owner() -> String {
    match zbus::blocking::Connection::session() {
        Ok(c) => owner(&c).unwrap_or_else(|| "nobody".into()),
        Err(e) => format!("no session bus ({e})"),
    }
}

/// The program holding the name, for the log.
fn owner(conn: &zbus::blocking::Connection) -> Option<String> {
    let dbus = zbus::blocking::fdo::DBusProxy::new(conn).ok()?;
    let name = zbus::names::BusName::try_from(NAME).ok()?;
    let pid = dbus.get_connection_unix_process_id(name).ok()?;
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(format!("{} (pid {pid})", comm.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_and_groups() {
        assert_eq!(path_of("file:///home/a/My%20File.txt"), Some(PathBuf::from("/home/a/My File.txt")));
        assert_eq!(path_of("/tmp/x"), Some(PathBuf::from("/tmp/x")));
        assert_eq!(path_of("https://example.org/x"), None);
        let g = group(&[PathBuf::from("/a/x"), PathBuf::from("/b/y"), PathBuf::from("/a/z")]);
        assert_eq!(g, vec![(PathBuf::from("/a"), vec!["x".to_string(), "z".into()]), (PathBuf::from("/b"), vec!["y".to_string()])]);
    }
}
