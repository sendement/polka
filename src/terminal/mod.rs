//! The terminal page: shells in tabs of their own, drawn here from
//! alacritty's terminal core (parser, grid, scrollback, PTY), played with a
//! pad as well as the keyboard.

pub mod app;
pub mod draw;
pub mod keys;
pub mod scheme;

use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};
use crossbeam_channel::{Receiver, Sender};

use crate::config::TermPrefs;

/// Wakes the window and hands terminal events over to the UI thread.
#[derive(Clone)]
pub struct Proxy {
    tx: Sender<Event>,
    ctx: egui::Context,
}

impl EventListener for Proxy {
    fn send_event(&self, e: Event) {
        let _ = self.tx.send(e);
        self.ctx.request_repaint();
    }
}

/// One shell.
pub struct Session {
    pub term: Arc<FairMutex<Term<Proxy>>>,
    tx: EventLoopSender,
    events: Receiver<Event>,
    /// What the shell called itself (OSC title), or the shell's name.
    pub title: String,
    pub pid: u32,
    /// The PTY's controlling side, for the foreground process.
    master: i32,
    pub exited: bool,
    pub cols: usize,
    pub rows: usize,
    /// Bell: flash the tab for a moment.
    pub bell: Option<Instant>,
    /// In a sandbox: where the host side wrote its pid.
    pid_file: Option<PathBuf>,
    /// A resize the host side hasn't heard of yet.
    resize_pending: Option<Instant>,
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(p) = &self.pid_file {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// The program a new shell runs: the chosen one, else the user's own; in a
/// Flatpak sandbox it runs on the host (through `script` there, so it gets a
/// terminal of its own with job control).
fn shell_command(prefs: &TermPrefs, cwd: &Path, pid_file: Option<&Path>, env: &std::collections::HashMap<String, String>) -> tty::Shell {
    let chosen = prefs.shell.clone().filter(|s| !s.is_empty());
    if crate::sandbox::flatpak() {
        let sh = chosen.unwrap_or_else(|| "\"$(getent passwd \"$(id -un)\" | cut -d: -f7)\"".into());
        // Its host pid goes to a file both sides see (to pass on resizes).
        let note = pid_file.map(|p| format!("echo $$ > '{}'; ", p.display())).unwrap_or_default();
        let inner = format!("{note}s={sh}; [ -x \"$s\" ] || s=/bin/sh; command -v script >/dev/null && exec script -qfe /dev/null -c \"exec $s\"; exec \"$s\"");
        let mut args: Vec<String> = vec!["--host".into(), "--watch-bus".into()];
        args.extend(env.iter().map(|(k, v)| format!("--env={k}={v}")));
        args.extend([format!("--directory={}", cwd.display()), "sh".into(), "-c".into(), inner]);
        return tty::Shell::new("flatpak-spawn".into(), args);
    }
    let sh = chosen.or_else(login_shell).unwrap_or_else(|| "/bin/sh".into());
    tty::Shell::new(sh, Vec::new())
}

/// The user's shell as the system has it (passwd), else $SHELL — which a
/// long-running session may still have from before a `chsh`.
pub fn login_shell() -> Option<String> {
    let pw = unsafe { libc::getpwuid(libc::getuid()) };
    let from_pw = (!pw.is_null())
        .then(|| unsafe { std::ffi::CStr::from_ptr((*pw).pw_shell) }.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty() && Path::new(s).exists());
    from_pw.or_else(|| std::env::var("SHELL").ok().filter(|s| !s.is_empty()))
}

pub fn shell_name(prefs: &TermPrefs) -> String {
    let sh = prefs.shell.clone().or_else(login_shell).unwrap_or_else(|| "sh".into());
    Path::new(&sh).file_name().map_or(sh.clone(), |n| n.to_string_lossy().into_owned())
}

fn window_size(cols: usize, rows: usize, cell: (f32, f32)) -> WindowSize {
    WindowSize { num_lines: rows.max(1) as u16, num_cols: cols.max(2) as u16, cell_width: cell.0.round().max(1.0) as u16, cell_height: cell.1.round().max(1.0) as u16 }
}

pub fn term_config(prefs: &TermPrefs) -> TermConfig {
    let shape = match prefs.cursor {
        1 => CursorShape::Beam,
        2 => CursorShape::Underline,
        _ => CursorShape::Block,
    };
    TermConfig { scrolling_history: prefs.scrollback, default_cursor_style: CursorStyle { shape, blinking: false }, ..Default::default() }
}

impl Session {
    /// A shell; `private` keeps what's typed out of its history.
    pub fn spawn(prefs: &TermPrefs, cwd: &Path, ctx: &egui::Context, cols: usize, rows: usize, private: bool) -> std::io::Result<Self> {
        let (tx, events) = crossbeam_channel::unbounded();
        let proxy = Proxy { tx, ctx: ctx.clone() };
        let mut env = std::collections::HashMap::new();
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());
        env.insert("TERM_PROGRAM".to_string(), "polka".to_string());
        if private {
            env.insert("fish_private_mode".to_string(), "1".to_string());
            env.insert("HISTFILE".to_string(), "/dev/null".to_string());
        }
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let pid_file = crate::sandbox::flatpak().then(|| dirs::cache_dir()).flatten().map(|d| {
            let dir = d.join("polka");
            let _ = std::fs::create_dir_all(&dir);
            dir.join(format!("term-{}-{}.pid", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)))
        });
        let opts = tty::Options { shell: Some(shell_command(prefs, cwd, pid_file.as_deref(), &env)), working_directory: Some(cwd.to_path_buf()), drain_on_exit: true, env, ..Default::default() };
        let pty = tty::new(&opts, window_size(cols, rows, (8.0, 16.0)), 0)?;
        let pid = pty.child().id();
        let master = pty.file().as_raw_fd();
        let term = Term::new(term_config(prefs), &TermSize::new(cols.max(2), rows.max(1)), proxy.clone());
        let term = Arc::new(FairMutex::new(term));
        let lp = EventLoop::new(term.clone(), proxy, pty, true, false)?;
        let tx = lp.channel();
        lp.spawn();
        Ok(Self { term, tx, events, title: shell_name(prefs), pid, master, exited: false, cols, rows, bell: None, pid_file, resize_pending: None })
    }

