//! Network places: SMB, NFS, SFTP, WebDAV, FTP shares and rclone remotes,
//! mounted with whatever the system has (sshfs, rclone, GVfs's gio,
//! mount.cifs / mount.nfs through sudo) so the rest of Polka sees plain
//! folders. Passwords go to the keyring (secret-tool) when asked to, else
//! they're kept for the session only.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    #[default]
    Smb,
    Nfs,
    Sftp,
    WebDav,
    Ftp,
    Rclone,
}

pub const KINDS: [Kind; 6] = [Kind::Smb, Kind::Nfs, Kind::Sftp, Kind::WebDav, Kind::Ftp, Kind::Rclone];

/// What does the mounting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tool {
    Sshfs,
    Rclone,
    Gio,
    /// mount.cifs, as administrator.
    Cifs,
    /// mount.nfs, as administrator.
    Nfs,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Conn {
    /// Stable, also names the mount folder and the keyring entry.
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// Server name or address; the whole URL for WebDAV; the remote's name
    /// for rclone.
    pub address: String,
    pub port: Option<u16>,
    /// The share (SMB), the export (NFS), else a folder on the server.
    pub path: String,
    pub user: String,
    /// None: the first tool there is.
    pub tool: Option<Tool>,
    /// The password goes to the keyring.
    pub remember: bool,
    /// The mount folder's name: from the name it was first saved with, kept
    /// after (renaming it mustn't lose the mount).
    pub dir: String,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Smb => "SMB",
            Kind::Nfs => "NFS",
            Kind::Sftp => "SFTP",
            Kind::WebDav => "WebDAV",
            Kind::Ftp => "FTP",
            Kind::Rclone => "rclone",
        }
    }

    /// Tools that can mount it, best first.
    pub fn tools(self) -> &'static [Tool] {
        match self {
            Kind::Smb => &[Tool::Gio, Tool::Rclone, Tool::Cifs],
            Kind::Nfs => &[Tool::Nfs],
            Kind::Sftp => &[Tool::Sshfs, Tool::Rclone, Tool::Gio],
            Kind::WebDav => &[Tool::Rclone, Tool::Gio],
            Kind::Ftp => &[Tool::Rclone, Tool::Gio],
            Kind::Rclone => &[Tool::Rclone],
        }
    }

    pub fn has_user(self) -> bool {
        !matches!(self, Kind::Nfs | Kind::Rclone)
    }

    pub fn has_port(self) -> bool {
        !matches!(self, Kind::Rclone | Kind::WebDav)
    }

    /// Asks for a password before trying (SFTP tries keys first; NFS and
    /// rclone have none of their own).
    pub fn wants_password(self) -> bool {
        matches!(self, Kind::Smb | Kind::WebDav | Kind::Ftp)
    }

    fn gvfs_backend(self) -> Option<&'static str> {
        match self {
            Kind::Smb => Some("smb"),
            Kind::Sftp => Some("sftp"),
            Kind::WebDav => Some("dav"),
            Kind::Ftp => Some("ftp"),
            _ => None,
        }
    }
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Sshfs => "sshfs",
            Tool::Rclone => "rclone",
            Tool::Gio => "GVfs (gio)",
            Tool::Cifs => "mount.cifs (sudo)",
            Tool::Nfs => "mount.nfs (sudo)",
        }
    }

    /// The package to install for it (Arch / most distributions).
    pub fn package(self) -> &'static str {
        match self {
            Tool::Sshfs => "sshfs",
            Tool::Rclone => "rclone",
            Tool::Gio => "gvfs",
            Tool::Cifs => "cifs-utils",
            Tool::Nfs => "nfs-utils",
        }
    }

    /// Mounted as administrator (sudo).
    pub fn root(self) -> bool {
        matches!(self, Tool::Cifs | Tool::Nfs)
    }
}

