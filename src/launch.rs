//! Opening files: .desktop entries are executed, native executables run,
//! everything else goes through xdg-open.

use std::io::Read;
use std::path::Path;
use std::process::Command;

use freedesktop_desktop_entry::{DesktopEntry, get_languages_from_env};

pub fn open(path: &Path) -> Result<(), String> {
    let lower = path.to_string_lossy().to_lowercase();
    if lower.ends_with(".desktop") {
        let locales = get_languages_from_env();
        let de = DesktopEntry::from_path(path, Some(&locales)).map_err(|e| e.to_string())?;
        let argv = de.parse_exec().map_err(|e| e.to_string())?;
        let (prog, args) = argv.split_first().ok_or("empty Exec")?;
        let mut cmd = Command::new(prog);
        cmd.args(args);
        if let Some(dir) = de.path() {
            cmd.current_dir(dir);
        }
        return spawn(cmd);
    }
    if is_native_executable(path) {
        let mut cmd = Command::new(path);
        if let Some(dir) = path.parent() {
            cmd.current_dir(dir);
        }
        return spawn(cmd);
    }
    let mut cmd = Command::new("xdg-open");
    cmd.arg(path);
    spawn(cmd)
}

/// What to call a program or launcher: its Name= or its file name.
pub fn app_name(app: &Path) -> String {
    if app.to_string_lossy().to_lowercase().ends_with(".desktop") {
        let locales = get_languages_from_env();
        if let Some(n) = DesktopEntry::from_path(app, Some(&locales)).ok().and_then(|de| de.name(&locales).map(|n| n.to_string())) {
            return n;
        }
    }
    app.file_name().map_or_else(|| app.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Run a launcher or a program with these files (dropped on it).
pub fn open_with(app: &Path, files: &[std::path::PathBuf]) -> Result<(), String> {
    let names: Vec<String> = files.iter().map(|f| f.to_string_lossy().into_owned()).collect();
    let cmd = if app.to_string_lossy().to_lowercase().ends_with(".desktop") {
        let locales = get_languages_from_env();
        let de = DesktopEntry::from_path(app, Some(&locales)).map_err(|e| e.to_string())?;
        // %f / %F / %u / %U take the files; a launcher without them gets none.
        let uris: Vec<&str> = names.iter().map(String::as_str).collect();
        let argv = de.parse_exec_with_uris(&uris, &locales).map_err(|e| e.to_string())?;
        let (prog, args) = argv.split_first().ok_or("empty Exec")?;
        let mut cmd = Command::new(prog);
        cmd.args(args);
        if let Some(dir) = de.path() {
            cmd.current_dir(dir);
        }
        cmd
    } else {
        let mut cmd = Command::new(app);
        cmd.args(&names);
        if let Some(dir) = app.parent() {
            cmd.current_dir(dir);
        }
        cmd
    };
    spawn(cmd)
}

pub fn spawn_cmd(cmd: Command) -> Result<(), String> {
    spawn(cmd)
}

/// Opening this runs something (a launcher or a program) rather than
/// showing a document.
pub fn runs(path: &Path) -> bool {
    path.to_string_lossy().to_lowercase().ends_with(".desktop") || is_native_executable(path)
}

/// ELF binaries and scripts with a shebang that have the exec bit.
fn is_native_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Ok(m) = std::fs::metadata(path) else { return false };
    if !m.is_file() || m.permissions().mode() & 0o111 == 0 {
        return false;
    }
    let mut head = [0u8; 4];
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let n = f.read(&mut head).unwrap_or(0);
    head[..n].starts_with(b"\x7fELF") || head[..n].starts_with(b"#!")
}

/// Tests never start other programs: they'd show up on the desktop.
#[cfg(test)]
fn spawn(cmd: Command) -> Result<(), String> {
    let line = format!("{:?}", cmd);
    LAUNCHED.with(|l| l.borrow_mut().push(line));
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// What tests asked to launch.
    pub static LAUNCHED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(not(test))]
fn spawn(mut cmd: Command) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0);
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    // Reap it so it never lingers as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