    pub fn write(&self, bytes: impl Into<Vec<u8>>) {
        let b: Vec<u8> = bytes.into();
        if !b.is_empty() {
            let _ = self.tx.send(Msg::Input(b.into()));
        }
    }

    /// Typed input brings the view back to the prompt.
    pub fn input(&self, bytes: impl Into<Vec<u8>>) {
        self.term.lock().scroll_display(Scroll::Bottom);
        self.write(bytes);
    }

    /// Pasted text, bracketed when the program asked for it.
    pub fn paste(&self, text: &str) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        if bracketed {
            // Never let the text end the bracket early.
            let clean = text.replace("\x1b[201~", "");
            self.input(format!("\x1b[200~{clean}\x1b[201~"));
        } else {
            self.input(text);
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize, cell: (f32, f32)) {
        let (cols, rows) = (cols.max(2), rows.max(1));
        if (cols, rows) == (self.cols, self.rows) {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.term.lock().resize(TermSize::new(cols, rows));
        let _ = self.tx.send(Msg::Resize(window_size(cols, rows, cell)));
        if self.pid_file.is_some() {
            self.resize_pending = Some(Instant::now());
        }
    }

    /// Something to finish on a later frame.
    pub fn waiting(&self) -> bool {
        self.resize_pending.is_some()
    }

    /// The host side's pid (sandbox only).
    fn host_pid(&self) -> Option<u32> {
        std::fs::read_to_string(self.pid_file.as_ref()?).ok()?.trim().parse().ok()
    }

    /// Asks the host what runs in the foreground and where the shell is.
    fn host_probe(&self) -> Option<(String, PathBuf)> {
        let pid = self.host_pid()?;
        // script's child is the shell; its terminal's foreground group runs.
        let probe = format!(
            "c=$(cut -d' ' -f1 /proc/{pid}/task/{pid}/children); t=$(ps -o tpgid= -p \"$c\" | tr -d ' '); cat /proc/$t/comm; readlink /proc/$c/cwd"
        );
        let out = std::process::Command::new("flatpak-spawn").args(["--host", "sh", "-c", &probe]).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let mut l = text.lines();
        Some((l.next()?.trim().to_string(), PathBuf::from(l.next()?.trim())))
    }

    pub fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    /// Name of the program in the foreground (htop, nvim…), when it's not
    /// the shell itself.
    pub fn foreground(&self) -> Option<String> {
        let comm = if self.pid_file.is_some() {
            self.host_probe().map(|(c, _)| c)
        } else {
            let pg = unsafe { libc::tcgetpgrp(self.master) };
            (pg > 0).then(|| std::fs::read_to_string(format!("/proc/{pg}/comm")).ok()).flatten().map(|s| s.trim().to_string())
        };
        let shells = ["bash", "zsh", "fish", "sh", "dash", "ksh", "tcsh", "nu", "elvish", "xonsh", "flatpak-spawn", "script"];
        match comm {
            Some(c) if !shells.contains(&c.as_str()) => Some(c),
            // In a sandbox (or a shell that says what runs in its title).
            _ => {
                let first = self.title.split_whitespace().next().unwrap_or("");
                let name = first.rsplit('/').next().unwrap_or(first).to_string();
                (!name.is_empty() && !shells.contains(&name.as_str()) && keys::known_app(&name)).then_some(name)
            }
        }
    }

    /// The shell's working folder (not inside a sandbox: the shell runs on
    /// the host there).
    pub fn cwd(&self) -> Option<PathBuf> {
        if self.pid_file.is_some() {
            return self.host_probe().map(|(_, d)| d).filter(|d| d.is_dir());
        }
        let pg = unsafe { libc::tcgetpgrp(self.master) };
        let pid = if pg > 0 { pg as u32 } else { self.pid };
        std::fs::read_link(format!("/proc/{pid}/cwd")).or_else(|_| std::fs::read_link(format!("/proc/{}/cwd", self.pid))).ok()
    }

    /// Terminal events since the last frame; what needs the app comes back.
    pub fn poll(&mut self, scheme: &scheme::Scheme) -> Vec<Out> {
        let mut out = Vec::new();
        // The host's shell hears of a new size once resizing settles.
        if self.resize_pending.is_some_and(|t| t.elapsed().as_millis() >= 120) {
            self.resize_pending = None;
            if let Some(pid) = self.host_pid() {
                let _ = std::process::Command::new("flatpak-spawn")
                    .args(["--host", "kill", "-WINCH", &pid.to_string()])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .map(|mut c| std::thread::spawn(move || c.wait()));
            }
        }
        while let Ok(e) = self.events.try_recv() {
            match e {
                Event::Title(t) => self.title = t,
                Event::ResetTitle => self.title = String::new(),
                Event::PtyWrite(s) => self.write(s),
                Event::ClipboardStore(_, s) => out.push(Out::Copy(s)),
                Event::ClipboardLoad(_, f) => out.push(Out::PasteRequest(f)),
                Event::ColorRequest(i, f) => {
                    let c = self.term.lock().colors()[i].unwrap_or_else(|| scheme.rgb(i));
                    self.write(f(c));
                }
                Event::TextAreaSizeRequest(f) => self.write(f(window_size(self.cols, self.rows, (8.0, 16.0)))),
                Event::Bell => self.bell = Some(Instant::now()),
                Event::ChildExit(_) | Event::Exit => self.exited = true,
                _ => {}
            }
        }
        out
    }
}

/// What a session asks of the app.
pub enum Out {
    Copy(String),
    PasteRequest(Arc<dyn Fn(&str) -> String + Sync + Send + 'static>),
}

/// The page in a tab.
pub struct Terminal {
    pub shown: bool,
    pub sessions: Vec<Session>,
    pub active: usize,
    /// Choosing text to copy with the pad (alacritty's vi mode).
    pub selecting: bool,
    /// ○ is down since (held: leave the page; tapped: Esc).
    pub back_down: Option<Instant>,
    /// Cell size and grid of the last frame.
    pub cell: (f32, f32),
    pub grid: (usize, usize),
}

impl Terminal {
    pub fn new() -> Self {
        Self { shown: true, sessions: Vec::new(), active: 0, selecting: false, back_down: None, cell: (8.0, 16.0), grid: (80, 24) }
    }

