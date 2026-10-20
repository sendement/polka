//! Network places in Polka: the sidebar's "Network" section, connecting
//! (asking for a password when one is needed or was wrong), disconnecting,
//! and the dialog that adds or edits a connection.

use std::path::PathBuf;

use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2};

use crate::app::{App, Cmd, Focus, Menu, Msg, Osk, OskPurpose, Overlay, item};
use crate::i18n::tr;
use crate::input::Action;
use crate::net::{Conn, KINDS, Kind, Plan, Tool, Unplan};
use crate::ops::After;
use crate::vfs::Location;

/// The add / edit dialog.
pub struct NetDlg {
    pub conn: Conn,
    /// Not saved yet.
    pub new: bool,
    pub row: NRow,
    /// On the buttons' row: Connect, Save, Cancel.
    pub button: usize,
    /// Typed in here (saved to the keyring or the session, never the
    /// settings file).
    pub password: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NRow {
    Kind,
    Name,
    Address,
    Path,
    User,
    Port,
    Password,
    Remember,
    Tool,
    Buttons,
}

impl NetDlg {
    /// The rows this kind has.
    pub fn rows(&self) -> Vec<NRow> {
        let k = self.conn.kind;
        let mut v = vec![NRow::Kind, NRow::Name, NRow::Address, NRow::Path];
        if k.has_user() {
            v.push(NRow::User);
        }
        if k.has_port() {
            v.push(NRow::Port);
        }
        if k.has_user() {
            v.push(NRow::Password);
            v.push(NRow::Remember);
        }
        if k.tools().len() > 1 {
            v.push(NRow::Tool);
        }
        v.push(NRow::Buttons);
        v
    }

    fn step_row(&mut self, d: isize) {
        let rows = self.rows();
        let at = rows.iter().position(|r| *r == self.row).unwrap_or(0) as isize;
        self.row = rows[(at + d).clamp(0, rows.len() as isize - 1) as usize];
    }

    /// ← → on a row of choices.
    fn step(&mut self, row: NRow, d: isize) {
        match row {
            NRow::Kind => {
                let at = KINDS.iter().position(|k| *k == self.conn.kind).unwrap_or(0) as isize;
                self.conn.kind = KINDS[(at + d).rem_euclid(KINDS.len() as isize) as usize];
                self.conn.tool = None;
            }
            NRow::Tool => {
                let tools = self.conn.kind.tools();
                let cur = self.conn.tool().and_then(|t| tools.iter().position(|x| *x == t)).unwrap_or(0) as isize;
                self.conn.tool = Some(tools[(cur + d).rem_euclid(tools.len() as isize) as usize]);
            }
            NRow::Remember => self.conn.remember = !self.conn.remember,
            NRow::Buttons => self.button = (self.button as isize + d).clamp(0, 2) as usize,
            _ => {}
        }
    }

    /// The label a text row has for this kind.
    fn label(&self, row: NRow) -> &'static str {
        match (row, self.conn.kind) {
            (NRow::Address, Kind::WebDav) => tr("Address (URL)"),
            (NRow::Address, Kind::Rclone) => tr("Remote (from rclone config)"),
            (NRow::Address, _) => tr("Server"),
            (NRow::Path, Kind::Smb) => tr("Share"),
            (NRow::Path, Kind::Nfs) => tr("Export"),
            (NRow::Path, _) => tr("Folder"),
            (NRow::Name, _) => tr("Name"),
            (NRow::User, _) => tr("User"),
            (NRow::Port, _) => tr("Port"),
            (NRow::Password, _) => tr("Password"),
            _ => "",
        }
    }
}

impl App {
    /// The connection with this id.
    fn conn(&self, id: &str) -> Option<Conn> {
        self.cfg.connections.iter().find(|c| c.id == id).cloned()
    }