/// A shell check on the host (where the tools live, also from Flatpak).
fn host_ok(script: &str) -> bool {
    let mut c = Command::new("sh");
    c.args(["-c", script]);
    crate::sandbox::host(c).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

static FOUND: Mutex<Option<HashMap<(Tool, Option<&'static str>), bool>>> = Mutex::new(None);

/// Whether the system has the tool (for this kind: GVfs needs its backend).
pub fn available(tool: Tool, kind: Kind) -> bool {
    if cfg!(test) {
        return std::env::var("POLKA_NET_TOOLS").is_ok_and(|v| v.split(',').any(|t| t == tool.label().split(' ').next().unwrap_or("")));
    }
    let backend = if tool == Tool::Gio { kind.gvfs_backend() } else { None };
    let key = (tool, backend);
    if let Some(v) = FOUND.lock().ok().and_then(|f| f.as_ref().and_then(|m| m.get(&key).copied())) {
        return v;
    }
    let script = match tool {
        Tool::Sshfs => "command -v sshfs".to_string(),
        Tool::Rclone => "command -v rclone".to_string(),
        Tool::Cifs => "command -v mount.cifs || [ -x /usr/bin/mount.cifs ] || [ -x /sbin/mount.cifs ]".to_string(),
        Tool::Nfs => "command -v mount.nfs || [ -x /usr/bin/mount.nfs ] || [ -x /sbin/mount.nfs ]".to_string(),
        Tool::Gio => format!(
            "command -v gio >/dev/null && for d in /usr/lib/gvfs /usr/libexec /usr/libexec/gvfs /usr/lib/*/gvfs; do [ -x \"$d/gvfsd-{}\" ] && exit 0; done; exit 1",
            backend.unwrap_or("none")
        ),
    };
    let ok = host_ok(&script);
    if let Ok(mut f) = FOUND.lock() {
        f.get_or_insert_with(HashMap::new).insert(key, ok);
    }
    ok
}

/// Look again (after installing something).
pub fn forget_tools() {
    if let Ok(mut f) = FOUND.lock() {
        *f = None;
    }
}

impl Conn {
    /// The tool to use: the chosen one, else the first there is.
    pub fn tool(&self) -> Option<Tool> {
        self.tool.filter(|t| self.kind.tools().contains(t)).or_else(|| self.kind.tools().iter().copied().find(|t| available(*t, self.kind)))
    }

    /// Where it's mounted (all but GVfs, which picks its own place).
    pub fn mount_dir(&self) -> PathBuf {
        let name = if self.dir.is_empty() { &self.id } else { &self.dir };
        net_root().join(name)
    }

    fn host(&self) -> String {
        match self.port {
            Some(p) if self.kind.has_port() => format!("{}:{p}", self.address),
            _ => self.address.clone(),
        }
    }

    fn rel(&self) -> String {
        self.path.trim_matches('/').to_string()
    }

    /// The address GVfs takes.
    pub fn uri(&self) -> String {
        let user = if self.user.is_empty() { String::new() } else { format!("{}@", self.user) };
        let rel = self.rel();
        match self.kind {
            Kind::Smb => format!("smb://{user}{}/{rel}", self.host()),
            Kind::Sftp => format!("sftp://{user}{}/{rel}", self.host()),
            Kind::Ftp => format!("ftp://{user}{}/{rel}", self.host()),
            Kind::WebDav => {
                let a = self.address.trim_end_matches('/');
                let a = if let Some(r) = a.strip_prefix("https://") { format!("davs://{user}{r}") } else if let Some(r) = a.strip_prefix("http://") { format!("dav://{user}{r}") } else { format!("davs://{user}{a}") };
                if rel.is_empty() { a } else { format!("{a}/{rel}") }
            }
            Kind::Nfs => format!("nfs://{}/{rel}", self.host()),
            Kind::Rclone => format!("{}:{rel}", self.address),
        }
    }

    /// The remote as rclone names it, made up on the spot for the others.
    pub fn rclone_remote(&self) -> String {
        let q = |v: &str| format!("'{}'", v.replace('\'', "''"));
        let mut opts = Vec::new();
        let backend = match self.kind {
            Kind::Rclone => return format!("{}:{}", self.address, self.rel()),
            Kind::Sftp => "sftp",
            Kind::Smb => "smb",
            Kind::Ftp => "ftp",
            Kind::WebDav => "webdav",
            Kind::Nfs => "nfs",
        };
        if self.kind == Kind::WebDav {
            opts.push(format!("url={}", q(&self.address)));
        } else {
            opts.push(format!("host={}", q(&self.address)));
            if let Some(p) = self.port {
                opts.push(format!("port={p}"));
            }
        }
        if !self.user.is_empty() {
            opts.push(format!("user={}", q(&self.user)));
        }
        format!(":{backend},{}:{}", opts.join(","), self.rel())
    }

    /// GVfs's folder for it, if it's mounted (it names them by the server
    /// and share: smb-share:server=nas,share=media,user=me).
    pub fn gvfs_dir(&self) -> Option<PathBuf> {
        let root = gvfs_root()?;
        let want_scheme = match self.kind {
            Kind::Smb => "smb-share",
            Kind::Sftp => "sftp",
            Kind::Ftp => "ftp",
            Kind::WebDav => "dav",
            _ => return None,
        };
        let host = match self.kind {
            Kind::WebDav => self.address.split("://").last().unwrap_or("").split(['/', ':']).next().unwrap_or("").to_lowercase(),
            _ => self.address.to_lowercase(),
        };
        let share = self.rel().split('/').next().unwrap_or("").to_lowercase();
        for e in std::fs::read_dir(root).ok()?.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some((scheme, rest)) = name.split_once(':') else { continue };
            if scheme != want_scheme {
                continue;
            }
            let kv: HashMap<&str, String> = rest.split(',').filter_map(|p| p.split_once('=')).map(|(k, v)| (k, unescape_gvfs(v).to_lowercase())).collect();
            let h = kv.get("server").or_else(|| kv.get("host")).cloned().unwrap_or_default();
            let user_ok = self.user.is_empty() || kv.get("user").is_none_or(|u| *u == self.user.to_lowercase());
            let share_ok = self.kind != Kind::Smb || kv.get("share").is_some_and(|s| *s == share);
            if h == host && user_ok && share_ok {
                let mut p = e.path();
                // Within the share, the folder asked for.
                let rel = self.rel();
                let inner: Vec<&str> = rel.split('/').skip(if self.kind == Kind::Smb { 1 } else { 0 }).filter(|s| !s.is_empty()).collect();
                for s in inner {
                    p.push(s);
                }
                return Some(p);
            }
        }
        None
    }

    /// Mounted: where to open it.
    pub fn mounted(&self, mounts: &[PathBuf]) -> Option<PathBuf> {
        let dir = self.mount_dir();
        if mounts.contains(&dir) {
            return Some(dir);
        }
        if self.kind.gvfs_backend().is_some() { self.gvfs_dir() } else { None }
    }
}

fn unescape_gvfs(v: &str) -> String {
    // GVfs escapes like URIs (%20).
    crate::app::percent_decoded(v).to_string_lossy().into_owned()
}

fn gvfs_root() -> Option<PathBuf> {
    let run = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| format!("/run/user/{}", unsafe { libc::getuid() }).into());
    Some(run.join("gvfs")).filter(|p| p.is_dir())
}