    pub fn cur(&self) -> Option<&Session> {
        self.sessions.get(self.active)
    }

    pub fn cur_mut(&mut self) -> Option<&mut Session> {
        self.sessions.get_mut(self.active)
    }

    pub fn open(&mut self, prefs: &TermPrefs, cwd: &Path, ctx: &egui::Context) -> std::io::Result<()> {
        let s = Session::spawn(prefs, cwd, ctx, self.grid.0, self.grid.1, false)?;
        self.sessions.push(s);
        self.active = self.sessions.len() - 1;
        self.selecting = false;
        Ok(())
    }

    pub fn close(&mut self, i: usize) {
        if i < self.sessions.len() {
            self.sessions.remove(i);
            if self.active >= i && self.active > 0 {
                self.active -= 1;
            }
            self.active = self.active.min(self.sessions.len().saturating_sub(1));
            self.selecting = false;
        }
    }
}

impl Default for Terminal {
    fn default() -> Self {
        Self::new()
    }
}

/// Saved shell history, newest first: the shell's own file merged with what
/// was run from here.
pub fn shell_history(prefs: &TermPrefs, limit: usize) -> Vec<String> {
    let home = dirs::home_dir().unwrap_or_default();
    let data = dirs::data_dir().unwrap_or_else(|| home.join(".local/share"));
    let mut lines: Vec<String> = Vec::new();
    let name = shell_name(prefs);
    let fish = data.join("fish/fish_history");
    let zsh = std::env::var_os("HISTFILE").map(PathBuf::from).filter(|_| name == "zsh").unwrap_or_else(|| home.join(".zsh_history"));
    let bash = home.join(".bash_history");
    let order: Vec<(&str, PathBuf)> = match name.as_str() {
        "fish" => vec![("fish", fish)],
        "zsh" => vec![("zsh", zsh)],
        "bash" => vec![("bash", bash)],
        _ => vec![("fish", fish), ("zsh", zsh), ("bash", bash)],
    };
    for (kind, p) in order {
        let Ok(bytes) = std::fs::read(&p) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        match kind {
            "fish" => {
                for l in text.lines() {
                    if let Some(c) = l.strip_prefix("- cmd: ") {
                        lines.push(c.replace("\\n", "\n").replace("\\\\", "\\"));
                    }
                }
            }
            "zsh" => {
                for l in text.lines() {
                    let c = if l.starts_with(": ") { l.split_once(';').map_or(l, |(_, c)| c) } else { l };
                    lines.push(c.to_string());
                }
            }
            _ => lines.extend(text.lines().filter(|l| !l.starts_with('#')).map(str::to_string)),
        }
    }
    lines.extend(own_history());
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for l in lines.into_iter().rev() {
        let l = l.trim().to_string();
        if !l.is_empty() && !l.contains('\n') && seen.insert(l.clone()) {
            out.push(l);
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}

fn own_history_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("polka/terminal-history"))
}

fn own_history() -> Vec<String> {
    own_history_path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default()
}

/// Remembers a command run from here (the shell keeps its own too).
pub fn remember(cmd: &str) {
    let Some(p) = own_history_path() else { return };
    let mut v = own_history();
    v.retain(|l| l != cmd);
    v.push(cmd.to_string());
    let start = v.len().saturating_sub(500);
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, v[start..].join("\n") + "\n");
}

/// Shells listed in /etc/shells (the host's inside a sandbox).
pub fn shells() -> Vec<String> {
    let f = if crate::sandbox::flatpak() { "/run/host/etc/shells" } else { "/etc/shells" };
    let mut v: Vec<String> = std::fs::read_to_string(f).unwrap_or_default().lines().map(str::trim).filter(|l| l.starts_with('/')).map(str::to_string).collect();
    // /bin/x and /usr/bin/x are the same shell on merged-usr systems.
    v.sort_by_key(|s| (Path::new(s).file_name().map(|n| n.to_owned()), s.starts_with("/usr")));
    v.dedup_by(|a, b| Path::new(a).file_name() == Path::new(b).file_name());
    v
}

/// Installed programs among `names` (on the host inside a sandbox).
pub fn installed(names: &[&'static str]) -> Vec<&'static str> {
    let dirs: Vec<PathBuf> = if crate::sandbox::flatpak() {
        ["/run/host/usr/bin", "/run/host/usr/local/bin"].iter().map(PathBuf::from).chain(dirs::home_dir().map(|h| h.join(".local/bin"))).collect()
    } else {
        std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default()
    };
    names.iter().copied().filter(|n| dirs.iter().any(|d| d.join(n).is_file())).collect()
}

pub fn cursor_names() -> [&'static str; 3] {
    [crate::i18n::tr("Block"), crate::i18n::tr("Bar"), crate::i18n::tr("Underline")]
}
