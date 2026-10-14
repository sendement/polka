//! Drag and drop with other programs on Wayland (winit has none): a second
//! queue on the window's own connection listens to the seat's data device
//! for drops coming in, and starts a drag when files are pulled out of the
//! window. The app sees it all as pointer moves and a release, so dropping
//! works through the same code as inside the window.

use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::path::PathBuf;
use std::sync::Mutex;

use crossbeam_channel::{Receiver, Sender};
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, event_created_child};

const URIS: &str = "text/uri-list";

/// What happened, in the window's logical coordinates.
#[derive(Debug)]
pub enum Ev {
    /// Something is dragged over the window: files from elsewhere (None:
    /// it's our own drag coming back).
    Enter { x: f32, y: f32, files: Option<Vec<PathBuf>> },
    Motion { x: f32, y: f32 },
    Leave,
    /// Let go (where the last motion was).
    Drop,
    /// Our own drag ended (dropped somewhere or given up).
    OutEnded,
}

enum Cmd {
    Start(Vec<PathBuf>),
}

pub struct Dnd {
    cmd: Sender<Cmd>,
    pub events: Receiver<Ev>,
}

impl Dnd {
    /// Hook into the window's Wayland connection (None elsewhere).
    pub fn new(display: *mut std::ffi::c_void, surface: *mut std::ffi::c_void, ctx: egui::Context) -> Option<Self> {
        let backend = unsafe { wayland_backend::client::Backend::from_foreign_display(display.cast()) };
        let conn = Connection::from_backend(backend);
        let id = unsafe { wayland_backend::client::ObjectId::from_ptr(WlSurface::interface(), surface.cast()) }.ok()?;
        let surface = WlSurface::from_id(&conn, id).ok()?;
        let mut queue: EventQueue<State> = conn.new_event_queue();
        let qh = queue.handle();
        let (ev_tx, events) = crossbeam_channel::unbounded();
        let (cmd, cmd_rx) = crossbeam_channel::unbounded();
        let mut st = State { qh: qh.clone(), surface, ev: ev_tx, ctx, seat: None, manager: None, device: None, pointer: None, serial: 0, offer: None, own: None, out_files: Vec::new() };
        conn.display().get_registry(&qh, ());
        queue.roundtrip(&mut st).ok()?;
        queue.roundtrip(&mut st).ok()?;
        st.device.as_ref()?;
        std::thread::Builder::new().name("polka-dnd".into()).spawn(move || run(conn, queue, st, cmd_rx)).ok()?;
        Some(Self { cmd, events })
    }

    /// Hand the files to the system's drag (they've left the window).
    pub fn start(&self, files: Vec<PathBuf>) {
        let _ = self.cmd.send(Cmd::Start(files));
    }
    /// For tests: events fed by hand, and the files of drags started.
    #[cfg(test)]
    pub fn fake() -> (Self, Sender<Ev>, Receiver<Vec<PathBuf>>) {
        let (ev_tx, events) = crossbeam_channel::unbounded();
        let (cmd, cmd_rx) = crossbeam_channel::unbounded::<Cmd>();
        let (out_tx, out) = crossbeam_channel::unbounded();
        std::thread::spawn(move || {
            while let Ok(Cmd::Start(f)) = cmd_rx.recv() {
                let _ = out_tx.send(f);
            }
        });
        (Self { cmd, events }, ev_tx, out)
    }
}

fn run(conn: Connection, mut queue: EventQueue<State>, mut st: State, cmds: Receiver<Cmd>) {
    loop {
        if queue.dispatch_pending(&mut st).is_err() {
            return;
        }
        while let Ok(c) = cmds.try_recv() {
            match c {
                Cmd::Start(files) => st.start(files),
            }
        }
        let _ = conn.flush();
        let Some(guard) = queue.prepare_read() else { continue };
        let raw = std::os::fd::AsRawFd::as_raw_fd(&guard.connection_fd());
        let mut p = [libc::pollfd { fd: raw, events: libc::POLLIN, revents: 0 }];
        let n = unsafe { libc::poll(p.as_mut_ptr(), 1, 30) };
        if n > 0 {
            if guard.read().is_err() {
                return;
            }
        } else {
            drop(guard);
        }
    }
}