    pub fn net_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::NetAdd => {
                crate::net::forget_tools();
                let conn = Conn { id: crate::net::new_id(), remember: crate::net::keyring(), ..Default::default() };
                self.overlay = Some(Overlay::Net(Box::new(NetDlg { conn, new: true, row: NRow::Kind, button: 0, password: String::new() })));
            }
            Cmd::NetEdit(id) => {
                crate::net::forget_tools();
                if let Some(conn) = self.conn(&id) {
                    let password = self.net_pw.get(&id).cloned().unwrap_or_default();
                    self.overlay = Some(Overlay::Net(Box::new(NetDlg { conn, new: false, row: NRow::Name, button: 1, password })));
                }
            }
            Cmd::NetConnect(id) => self.net_connect(&id, true),
            Cmd::NetDisconnect(id) => self.net_disconnect(&id),
            Cmd::NetRemoveAsk(id) => {
                let Some(c) = self.conn(&id) else { return };
                let items = vec![item(tr("Remove"), Cmd::NetRemoveNow(id)), item(tr("Cancel"), Cmd::Nothing)];
                self.overlay = Some(Overlay::Menu(Menu::new(format!("{} «{}»?", tr("Remove the connection"), c.name), items)));
            }
            Cmd::NetRemoveNow(id) => {
                self.cfg.connections.retain(|c| c.id != id);
                self.cfg.save();
                self.net_pw.remove(&id);
                crate::net::keyring_clear(&id);
                self.refresh_sidebar();
            }
            _ => {}
        }
    }

    /// Open it: right away when it's mounted, else after connecting.
    pub fn net_open(&mut self, id: &str) {
        let mounts = crate::net::mount_points();
        match self.conn(id).and_then(|c| c.mounted(&mounts)) {
            Some(p) => self.net_show(p),
            None => self.net_connect(id, true),
        }
    }

    fn net_show(&mut self, p: PathBuf) {
        let cfg = self.cfg.clone();
        if let Some(u) = self.tabs[self.tab].usage.as_deref_mut() {
            u.shown = false;
        }
        if let Some(t) = self.tabs[self.tab].term.as_deref_mut() {
            t.shown = false;
        }
        self.pane_mut().navigate(Location::Local(p), None, &cfg);
    }

    /// Connect: with the password known (the session's, the keyring's),
    /// asking for one first when it'll be needed.
    pub fn net_connect(&mut self, id: &str, open: bool) {
        let Some(c) = self.conn(id) else { return };
        if self.net_busy.contains(id) {
            return;
        }
        let pw = self.net_pw.get(id).cloned().or_else(|| if c.remember { crate::net::keyring_get(id) } else { None });
        if pw.is_none() && c.kind.wants_password() && !c.user.is_empty() {
            return self.net_ask_password(&c, open, false);
        }
        self.net_try(c, pw, open);
    }

    fn net_ask_password(&mut self, c: &Conn, open: bool, wrong: bool) {
        let title = if wrong { format!("{} — {}", tr("Wrong password"), c.name) } else { format!("{} {}", tr("Password for"), c.name) };
        let mut k = Osk::new(&title, "", OskPurpose::NetPassword { id: c.id.clone(), open });
        k.secret = true;
        self.overlay = Some(Overlay::Osk(k));
    }

    /// The password typed when connecting.
    pub fn net_password_typed(&mut self, id: String, pw: String, open: bool) {
        let Some(c) = self.conn(&id) else { return };
        self.net_try(c, Some(pw), open);
    }

    fn net_try(&mut self, c: Conn, pw: Option<String>, open: bool) {
        let plan = match crate::net::plan(&c, pw.as_deref()) {
            Ok(p) => p,
            Err(e) => return self.toast(format!("{}: {e}", c.name), true),
        };
        match plan {
            Plan::Root { steps, cred } => {
                if let Some(pw) = pw {
                    self.net_pw.insert(c.id.clone(), pw);
                }
                self.elevate(steps, tr("Connecting"), Vec::new(), Some(After::Net { id: c.id, cred, open, mount: true }));
            }
            Plan::User => {
                self.net_busy.insert(c.id.clone());
                self.toast(format!("{} {}…", tr("Connecting"), c.name), false);
                let tx = self.msg_tx.clone();
                let wake = self.wake();
                std::thread::spawn(move || {
                    let r = crate::net::mount(&c, pw.as_deref());
                    let _ = tx.send(Msg::NetDone { id: c.id.clone(), result: r, open, pw });
                    wake();
                });
            }
        }
    }

    /// A connection attempt ended.
    pub fn net_done(&mut self, id: String, result: Result<PathBuf, String>, open: bool, pw: Option<String>) {
        self.net_busy.remove(&id);
        let Some(c) = self.conn(&id) else { return };
        match result {
            Ok(p) => {
                if let Some(pw) = pw {
                    if c.remember && crate::net::keyring() {
                        if let Err(e) = crate::net::keyring_set(&id, &c.name, &pw) {
                            crate::log::line(format!("keyring: {e}"));
                        }
                    }
                    self.net_pw.insert(id, pw);
                }
                self.refresh_sidebar();
                if open {
                    self.net_show(p);
                }
            }
            Err(e) => {
                crate::log::line(format!("connecting {}: {e}", c.name));
                if crate::net::auth_failed(&e) && c.kind.has_user() {
                    // The password was wrong (or one is needed): ask.
                    self.net_pw.remove(&id);
                    self.net_ask_password(&c, open, pw.is_some());
                } else {
                    self.toast(format!("{}: {e}", c.name), true);
                }
            }
        }
    }

    /// Disconnect, first taking the panes out of it.
    pub fn net_disconnect(&mut self, id: &str) {
        let Some(c) = self.conn(id) else { return };
        let Some(at) = c.mounted(&crate::net::mount_points()) else { return self.refresh_sidebar() };
        let home = dirs::home_dir().unwrap_or_else(|| "/".into());
        let cfg = self.cfg.clone();
        for t in &mut self.tabs {
            if t.pane.loc.local_path().is_some_and(|p| p.starts_with(&at)) {
                t.pane.navigate(Location::Local(home.clone()), None, &cfg);
            }
        }
        match crate::net::unmount_plan(&c, &at) {
            Unplan::Root(step) => self.elevate(vec![step], tr("Disconnecting"), Vec::new(), Some(After::Net { id: id.to_string(), cred: None, open: false, mount: false })),
            Unplan::User(cmd) => {
                let tx = self.msg_tx.clone();
                let wake = self.wake();
                let name = c.name.clone();
                std::thread::spawn(move || {
                    if let Err(e) = crate::net::unmount(cmd) {
                        let _ = tx.send(Msg::Toast(format!("{name}: {e}"), true));
                    }
                    let _ = tx.send(Msg::NetGone);
                    wake();
                });
            }
        }
    }

    /// Done as administrator: the password file goes; then as any other.
    pub fn net_elevated(&mut self, id: String, cred: Option<PathBuf>, open: bool, mount: bool, result: &Result<(), String>) {
        if let Some(f) = cred {
            let _ = std::fs::remove_file(f);
        }
        match result {
            Ok(()) if mount => {
                let p = self.conn(&id).map(|c| c.mount_dir());
                self.net_done(id, p.ok_or_else(|| "gone".to_string()), open, None);
            }
            Ok(()) => self.refresh_sidebar(),
            Err(e) => {
                let name = self.conn(&id).map(|c| c.name).unwrap_or_default();
                if mount && crate::net::auth_failed(e) {
                    if let Some(c) = self.conn(&id) {
                        self.net_pw.remove(&id);
                        return self.net_ask_password(&c, open, true);
                    }
                }
                self.toast(format!("{name}: {e}"), true);
            }
        }
    }

    // -------------------------------------------------------- the dialog

    pub fn net_input(&mut self, a: Action) {
        let Some(Overlay::Net(d)) = self.overlay.as_mut() else { return };
        match a {
            Action::Up => d.step_row(-1),
            Action::Down => d.step_row(1),
            Action::Left | Action::Right => {
                let row = d.row;
                d.step(row, if a == Action::Left { -1 } else { 1 });
            }
            Action::Accept => {
                let row = d.row;
                self.net_activate(row);
            }
            Action::Menu => self.net_finish(true),
            Action::Back => self.overlay = None,
            _ => {}
        }
    }

    /// ✕ (or a click) on a row: type into it, flip it, or a button.
    pub fn net_activate(&mut self, row: NRow) {
        let Some(Overlay::Net(d)) = self.overlay.as_mut() else { return };
        d.row = row;
        match row {
            NRow::Kind | NRow::Tool => d.step(row, 1),
            NRow::Remember => d.step(row, 1),
            NRow::Buttons => match d.button {
                0 => self.net_finish(true),
                1 => self.net_finish(false),
                _ => self.overlay = None,
            },
            _ => {
                let Some(Overlay::Net(d)) = self.overlay.take() else { return };
                let now = match row {
                    NRow::Name => d.conn.name.clone(),
                    NRow::Address => d.conn.address.clone(),
                    NRow::Path => d.conn.path.clone(),
                    NRow::User => d.conn.user.clone(),
                    NRow::Port => d.conn.port.map(|p| p.to_string()).unwrap_or_default(),
                    NRow::Password => d.password.clone(),
                    _ => String::new(),
                };
                let title = d.label(row).to_string();
                let mut k = Osk::new(&title, &now, OskPurpose::NetField { dlg: d, row });
                k.secret = row == NRow::Password;
                self.overlay = Some(Overlay::Osk(k));
            }
        }
    }

    /// Text typed into the dialog: in, and the dialog back.
    pub fn net_field(&mut self, mut d: Box<NetDlg>, row: NRow, text: String) {
        let t = if row == NRow::Password { text } else { text.trim().to_string() };
        match row {
            NRow::Name => d.conn.name = t,
            NRow::Address => d.conn.address = t,
            NRow::Path => d.conn.path = t,
            NRow::User => d.conn.user = t,
            NRow::Port => d.conn.port = t.parse().ok(),
            NRow::Password => d.password = t,
            _ => {}
        }
        self.overlay = Some(Overlay::Net(d));
    }

    /// Save (and connect).
    fn net_finish(&mut self, connect: bool) {
        let Some(Overlay::Net(d)) = self.overlay.as_ref() else { return };
        if d.conn.address.trim().is_empty() {
            return self.toast(format!("{}: {}", tr("Failed"), d.label(NRow::Address)), true);
        }
        let Some(Overlay::Net(d)) = self.overlay.take() else { return };
        let mut c = d.conn;
        if c.name.trim().is_empty() {
            c.name = if c.path.is_empty() { c.address.clone() } else { format!("{} ({})", c.path.trim_matches('/'), c.address) };
        }
        if c.dir.is_empty() {
            let taken: Vec<String> = self.cfg.connections.iter().map(|x| x.dir.clone()).collect();
            c.dir = crate::net::dir_name(&c.name, &taken);
        }
        let id = c.id.clone();
        match self.cfg.connections.iter_mut().find(|x| x.id == id) {
            Some(x) => *x = c.clone(),
            None => self.cfg.connections.push(c.clone()),
        }
        self.cfg.save();
        if !d.password.is_empty() {
            self.net_pw.insert(id.clone(), d.password.clone());
            if c.remember && crate::net::keyring() {
                if let Err(e) = crate::net::keyring_set(&id, &c.name, &d.password) {
                    self.toast(format!("{}: {e}", tr("Keyring")), true);
                }
            }
        } else if !c.remember {
            crate::net::keyring_clear(&id);
        }
        self.refresh_sidebar();
        if connect {
            self.focus = Focus::Panes;
            self.net_connect(&id, true);
        }
    }
}