/// Mount points in use (read without touching any of them: a dead network
/// mount could hang a look at it).
pub fn mount_points() -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string("/proc/self/mounts") else { return Vec::new() };
    let unescape = |s: &str| s.replace("\\040", " ").replace("\\011", "\t").replace("\\134", "\\");
    text.lines().filter_map(|l| l.split(' ').nth(1)).map(|m| PathBuf::from(unescape(m))).collect()
}

/// How it'll be mounted: commands of the user's own, or ones for sudo.
pub enum Plan {
    /// Run in the background; the folder to open after.
    User,
    /// Steps for sudo, and a file with the password to remove after.
    Root { steps: Vec<Vec<std::ffi::OsString>>, cred: Option<PathBuf> },
}

/// What to do to mount it (sudo steps for the administrator's tools; the
/// rest is done by [`mount`]).
pub fn plan(c: &Conn, password: Option<&str>) -> Result<Plan, String> {
    let tool = c.tool().ok_or_else(|| missing(c.kind))?;
    let dir = c.mount_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let os = |v: &[&str]| v.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
    match tool {
        Tool::Cifs => {
            let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
            let mut o = format!("uid={uid},gid={gid},iocharset=utf8,file_mode=0644,dir_mode=0755");
            if let Some(p) = c.port {
                o.push_str(&format!(",port={p}"));
            }
            let cred = if c.user.is_empty() {
                o += ",guest";
                None
            } else {
                // The password in a file only we can read, never on a
                // command line.
                // (Beside the mount folders: the same path on the host,
                // where sudo reads it, also from Flatpak.)
                use std::os::unix::fs::OpenOptionsExt;
                let f = dir.with_file_name(format!(".cred-{}", std::process::id()));
                let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&f).map_err(|e| e.to_string())?;
                writeln!(file, "username={}\npassword={}", c.user, password.unwrap_or("")).map_err(|e| e.to_string())?;
                o.push_str(&format!(",credentials={}", f.display()));
                Some(f)
            };
            let src = format!("//{}/{}", c.address, c.rel());
            Ok(Plan::Root { steps: vec![os(&["mount", "-t", "cifs", &src, &dir.to_string_lossy(), "-o", &o])], cred })
        }
        Tool::Nfs => {
            let src = format!("{}:/{}", c.address, c.rel());
            let mut step = os(&["mount", "-t", "nfs", &src, &dir.to_string_lossy()]);
            if let Some(p) = c.port {
                step.extend(os(&["-o", &format!("port={p}")]));
            }
            Ok(Plan::Root { steps: vec![step], cred: None })
        }
        _ => Ok(Plan::User),
    }
}

