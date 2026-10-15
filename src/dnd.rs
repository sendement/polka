//! Drag and drop with other programs, and the clipboard, on Wayland: one
//! data device on the window's own connection (a second queue), since some
//! compositors (Hyprland) talk to only one device per client. Drops coming
//! in and our own drag coming back become pointer moves and a release for
//! egui, so they work through the same code as inside the window; files
//! pulled out of the window start the system's drag, with their picture.

use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use crossbeam_channel::{Receiver, Sender};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_keyboard::{self, WlKeyboard};
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::{self, WlSurface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, event_created_child};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;

const URIS: &str = "text/uri-list";
/// Text kinds, the best first.
const TEXT: [&str; 5] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "STRING", "TEXT"];

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

/// A picture to drag along: width and height in pixels, BGRA premultiplied
/// (Wayland's ARGB8888), and its scale.
pub struct Icon {
    pub w: u32,
    pub h: u32,
    pub pixels: Vec<u8>,
    pub scale: i32,
}

enum Cmd {
    Start(Vec<PathBuf>, Option<Icon>),
    GetText(Sender<Option<String>>),
    SetText(String),
    /// Raise the window with another program's activation token.
    Activate(String),
}

pub struct Dnd {
    cmd: Sender<Cmd>,
    pub events: Receiver<Ev>,
}

/// Wakes the Wayland thread for a command.
struct Waker {
    tx: Sender<Cmd>,
    pipe: Mutex<std::io::PipeWriter>,
}

impl Waker {
    fn send(&self, c: Cmd) {
        if self.tx.send(c).is_ok() {
            if let Ok(mut p) = self.pipe.lock() {
                let _ = p.write_all(b"x");
            }
        }
    }
}

/// Whether the compositor offers xdg-activation.
static ACTIVATION: OnceLock<bool> = OnceLock::new();

/// The running one, for the clipboard functions egui-winit calls.
static WAKER: OnceLock<Waker> = OnceLock::new();

/// The clipboard's text (egui-winit's get).
pub fn clip_get() -> Option<String> {
    let w = WAKER.get()?;
    let (tx, rx) = crossbeam_channel::bounded(1);
    w.send(Cmd::GetText(tx));
    rx.recv_timeout(std::time::Duration::from_millis(1500)).ok().flatten()
}

/// Puts text on the clipboard (egui-winit's set).
pub fn clip_set(text: String) {
    if let Some(w) = WAKER.get() {
        w.send(Cmd::SetText(text));
    }
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
        let (wake_r, wake_w) = std::io::pipe().ok()?;
        let mut st = State {
            qh: qh.clone(),
            surface,
            ev: ev_tx,
            ctx,
            seat: None,
            activation: None,
            manager: None,
            device: None,
            pointer: None,
            keyboard: None,
            compositor: None,
            shm: None,
            serial: 0,
            offer: None,
            selection: None,
            own: None,
            out_files: Vec::new(),
            icon: None,
            clip: None,
        };
        conn.display().get_registry(&qh, ());
        queue.roundtrip(&mut st).ok()?;
        queue.roundtrip(&mut st).ok()?;
        st.device.as_ref()?;
        let _ = ACTIVATION.set(st.activation.is_some());
        let _ = WAKER.set(Waker { tx: cmd.clone(), pipe: Mutex::new(wake_w) });
        std::thread::Builder::new().name("polka-dnd".into()).spawn(move || run(conn, queue, st, cmd_rx, wake_r)).ok()?;
        Some(Self { cmd, events })
    }

    /// Hand the files to the system's drag (they've left the window).
    pub fn start(&self, files: Vec<PathBuf>, icon: Option<Icon>) {
        match WAKER.get() {
            Some(w) => w.send(Cmd::Start(files, icon)),
            None => {
                let _ = self.cmd.send(Cmd::Start(files, icon));
            }
        }
    }

    /// Raise the window with an activation token another program handed
    /// over (false: the compositor has no xdg-activation).
    pub fn activate(&self, token: String) -> bool {
        let Some(w) = WAKER.get() else { return false };
        if !ACTIVATION.get().copied().unwrap_or(false) {
            return false;
        }
        w.send(Cmd::Activate(token));
        true
    }

    /// For tests: events fed by hand, and the files of drags started.
    #[cfg(test)]
    pub fn fake() -> (Self, Sender<Ev>, Receiver<Vec<PathBuf>>) {
        let (ev_tx, events) = crossbeam_channel::unbounded();
        let (cmd, cmd_rx) = crossbeam_channel::unbounded::<Cmd>();
        let (out_tx, out) = crossbeam_channel::unbounded();
        std::thread::spawn(move || {
            while let Ok(c) = cmd_rx.recv() {
                if let Cmd::Start(f, _) = c {
                    let _ = out_tx.send(f);
                }
            }
        });
        (Self { cmd, events }, ev_tx, out)
    }
}