struct State {
    qh: QueueHandle<State>,
    surface: WlSurface,
    ev: Sender<Ev>,
    ctx: egui::Context,
    seat: Option<WlSeat>,
    manager: Option<WlDataDeviceManager>,
    device: Option<WlDataDevice>,
    pointer: Option<WlPointer>,
    /// The last button press on our window (a drag must name it).
    serial: u32,
    /// The offer over the window now.
    offer: Option<WlDataOffer>,
    /// Our own drag, while it lasts.
    own: Option<WlDataSource>,
    out_files: Vec<PathBuf>,
}

impl State {
    fn send(&self, e: Ev) {
        let _ = self.ev.send(e);
        self.ctx.request_repaint();
    }

    fn start(&mut self, files: Vec<PathBuf>) {
        let (Some(m), Some(d)) = (&self.manager, &self.device) else { return };
        if self.own.is_some() || files.is_empty() {
            return;
        }
        let src = m.create_data_source(&self.qh, ());
        src.offer(URIS.into());
        src.offer("text/plain;charset=utf-8".into());
        if src.version() >= 3 {
            src.set_actions(DndAction::Copy | DndAction::Move);
        }
        d.start_drag(Some(&src), &self.surface, None, self.serial);
        self.out_files = files;
        self.own = Some(src);
    }

    fn end_own(&mut self) {
        if let Some(s) = self.own.take() {
            s.destroy();
            self.send(Ev::OutEnded);
        }
    }
}

/// file:// URIs for a uri-list (percent-encoded).
fn uri_list(files: &[PathBuf]) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::new();
    for f in files {
        out += "file://";
        for &b in f.as_os_str().as_bytes() {
            if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
        out += "\r\n";
    }
    out
}

/// Local paths in a uri-list.
pub fn parse_uris(text: &str) -> Vec<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("file://"))
        // file://host/path: skip the host.
        .map(|l| if l.starts_with('/') { l } else { l.find('/').map_or(l, |i| &l[i..]) })
        .map(|l| {
            let b = l.as_bytes();
            let mut out = Vec::with_capacity(b.len());
            let mut i = 0;
            while i < b.len() {
                if b[i] == b'%' && i + 2 < b.len() {
                    if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                        out.push(v);
                        i += 3;
                        continue;
                    }
                }
                out.push(b[i]);
                i += 1;
            }
            PathBuf::from(std::ffi::OsString::from_vec(out))
        })
        .collect()
}

/// Reads what an offer holds as a uri-list (the giver writes into a pipe).
fn receive(conn: &Connection, offer: &WlDataOffer) -> Option<Vec<PathBuf>> {
    let (r, w) = std::io::pipe().ok()?;
    offer.receive(URIS.into(), w.as_fd());
    drop(w);
    let _ = conn.flush();
    let mut text = String::new();
    let mut r = r;
    // A slow giver shouldn't hang the drag forever.
    let (tx, rx) = crossbeam_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = r.read_to_string(&mut text);
        let _ = tx.send(text);
    });
    let text = rx.recv_timeout(std::time::Duration::from_secs(2)).ok()?;
    Some(parse_uris(&text))
}