/// "Install sshfs or rclone" for a kind with no tool on the system.
pub fn missing(kind: Kind) -> String {
    let names: Vec<&str> = kind.tools().iter().map(|t| t.package()).collect();
    format!("{}: {}", crate::i18n::tr("Install one of"), names.join(", "))
}

/// Run a tool, the password (if any) on its input; what it said, if it
/// failed.
fn run(mut c: Command, input: Option<String>) -> Result<(), String> {
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = crate::sandbox::host(c).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    if let Some(mut i) = child.stdin.take() {
        if let Some(s) = input {
            let _ = i.write_all(s.as_bytes());
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        let mut msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if msg.is_empty() {
            msg = String::from_utf8_lossy(&out.stdout).trim().to_string();
        }
        Err(msg.lines().last().unwrap_or("failed").to_string())
    }
}

/// Mount with the user's own tools (the blocking part; off the UI thread).
/// Where to open it after.
pub fn mount(c: &Conn, password: Option<&str>) -> Result<PathBuf, String> {
    let tool = c.tool().ok_or_else(|| missing(c.kind))?;
    let dir = c.mount_dir();
    match tool {
        Tool::Sshfs => {
            let mut cmd = Command::new("sshfs");
            let target = if c.user.is_empty() { format!("{}:/{}", c.address, c.rel()) } else { format!("{}@{}:/{}", c.user, c.address, c.rel()) };
            cmd.arg(target).arg(&dir);
            if let Some(p) = c.port {
                cmd.args(["-p", &p.to_string()]);
            }
            let mut o = "reconnect,ServerAliveInterval=15,ServerAliveCountMax=3,StrictHostKeyChecking=accept-new".to_string();
            // No password: keys only, and no question nobody can answer.
            o.push_str(if password.is_some() { ",password_stdin" } else { ",BatchMode=yes" });
            cmd.args(["-o", &o]);
            run(cmd, password.map(|p| format!("{p}\n")))?;
            Ok(dir)
        }
        Tool::Rclone => {
            let mut cmd = Command::new("rclone");
            cmd.args(["mount", &c.rclone_remote()]).arg(&dir).args(["--daemon", "--vfs-cache-mode", "writes"]);
            if let (Some(pw), Some(var)) = (password, rclone_pass_var(c.kind)) {
                cmd.env(var, obscure(pw)?);
            }
            run(cmd, None)?;
            Ok(dir)
        }
        Tool::Gio => {
            let mut cmd = Command::new("gio");
            cmd.args(["mount", &c.uri()]);
            // gio asks on its input: SMB for the user, the domain, the
            // password; the others just the password.
            let answers = match (c.kind, password) {
                (Kind::Smb, Some(p)) => Some(format!("{}\n\n{p}\n", c.user)),
                (_, Some(p)) => Some(format!("{p}\n")),
                (_, None) => None,
            };
            if answers.is_none() {
                cmd.arg("--anonymous");
            }
            match run(cmd, answers) {
                Ok(()) => {}
                // Mounted already is fine.
                Err(e) if e.to_lowercase().contains("already mounted") => {}
                Err(e) => return Err(e),
            }
            c.gvfs_dir().ok_or_else(|| "GVfs: mounted, but its folder isn't there".to_string())
        }
        Tool::Cifs | Tool::Nfs => Err("needs sudo".into()),
    }
}

fn rclone_pass_var(k: Kind) -> Option<&'static str> {
    match k {
        Kind::Sftp => Some("RCLONE_SFTP_PASS"),
        Kind::Smb => Some("RCLONE_SMB_PASS"),
        Kind::Ftp => Some("RCLONE_FTP_PASS"),
        Kind::WebDav => Some("RCLONE_WEBDAV_PASS"),
        _ => None,
    }
}

