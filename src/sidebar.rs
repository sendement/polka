//! The sidebar's own commands: each place's △ menu, drives (mount,
//! unmount, eject) and emptying the trash.

use std::path::PathBuf;

use crate::app::{App, Cmd, Focus, Menu, MenuItem, Msg, Overlay, SideItem, item, trash_files};
use crate::drives::{Done, Drive, Op};
use crate::i18n::{tr, trn};
use crate::vfs::Location;

impl App {
    /// △ on a sidebar entry.
    pub fn side_menu(&mut self, it: SideItem) {
        let i = self.side_cursor;
        let mut items: Vec<MenuItem> = Vec::new();
        let real = !it.offline() && !it.term;
        if it.term {
            items.push(item(tr("New terminal tab"), Cmd::TermNew));
        }
        if let Some(d) = &it.drive {
            if d.mount.is_none() {
                items.push(item(tr("Mount"), Cmd::DriveMount(i)));
            }
        }
        if real {
            items.push(item(tr("New tab"), Cmd::OpenInNewTab(it.loc.clone())));
        }
        if let Some(d) = &it.drive {
            if d.mount.is_some() {
                items.push(item(tr("Unmount"), Cmd::DriveUnmount(i)));
            }
            if d.removable || d.optical {
                items.push(item(tr("Eject"), Cmd::DriveEject(i)));
            }
        }
        if it.loc.local_path().is_some() && trash_files().as_deref() == it.loc.local_path() {
            items.push(MenuItem { enabled: crate::trashbin::count() > 0, ..item(tr("Empty trash…"), Cmd::EmptyTrashAsk) });
        }
        if let Some((id, online)) = &it.net {
            items.push(if *online { item(tr("Disconnect"), Cmd::NetDisconnect(id.clone())) } else { item(tr("Connect"), Cmd::NetConnect(id.clone())) });
            items.push(item(tr("Edit connection…"), Cmd::NetEdit(id.clone())));
            items.push(item(tr("Remove connection…"), Cmd::NetRemoveAsk(id.clone())));
            items.push(item(tr("Add network place…"), Cmd::NetAdd));
        }
        // Any entry can wear its own icon; drives keep theirs by UUID.
        let key = match (&it.drive, &it.loc) {
            _ if it.net.is_some() => it.net.as_ref().map(|(id, _)| PathBuf::from(format!("net:{id}"))),
            (Some(d), _) => Some(PathBuf::from(d.key())),
            (None, _) if it.term => Some(PathBuf::from("polka:terminal")),
            (None, Location::Steam(_)) => Some(PathBuf::from("polka:steam")),
            (None, loc) => loc.local_path().map(|p| p.to_path_buf()),
        };
        if let Some(k) = key {
            items.push(item(tr("Choose icon…"), Cmd::ChooseIcon(k)));
        }
        if let (true, Some(p)) = (it.pinned, it.loc.local_path()) {
            items.push(item(tr("Unpin from sidebar"), Cmd::Unpin(p.to_path_buf())));
        }
        self.overlay = Some(Overlay::Menu(Menu::new(it.label, items)));
    }

    pub fn side_cmd(&mut self, cmd: Cmd) {
        let drive = |app: &App, i: usize| app.side_items.get(i).and_then(|it| it.drive.clone());
        match cmd {
            Cmd::EmptyTrashAsk => {
                let n = crate::trashbin::count();
                if n == 0 {
                    return self.toast(tr("The trash is empty"), false);
                }
                let items = vec![item(tr("Empty trash"), Cmd::EmptyTrashNow), item(tr("Cancel"), Cmd::Nothing)];
                self.overlay = Some(Overlay::Menu(Menu::new(trn("Delete %n items in the trash for good?", n), items)));
            }
            Cmd::EmptyTrashNow => {
                let (tx, wake) = (self.msg_tx.clone(), self.wake());
                std::thread::spawn(move || {
                    let (n, failed) = crate::trashbin::empty();
                    let _ = tx.send(Msg::TrashEmptied(n, failed));
                    wake();
                });
            }
            Cmd::DriveMount(i) => {
                if let Some(d) = drive(self, i) {
                    self.drive_op(d, Op::Mount);
                }
            }
            Cmd::DriveUnmount(i) => {
                if let Some(d) = drive(self, i) {
                    self.drive_op(d, Op::Unmount);
                }
            }
            Cmd::DriveEject(i) => {
                if let Some(d) = drive(self, i) {
                    // Every mounted partition of that disk goes first.
                    let mounted = crate::drives::list().into_iter().filter(|x| x.disk == d.disk && x.mount.is_some()).collect();
                    self.drive_op(d, Op::Eject(mounted));
                }
            }
            _ => {}
        }
    }

    /// Run a drive operation off the UI thread (polkit may ask for a
    /// password meanwhile).
    pub fn drive_op(&mut self, d: Drive, op: Op) {
        let msg = match op {
            Op::Mount => tr("Mounting…"),
            Op::Unmount => tr("Unmounting…"),
            Op::Eject(_) => tr("Ejecting…"),
        };
        self.toast(msg, false);
        let (tx, wake) = (self.msg_tx.clone(), self.wake());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Drive(crate::drives::apply(&d, &op)));
            wake();
        });
    }

    pub fn drive_done(&mut self, r: Result<Done, String>) {
        self.refresh_sidebar();
        let gone = match r {
            Err(e) => return self.toast(format!("{}: {e}", tr("Failed")), true),
            Ok(Done::Mounted(p)) => {
                // Opened right away, in place of the disk usage page if any.
                let cfg = self.cfg.clone();
                if let Some(u) = self.tabs[self.tab].usage.as_deref_mut() {
                    u.shown = false;
                }
                self.close_viewer();
                self.pane_mut().navigate(Location::Local(p), None, &cfg);
                self.focus = Focus::Panes;
                // "Mounting…" is done with.
                self.toast = None;
                return;
            }
            Ok(Done::Unmounted(gone)) => {
                self.toast(tr("Unmounted"), false);
                gone
            }
            Ok(Done::Ejected(gone)) => {
                self.toast(tr("The drive can be removed now"), false);
                gone
            }
        };
        // Panes on what's gone go home.
        let home = dirs::home_dir().unwrap_or_else(|| "/".into());
        let cfg = self.cfg.clone();
        for t in &mut self.tabs {
            {
            let p = &mut t.pane;
                if p.loc.local_path().is_some_and(|l| gone.iter().any(|g| l.starts_with(g))) {
                    p.navigate(Location::Local(home.clone()), None, &cfg);
                }
            }
        }
    }
}