/// The dialog. Returns (row, step): step 0 activates the row.
pub fn draw(ui: &mut Ui, d: &NetDlg) -> Option<(NRow, isize)> {
    let mut ev = None;
    let v = ui.visuals().clone();
    let font = FontId::proportional(15.0);
    ui.label(egui::RichText::new(if d.new { tr("New connection") } else { tr("Connection") }).strong().size(18.0));
    ui.add_space(8.0);
    ui.spacing_mut().item_spacing.y = 4.0;
    let c = &d.conn;
    let tool = c.tool();
    for row in d.rows() {
        let h = if row == NRow::Kind || row == NRow::Tool { 48.0 } else { 40.0 };
        let (rect, r) = crate::view::settings_row(ui, h, d.row == row, ("net", row as u8));
        if row == NRow::Buttons {
            // The mouse's buttons (the pad: ← → and ✕).
            let names = [tr("Connect"), tr("Save"), tr("Cancel")];
            let w = 130.0;
            for (i, n) in names.iter().enumerate() {
                let b = Rect::from_min_size(pos2(rect.max.x - (3 - i) as f32 * (w + 8.0), rect.center().y - 15.0), egui::vec2(w, 30.0));
                let br = ui.interact(b, ui.id().with(("net-btn", i)), Sense::click());
                let on = d.row == NRow::Buttons && d.button == i;
                let fill = if on || (i == 0 && d.row != NRow::Buttons) { v.selection.bg_fill } else if br.hovered() { v.widgets.hovered.bg_fill } else { v.widgets.inactive.bg_fill };
                ui.painter().rect(b, 6.0, fill, Stroke::NONE, StrokeKind::Inside);
                let fg = if on || (i == 0 && d.row != NRow::Buttons) { v.selection.stroke.color } else { v.text_color() };
                ui.painter().text(b.center(), Align2::CENTER_CENTER, *n, font.clone(), fg);
                if br.clicked() {
                    ev = Some((NRow::Buttons, i as isize + 10));
                }
            }
            continue;
        }
        if r.clicked() && !matches!(row, NRow::Kind | NRow::Tool) {
            ev = Some((row, 0));
        }
        let label = match row {
            NRow::Kind => tr("Type"),
            NRow::Remember => {
                if crate::net::keyring() {
                    tr("Remember the password (keyring)")
                } else {
                    tr("Remember the password (this session)")
                }
            }
            NRow::Tool => tr("Mount with"),
            _ => d.label(row),
        };
        ui.painter().text(pos2(rect.min.x + 14.0, rect.center().y), Align2::LEFT_CENTER, label, font.clone(), v.text_color());
        match row {
            NRow::Kind | NRow::Tool => {
                // Chips: the kinds, or the tools (those not installed dim).
                let items: Vec<(String, bool, bool)> = if row == NRow::Kind {
                    KINDS.iter().map(|k| (k.label().to_string(), *k == c.kind, true)).collect()
                } else {
                    c.kind.tools().iter().map(|t| (t.label().to_string(), Some(*t) == tool, crate::net::available(*t, c.kind))).collect()
                };
                let mut x = rect.max.x - 10.0;
                for (i, (name, on, there)) in items.iter().enumerate().rev() {
                    let g = ui.painter().layout_no_wrap(name.clone(), FontId::proportional(13.0), Color32::WHITE);
                    let w = g.size().x + 16.0;
                    let chip = Rect::from_min_size(pos2(x - w, rect.center().y - 13.0), egui::vec2(w, 26.0));
                    x -= w + 4.0;
                    let cr = ui.interact(chip, ui.id().with(("net-chip", row as u8, i)), Sense::click());
                    let fill = if *on { v.selection.bg_fill } else if cr.hovered() { v.widgets.hovered.bg_fill } else { v.widgets.inactive.bg_fill };
                    ui.painter().rect(chip, 6.0, fill, Stroke::NONE, StrokeKind::Inside);
                    let fg = if *on { v.selection.stroke.color } else if *there { v.text_color() } else { v.weak_text_color() };
                    ui.painter().text(chip.center(), Align2::CENTER_CENTER, name, FontId::proportional(13.0), fg);
                    let cr = if *there { cr } else { cr.on_hover_text(format!("{} {}", tr("Not installed:"), c.kind.tools()[i].package())) };
                    if cr.clicked() {
                        ev = Some((row, i as isize + 100));
                    }
                }
            }
            NRow::Remember => {
                let b = Rect::from_center_size(pos2(rect.max.x - 26.0, rect.center().y), Vec2::splat(20.0));
                ui.painter().rect_stroke(b, 4.0, Stroke::new(1.5, v.strong_text_color()), StrokeKind::Inside);
                if c.remember {
                    ui.painter().text(b.center(), Align2::CENTER_CENTER, "✔", FontId::proportional(15.0), v.strong_text_color());
                }
            }
            _ => {
                let value = match row {
                    NRow::Name => c.name.clone(),
                    NRow::Address => c.address.clone(),
                    NRow::Path => c.path.clone(),
                    NRow::User => c.user.clone(),
                    NRow::Port => c.port.map(|p| p.to_string()).unwrap_or_default(),
                    NRow::Password => "•".repeat(d.password.chars().count().min(12)),
                    _ => String::new(),
                };
                let hint = match row {
                    NRow::Address if c.kind == Kind::WebDav => "https://cloud.example.org/remote.php/dav/files/me",
                    NRow::Address if c.kind == Kind::Rclone => "gdrive",
                    NRow::Address => "nas.local / 192.168.1.10",
                    NRow::Port => match c.kind {
                        Kind::Smb => "445",
                        Kind::Sftp => "22",
                        Kind::Ftp => "21",
                        _ => "",
                    },
                    _ => "",
                };
                let (text, color) = if value.is_empty() { (hint.to_string(), v.weak_text_color()) } else { (value, v.strong_text_color()) };
                let g = crate::view::elided(ui, &text, font.clone(), color, 300.0, 1);
                ui.painter().galley(pos2(rect.max.x - 14.0 - g.size().x, rect.center().y - g.size().y / 2.0), g, color);
            }
        }
    }
    // No tool for it on the system: what to install.
    if tool.is_none() {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(crate::net::missing(c.kind)).color(v.warn_fg_color));
    } else if tool.is_some_and(Tool::root) {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(tr("Mounted as administrator: sudo asks for its password")).weak());
    }
    ev
}

/// A click in the dialog (step 0: the row itself; 10+: a button; 100+: a
/// chip).
pub fn clicked(app: &mut App, row: NRow, step: isize) {
    let Some(Overlay::Net(d)) = app.overlay.as_mut() else { return };
    match step {
        0 => app.net_activate(row),
        10..=12 => {
            d.row = NRow::Buttons;
            d.button = (step - 10) as usize;
            app.net_activate(NRow::Buttons);
        }
        _ => {
            let i = (step - 100) as usize;
            d.row = row;
            if row == NRow::Kind {
                d.conn.kind = KINDS[i];
                d.conn.tool = None;
            } else if let Some(t) = d.conn.kind.tools().get(i) {
                d.conn.tool = Some(*t);
            }
        }
    }
}