/// rclone takes passwords only "obscured".
fn obscure(pw: &str) -> Result<String, String> {
    let mut c = Command::new("rclone");
    c.args(["obscure", "-"]);
    let mut child = crate::sandbox::host(c).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(pw.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// What unmounts it: the user's commands, or a step for sudo.
pub enum Unplan {
    User(Command),
    Root(Vec<std::ffi::OsString>),
}

pub fn unmount_plan(c: &Conn, at: &Path) -> Unplan {
    if c.mount_dir() == at {
        // Ours: FUSE ones the user can take down; the rest needs root.
        if c.tool().is_some_and(|t| t.root()) {
            return Unplan::Root(vec!["umount".into(), at.as_os_str().to_owned()]);
        }
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "fusermount3 -u \"$0\" 2>/dev/null || fusermount -u \"$0\""]).arg(at);
        return Unplan::User(cmd);
    }
    let mut cmd = Command::new("gio");
    cmd.args(["mount", "-u", &c.uri()]);
    Unplan::User(cmd)
}

pub fn unmount(cmd: Command) -> Result<(), String> {
    run(cmd, None)
}

/// A failure that a password would fix.
pub fn auth_failed(msg: &str) -> bool {
    let m = msg.to_lowercase();
    ["permission denied", "password", "authentication", "logon failure", "access denied", "401", "530 ", "login incorrect", "nt_status_logon"].iter().any(|w| m.contains(w))
}

// ---------------------------------------------------------------- keyring

pub fn keyring() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    !cfg!(test) && *K.get_or_init(|| host_ok("command -v secret-tool"))
}

pub fn keyring_get(id: &str) -> Option<String> {
    if cfg!(test) {
        return None;
    }
    let mut c = Command::new("secret-tool");
    c.args(["lookup", "application", "polka", "connection", id]);
    let out = crate::sandbox::host(c).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    let pw = String::from_utf8_lossy(&out.stdout).to_string();
    (out.status.success() && !pw.is_empty()).then_some(pw)
}

pub fn keyring_set(id: &str, label: &str, pw: &str) -> Result<(), String> {
    let mut c = Command::new("secret-tool");
    c.args(["store", &format!("--label=Polka: {label}"), "application", "polka", "connection", id]);
    run(c, Some(pw.to_string()))
}

pub fn keyring_clear(id: &str) {
    if cfg!(test) {
        return;
    }
    let mut c = Command::new("secret-tool");
    c.args(["clear", "application", "polka", "connection", id]);
    let _ = run(c, None);
}

fn net_root() -> PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("polka/net")
}

/// A folder name for a connection: its name made safe, unlike the others'.
pub fn dir_name(name: &str, taken: &[String]) -> String {
    let mut safe: String = name.chars().map(|c| if c.is_alphanumeric() || "-_.() ".contains(c) { c } else { '-' }).collect::<String>().trim().trim_matches('.').to_string();
    if safe.is_empty() {
        safe = "share".into();
    }
    crate::ops::unique_name(&safe, |n| taken.iter().any(|t| t == n))
}

/// A new connection's id: from the time, so it's never reused.
pub fn new_id() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    format!("c{t:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        let c = Conn { kind: Kind::Smb, address: "nas".into(), path: "/media/films/".into(), user: "me".into(), ..Default::default() };
        assert_eq!(c.uri(), "smb://me@nas/media/films");
        assert_eq!(c.rclone_remote(), ":smb,host='nas',user='me':media/films");
        let s = Conn { kind: Kind::Sftp, address: "box".into(), port: Some(2222), path: "home/me".into(), ..Default::default() };
        assert_eq!(s.uri(), "sftp://box:2222/home/me");
        assert_eq!(s.rclone_remote(), ":sftp,host='box',port=2222:home/me");
        let w = Conn { kind: Kind::WebDav, address: "https://cloud.example.org/remote.php/dav/files/me/".into(), user: "me".into(), ..Default::default() };
        assert_eq!(w.uri(), "davs://me@cloud.example.org/remote.php/dav/files/me");
        assert_eq!(w.rclone_remote(), ":webdav,url='https://cloud.example.org/remote.php/dav/files/me/',user='me':");
        let r = Conn { kind: Kind::Rclone, address: "gdrive".into(), path: "Photos".into(), ..Default::default() };
        assert_eq!(r.rclone_remote(), "gdrive:Photos");
        assert!(auth_failed("mount error(13): Permission denied") && auth_failed("NT_STATUS_LOGON_FAILURE") && !auth_failed("No route to host"));
    }
}