impl Dispatch<WlRegistry, ()> for State {
    fn event(st: &mut Self, reg: &WlRegistry, e: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = e {
            match interface.as_str() {
                "wl_seat" if st.seat.is_none() => st.seat = Some(reg.bind::<WlSeat, _, _>(name, version.min(5), qh, ())),
                "wl_data_device_manager" => st.manager = Some(reg.bind::<WlDataDeviceManager, _, _>(name, version.min(3), qh, ())),
                _ => {}
            }
            if let (Some(seat), Some(m), None) = (&st.seat, &st.manager, &st.device) {
                st.device = Some(m.get_data_device(seat, qh, ()));
            }
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(st: &mut Self, seat: &WlSeat, e: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(c) } = e {
            if c.contains(wl_seat::Capability::Pointer) && st.pointer.is_none() {
                st.pointer = Some(seat.get_pointer(qh, ()));
            }
        }
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(st: &mut Self, _: &WlPointer, e: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_pointer::Event::Button { serial, state: WEnum::Value(wl_pointer::ButtonState::Pressed), .. } = e {
            st.serial = serial;
        }
    }
}

impl Dispatch<WlDataDeviceManager, ()> for State {
    fn event(_: &mut Self, _: &WlDataDeviceManager, _: <WlDataDeviceManager as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

/// Kinds of data an offer has.
type Mimes = Mutex<Vec<String>>;

impl Dispatch<WlDataDevice, ()> for State {
    fn event(st: &mut Self, _: &WlDataDevice, e: wl_data_device::Event, _: &(), conn: &Connection, _: &QueueHandle<Self>) {
        match e {
            wl_data_device::Event::Enter { serial, surface, x, y, id } => {
                if surface != st.surface {
                    return;
                }
                let (x, y) = (x as f32, y as f32);
                let Some(offer) = id else { return };
                let mimes: Vec<String> = offer.data::<Mimes>().map(|m| m.lock().unwrap().clone()).unwrap_or_default();
                if !mimes.iter().any(|m| m == URIS) {
                    offer.accept(serial, None);
                    return;
                }
                offer.accept(serial, Some(URIS.into()));
                if offer.version() >= 3 {
                    offer.set_actions(DndAction::Copy | DndAction::Move, DndAction::Copy);
                }
                let files = if st.own.is_some() { None } else { receive(conn, &offer).filter(|v| !v.is_empty()) };
                if st.own.is_none() && files.is_none() {
                    offer.accept(serial, None);
                    return;
                }
                st.offer = Some(offer);
                st.send(Ev::Enter { x, y, files });
            }
            wl_data_device::Event::Motion { x, y, .. } => {
                if st.offer.is_some() {
                    st.send(Ev::Motion { x: x as f32, y: y as f32 });
                }
            }
            wl_data_device::Event::Leave => {
                if let Some(o) = st.offer.take() {
                    o.destroy();
                    st.send(Ev::Leave);
                }
            }
            wl_data_device::Event::Drop => {
                if let Some(o) = st.offer.take() {
                    // Where it was let go: the last motion stands.
                    st.send(Ev::Drop);
                    if o.version() >= 3 {
                        o.finish();
                    }
                    o.destroy();
                }
            }
            _ => {}
        }
    }

    event_created_child!(State, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, Mimes::default()),
    ]);
}

impl Dispatch<WlDataOffer, Mimes> for State {
    fn event(_: &mut Self, _: &WlDataOffer, e: wl_data_offer::Event, mimes: &Mimes, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_data_offer::Event::Offer { mime_type } = e {
            mimes.lock().unwrap().push(mime_type);
        }
    }
}

impl Dispatch<WlDataSource, ()> for State {
    fn event(st: &mut Self, _: &WlDataSource, e: wl_data_source::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match e {
            wl_data_source::Event::Send { mime_type, fd } => {
                let text = if mime_type == URIS {
                    uri_list(&st.out_files)
                } else {
                    st.out_files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n")
                };
                let mut f = std::fs::File::from(OwnedFd::from(fd));
                let _ = f.write_all(text.as_bytes());
            }
            wl_data_source::Event::Cancelled | wl_data_source::Event::DndFinished => st.end_own(),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_round_trip() {
        let files = vec![PathBuf::from("/home/u/Мои файлы/a b%.txt"), PathBuf::from("/tmp/x")];
        assert_eq!(parse_uris(&uri_list(&files)), files);
        assert_eq!(parse_uris("# comment\r\nfile://localhost/tmp/y\r\nhttp://x/z\r\n"), vec![PathBuf::from("/tmp/y")]);
    }
}