fn run(conn: Connection, mut queue: EventQueue<State>, mut st: State, cmds: Receiver<Cmd>, mut wake: std::io::PipeReader) {
    loop {
        if queue.dispatch_pending(&mut st).is_err() {
            return;
        }
        while let Ok(c) = cmds.try_recv() {
            match c {
                Cmd::Start(files, icon) => st.start(files, icon),
                Cmd::GetText(reply) => st.get_text(&conn, reply),
                Cmd::SetText(t) => st.set_text(t),
                Cmd::Activate(token) => {
                    if let Some(a) = &st.activation {
                        a.activate(token, &st.surface);
                    }
                }
            }
        }
        let _ = conn.flush();
        let Some(guard) = queue.prepare_read() else { continue };
        let raw = guard.connection_fd().as_raw_fd();
        let mut p = [libc::pollfd { fd: raw, events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: wake.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
        let n = unsafe { libc::poll(p.as_mut_ptr(), 2, 200) };
        if n > 0 && p[1].revents != 0 {
            let mut b = [0u8; 64];
            let _ = wake.read(&mut b);
        }
        if n > 0 && p[0].revents != 0 {
            if guard.read().is_err() {
                return;
            }
        } else {
            drop(guard);
        }
    }
}

/// A data source: our drag, or text we put on the clipboard.
enum Source {
    Drag,
    Clip,
}

struct State {
    qh: QueueHandle<State>,
    surface: WlSurface,
    ev: Sender<Ev>,
    ctx: egui::Context,
    seat: Option<WlSeat>,
    activation: Option<XdgActivationV1>,
    manager: Option<WlDataDeviceManager>,
    device: Option<WlDataDevice>,
    pointer: Option<WlPointer>,
    keyboard: Option<WlKeyboard>,
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    /// The last input on our window (drags and the clipboard must name one).
    serial: u32,
    /// The offer dragged over the window now, and the clipboard's.
    offer: Option<WlDataOffer>,
    selection: Option<WlDataOffer>,
    /// Our own drag, while it lasts, and its picture.
    own: Option<WlDataSource>,
    out_files: Vec<PathBuf>,
    icon: Option<(WlSurface, WlBuffer)>,
    /// Text we put on the clipboard.
    clip: Option<(WlDataSource, String)>,
}

impl State {
    fn send(&self, e: Ev) {
        let _ = self.ev.send(e);
        self.ctx.request_repaint();
    }

    fn start(&mut self, files: Vec<PathBuf>, icon: Option<Icon>) {
        let (Some(m), Some(d)) = (&self.manager, &self.device) else { return };
        if self.own.is_some() || files.is_empty() {
            return;
        }
        let src = m.create_data_source(&self.qh, Source::Drag);
        src.offer(URIS.into());
        src.offer("text/plain;charset=utf-8".into());
        if src.version() >= 3 {
            src.set_actions(DndAction::Copy | DndAction::Move);
        }
        let pic = icon.and_then(|i| self.icon_surface(i));
        d.start_drag(Some(&src), &self.surface, pic.as_ref().map(|(s, _)| s), self.serial);
        if let Some((s, _)) = &pic {
            s.commit();
        }
        self.icon = pic;
        self.out_files = files;
        self.own = Some(src);
    }

    /// The picture under the pointer while dragging out: a surface with an
    /// shm buffer.
    fn icon_surface(&self, i: Icon) -> Option<(WlSurface, WlBuffer)> {
        let (Some(c), Some(shm)) = (&self.compositor, &self.shm) else { return None };
        let size = (i.w * i.h * 4) as usize;
        let fd = unsafe { libc::memfd_create(c"polka-drag".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return None;
        }
        let mut file = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(fd) };
        file.write_all(&i.pixels).ok()?;
        let pool = shm.create_pool(file.as_fd(), size as i32, &self.qh, ());
        let buf = pool.create_buffer(0, i.w as i32, i.h as i32, (i.w * 4) as i32, wl_shm::Format::Argb8888, &self.qh, ());
        pool.destroy();
        let s = c.create_surface(&self.qh, ());
        if s.version() >= 3 {
            s.set_buffer_scale(i.scale.max(1));
        }
        s.attach(Some(&buf), 0, 0);
        s.damage(0, 0, i32::MAX, i32::MAX);
        Some((s, buf))
    }

    /// The drag's picture on or off (attached buffer or none).
    fn show_icon(&self, on: bool) {
        if let Some((s, b)) = &self.icon {
            s.attach(on.then_some(b), 0, 0);
            if on {
                s.damage(0, 0, i32::MAX, i32::MAX);
            }
            s.commit();
        }
    }

    fn end_own(&mut self) {
        if let Some(s) = self.own.take() {
            s.destroy();
            if let Some((s, b)) = self.icon.take() {
                s.destroy();
                b.destroy();
            }
            self.send(Ev::OutEnded);
        }
    }

    fn get_text(&self, conn: &Connection, reply: Sender<Option<String>>) {
        // Ours: no need to ask ourselves through a pipe.
        if let Some((_, t)) = &self.clip {
            let _ = reply.send(Some(t.clone()));
            return;
        }
        let Some(offer) = &self.selection else {
            let _ = reply.send(None);
            return;
        };
        let mimes: Vec<String> = offer.data::<Mimes>().map(|m| m.lock().unwrap().clone()).unwrap_or_default();
        let Some(mime) = TEXT.iter().find(|t| mimes.iter().any(|m| m == *t)) else {
            let _ = reply.send(None);
            return;
        };
        let Ok((mut r, w)) = std::io::pipe() else {
            let _ = reply.send(None);
            return;
        };
        offer.receive(mime.to_string(), w.as_fd());
        drop(w);
        let _ = conn.flush();
        std::thread::spawn(move || {
            let mut text = String::new();
            let ok = r.read_to_string(&mut text).is_ok();
            let _ = reply.send(ok.then_some(text));
        });
    }

    fn set_text(&mut self, text: String) {
        let (Some(m), Some(d)) = (&self.manager, &self.device) else { return };
        let src = m.create_data_source(&self.qh, Source::Clip);
        for t in TEXT {
            src.offer(t.into());
        }
        d.set_selection(Some(&src), self.serial);
        if let Some((old, _)) = self.clip.replace((src, text)) {
            old.destroy();
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
                out += &format!("%{b:02X}");
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
    let mut r = r;
    // A slow giver shouldn't hang the drag forever.
    let (tx, rx) = crossbeam_channel::bounded(1);
    std::thread::spawn(move || {
        let mut text = String::new();
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
                "wl_compositor" => st.compositor = Some(reg.bind::<WlCompositor, _, _>(name, version.min(4), qh, ())),
                "wl_shm" => st.shm = Some(reg.bind::<WlShm, _, _>(name, 1, qh, ())),
                "xdg_activation_v1" => st.activation = Some(reg.bind::<XdgActivationV1, _, _>(name, 1, qh, ())),
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
            if c.contains(wl_seat::Capability::Keyboard) && st.keyboard.is_none() {
                st.keyboard = Some(seat.get_keyboard(qh, ()));
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

impl Dispatch<WlKeyboard, ()> for State {
    fn event(st: &mut Self, _: &WlKeyboard, e: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        // Ctrl+C is a key press: its serial lets us take the clipboard.
        match e {
            wl_keyboard::Event::Key { serial, .. } | wl_keyboard::Event::Enter { serial, .. } => st.serial = serial,
            _ => {}
        }
    }
}

macro_rules! ignore_events {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for State {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_events!(WlDataDeviceManager, WlCompositor, WlShm, WlShmPool, XdgActivationV1);

impl Dispatch<WlSurface, ()> for State {
    fn event(_: &mut Self, _: &WlSurface, _: wl_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlBuffer, ()> for State {
    fn event(_: &mut Self, _: &WlBuffer, _: wl_buffer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
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
                // Ours back over the window: the window draws its own stack.
                if st.own.is_some() {
                    st.show_icon(false);
                }
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
                    if st.own.is_some() {
                        st.show_icon(true);
                    }
                    st.send(Ev::Leave);
                }
            }
            wl_data_device::Event::Drop => {
                if let Some(o) = st.offer.take() {
                    st.send(Ev::Drop);
                    if o.version() >= 3 {
                        o.finish();
                    }
                    o.destroy();
                }
            }
            wl_data_device::Event::Selection { id } => {
                if let Some(old) = std::mem::replace(&mut st.selection, id) {
                    old.destroy();
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

impl Dispatch<WlDataSource, Source> for State {
    fn event(st: &mut Self, src: &WlDataSource, e: wl_data_source::Event, kind: &Source, _: &Connection, _: &QueueHandle<Self>) {
        match (kind, e) {
            (Source::Drag, wl_data_source::Event::Send { mime_type, fd }) => {
                let text = if mime_type == URIS {
                    uri_list(&st.out_files)
                } else {
                    st.out_files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n")
                };
                let mut f = std::fs::File::from(OwnedFd::from(fd));
                let _ = f.write_all(text.as_bytes());
            }
            (Source::Drag, wl_data_source::Event::Cancelled | wl_data_source::Event::DndFinished) => st.end_own(),
            (Source::Clip, wl_data_source::Event::Send { fd, .. }) => {
                if let Some((_, t)) = &st.clip {
                    let mut f = std::fs::File::from(OwnedFd::from(fd));
                    let _ = f.write_all(t.as_bytes());
                }
            }
            // Someone else took the clipboard.
            (Source::Clip, wl_data_source::Event::Cancelled) => {
                if st.clip.as_ref().is_some_and(|(s, _)| s == src) {
                    st.clip = None;
                }
                src.destroy();
            }
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
