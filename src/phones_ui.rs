//! Phones in Polka: open one (mounting it first), unmount it, and what to
//! tell the user when it can't be done.

use std::path::PathBuf;

use crate::app::{App, Msg};
use crate::i18n::tr;
use crate::phones::Trouble;
use crate::vfs::Location;

impl App {
    fn phone(&self, key: &str) -> Option<crate::phones::Phone> {
        self.phones.iter().find(|p| p.key() == key).cloned()
    }

    /// Open it: right away if mounted, else after mounting.
    pub fn phone_open(&mut self, key: &str) {
        let Some(p) = self.phone(key) else { return };
        if let Some(at) = p.mounted(&crate::net::mount_points()) {
            return self.phone_show(at);
        }
        if !self.net_busy.insert(key.to_string()) {
            return;
        }
        self.toast(format!("{} {}…", tr("Connecting"), p.name), false);
        let tx = self.msg_tx.clone();
        let wake = self.wake();
        let key = key.to_string();
        let alone = self.phones.iter().filter(|x| x.kind == crate::phones::Kind::Android && x.files).count() <= 1;
        std::thread::spawn(move || {
            let result = crate::phones::mount(&p, alone);
            let _ = tx.send(Msg::PhoneDone { key, result });
            wake();
        });
    }

    fn phone_show(&mut self, at: PathBuf) {
        let cfg = self.cfg.clone();
        if let Some(u) = self.tabs[self.tab].usage.as_deref_mut() {
            u.shown = false;
        }
        if let Some(t) = self.tabs[self.tab].term.as_deref_mut() {
            t.shown = false;
        }
        self.pane_mut().navigate(Location::Local(at), None, &cfg);
    }

    pub fn phone_done(&mut self, key: String, result: Result<PathBuf, Trouble>) {
        self.net_busy.remove(&key);
        let name = self.phone(&key).map(|p| p.name).unwrap_or_default();
        match result {
            Ok(at) => {
                self.refresh_sidebar();
                self.phone_show(at);
            }
            Err(t) => {
                let msg = match t {
                    Trouble::Missing(pkgs) => format!("{}: {pkgs}", tr("Install one of")),
                    Trouble::NoFiles => tr("On the phone, choose USB use: File transfer").to_string(),
                    Trouble::Trust => tr("Unlock the iPhone and tap Trust, then open it again").to_string(),
                    Trouble::Failed(e) => {
                        crate::log::line(format!("phone {name}: {e}"));
                        let l = e.to_lowercase();
                        if l.contains("busy") || l.contains("claim") {
                            format!("{name}: {}", tr("another program is using the phone"))
                        } else if l.contains("timeout") || l.contains("no mtp device") {
                            // A locked screen, or the phone's MTP side stuck.
                            format!("{name}: {}", tr("unlock the phone and allow access to files; if it still doesn't answer, plug it in again"))
                        } else {
                            format!("{name}: {e}")
                        }
                    }
                };
                self.toast(msg, true);
            }
        }
    }

    /// Unmount it (do before unplugging), taking the panes out of it.
    pub fn phone_disconnect(&mut self, key: &str) {
        let Some(p) = self.phone(key) else { return };
        let Some(at) = p.mounted(&crate::net::mount_points()) else { return self.refresh_sidebar() };
        let home = dirs::home_dir().unwrap_or_else(|| "/".into());
        let cfg = self.cfg.clone();
        for t in &mut self.tabs {
            if t.pane.loc.local_path().is_some_and(|x| x.starts_with(&at)) {
                t.pane.navigate(Location::Local(home.clone()), None, &cfg);
            }
        }
        let tx = self.msg_tx.clone();
        let wake = self.wake();
        std::thread::spawn(move || {
            if let Err(e) = crate::phones::unmount(&p, &at) {
                let _ = tx.send(Msg::Toast(format!("{}: {e}", p.name), true));
            }
            let _ = tx.send(Msg::NetGone);
            wake();
        });
    }
}
