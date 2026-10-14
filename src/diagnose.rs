//! `polka --diagnose`: what the app sees around it, for bug reports (and
//! for checking a sandboxed build without looking at its window).

use std::process::Command;

pub fn run() {
    let env = |k: &str| std::env::var(k).unwrap_or_else(|_| "-".into());
    println!("polka {}", env!("CARGO_PKG_VERSION"));
    println!("flatpak: {}", crate::sandbox::flatpak());
    for k in ["XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_DATA_DIRS", "XDG_CURRENT_DESKTOP", "WAYLAND_DISPLAY", "DISPLAY", "LANG"] {
        println!("{k}: {}", env(k));
    }
    println!("language: {}", crate::i18n::current());
    let (ctype, codeset) = unsafe {
        let l = libc::setlocale(libc::LC_CTYPE, std::ptr::null());
        let c = libc::nl_langinfo(libc::CODESET);
        let s = |p: *const libc::c_char| if p.is_null() { "-".to_string() } else { std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned() };
        (s(l), s(c))
    };
    println!("character set: {ctype} ({codeset}); LANG={} LC_ALL={} LC_CTYPE={}", env("LANG"), env("LC_ALL"), env("LC_CTYPE"));
    println!("file system root: {}", crate::sandbox::root().display());
    println!("icon theme: {}", crate::icons::detect_theme().unwrap_or_else(|| "-".into()));
    let themes = crate::icons::installed_themes();
    println!("icon themes: {} ({})", themes.len(), themes.iter().map(|t| t.id.as_str()).collect::<Vec<_>>().join(", "));
    for r in crate::trashbin::roots() {
        println!("trash: {}", r.display());
    }
    println!("trash items: {}", crate::trashbin::count());
    for m in crate::usage::mounts() {
        println!("mount: {} (read at {}) {} {}/{} bytes", m.host.display(), m.path.display(), m.fstype, m.used, m.total);
    }
    for d in crate::drives::list() {
        println!("drive: {} {:?} {} at {:?} removable={} udisks={}", d.dev, d.label, d.fstype, d.mount, d.removable, d.udisks.is_some());
    }
    for r in crate::steam::roots() {
        println!("steam: {}", r.display());
    }
    for v in [crate::steam::View::Games, crate::steam::View::Prefixes] {
        let names: Vec<String> = crate::steam::list(v).unwrap_or_default().into_iter().map(|e| e.name).collect();
        println!("steam {}: {}", crate::steam::view_name(v), names.join(" | "));
    }
    for e in crate::steam::list(crate::steam::View::Games).unwrap_or_default() {
        let p = crate::steam::resolve(crate::steam::View::Games, &e.name);
        let exe = p.as_deref().and_then(|p| crate::steam::icon(p)).and_then(|(x, _)| x);
        println!("steam icon: {} ← {}", e.name, exe.map_or("-".into(), |x| x.strip_prefix(p.unwrap_or_default()).map(|r| r.display().to_string()).unwrap_or_default()));
    }
    let host = crate::sandbox::host(Command::new("true")).status();
    println!("run on host: {}", match host {
        Ok(s) => s.success().to_string(),
        Err(e) => e.to_string(),
    });
    for tool in ["7z", "bsdtar", "ffmpeg", "curl", "gdbus", "lsblk"] {
        let ok = Command::new(tool).arg("--help").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok();
        println!("tool {tool}: {}", if ok { "yes" } else { "missing" });
    }
    terminal();
    match gilrs::Gilrs::new() {
        Ok(mut g) => {
            // Devices come in as events: give them a moment.
            let until = std::time::Instant::now() + std::time::Duration::from_millis(500);
            while std::time::Instant::now() < until {
                g.next_event_blocking(Some(std::time::Duration::from_millis(50)));
            }
            let pads: Vec<String> = g.gamepads().map(|(_, p)| if crate::input::not_a_pad(&p) { format!("{} (ignored)", p.name()) } else { p.name().to_string() }).collect();
            println!("gamepads: {}", if pads.is_empty() { "-".into() } else { pads.join(", ") });
        }
        Err(e) => println!("gamepads: unavailable ({e})"),
    }
}

/// A real shell through the terminal page's own code: output, size after a
/// resize, and Ctrl+C.
fn terminal() {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line, Point};
    let prefs = crate::config::Config::load().terminal;
    let ctx = egui::Context::default();
    let home = dirs::home_dir().unwrap_or_else(|| "/".into());
    let mut s = match crate::terminal::Session::spawn(&prefs, &home, &ctx, 80, 24, true) {
        Ok(s) => s,
        Err(e) => return println!("terminal: failed to start ({e})"),
    };
    let screen = |s: &crate::terminal::Session| {
        let t = s.term.lock();
        let mut out = String::new();
        for l in 0..t.screen_lines() as i32 {
            out.extend((0..t.columns()).map(|c| t.grid()[Point::new(Line(l), Column(c))].c));
            out.push('\n');
        }
        out
    };
    let scheme = crate::terminal::scheme::resolve(&prefs.scheme, true);
    // The app answers the shell's questions (fish asks the terminal what it
    // can do before reading anything) every frame; here, while waiting.
    let pause = |s: &mut crate::terminal::Session, ms: u64| {
        for _ in 0..ms / 25 {
            s.poll(&scheme);
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    };
    // Typed commands show once as typed, then their output.
    let wait = |s: &mut crate::terminal::Session, what: &str, n: usize| {
        for _ in 0..200 {
            s.poll(&scheme);
            if screen(s).matches(what).count() >= n {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        false
    };
    pause(&mut s, 800);
    s.input(b"echo polka-term-ok\r".to_vec());
    let out = wait(&mut s, "polka-term-ok", 2);
    s.resize(100, 30, (8.0, 16.0));
    pause(&mut s, 300);
    s.input(b"stty size\r".to_vec());
    let size = wait(&mut s, "30 100", 1);
    s.input(b"sleep 30; echo not-interrupted\r".to_vec());
    pause(&mut s, 500);
    s.input(vec![3]);
    pause(&mut s, 300);
    s.input(b"echo after-ok\r".to_vec());
    let intr = wait(&mut s, "after-ok", 2) && !screen(&s).contains("not-interrupted\n");
    // A command of its own, as a package install runs (its end and code).
    // In a folder seen through the host's root (as "File system" shows it).
    let inside = crate::sandbox::root().join(home.strip_prefix("/").unwrap_or(&home));
    let mut cmd = crate::terminal::Session::spawn_with(&prefs, &inside, &ctx, 80, 24, true, Some("test -t 0 && echo polka-tty; exit 7")).ok();
    let mut cmd_ok = None;
    for _ in 0..200 {
        let Some(c) = cmd.as_mut() else { break };
        c.poll(&scheme);
        if c.exited {
            cmd_ok = Some((screen(c).contains("polka-tty"), c.status));
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    drop(cmd);
    println!("terminal command: {cmd_ok:?} (want: tty true, code 7)");
    s.input(b"cd /tmp\r".to_vec());
    pause(&mut s, 400);
    let cwd = s.cwd();
    s.input(b"sleep 3\r".to_vec());
    pause(&mut s, 400);
    let fg = s.foreground();
    s.input(vec![3]);
    println!("terminal: output {out}, resize {size}, ctrl+c {intr}, folder {cwd:?}, running {fg:?} (shell {})", s.title);
    if !(out && size && intr) {
        println!("{}", screen(&s).lines().map(str::trim_end).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n"));
    }
}
