//! Opening files: .desktop entries are executed, native executables run,
//! everything else goes through xdg-open.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

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

fn spawn(mut cmd: Command) -> Result<(), String> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0);
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    // Reap it so it never lingers as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
