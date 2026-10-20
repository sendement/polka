//! Phones on USB: Android (MTP, the "File transfer" mode) and iPhone / iPad
//! (AFC). Found in sysfs as they're plugged in, mounted with whatever the
//! system has (aft-mtp-mount, GVfs, jmtpfs, simple-mtpfs; ifuse), so the
//! rest of Polka sees plain folders.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Android,
    Apple,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Phone {
    pub kind: Kind,
    /// "Redmi Note 14 Pro+ 5G", "iPhone".
    pub name: String,
    pub maker: String,
    /// The USB serial (an iPhone's is its UDID without the dash).
    pub serial: String,
    pub bus: u32,
    pub dev: u32,
    /// Files can be reached: MTP on (Android), always for Apple.
    pub files: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Aft,
    Gio,
    Jmtpfs,
    SimpleMtpfs,
    Ifuse,
}

impl Tool {
    fn program(self) -> &'static str {
        match self {
            Tool::Aft => "aft-mtp-mount",
            Tool::Gio => "gio",
            Tool::Jmtpfs => "jmtpfs",
            Tool::SimpleMtpfs => "simple-mtpfs",
            Tool::Ifuse => "ifuse",
        }
    }
}

impl Kind {
    pub fn tools(self) -> &'static [Tool] {
        match self {
            Kind::Android => &[Tool::Aft, Tool::Gio, Tool::Jmtpfs, Tool::SimpleMtpfs],
            Kind::Apple => &[Tool::Ifuse, Tool::Gio],
        }
    }

    /// What to install (Arch package names).
    pub fn packages(self) -> &'static str {
        match self {
            Kind::Android => "android-file-transfer / gvfs-mtp",
            Kind::Apple => "ifuse + usbmuxd / gvfs-afc",
        }
    }
}

fn sysfs() -> PathBuf {
    std::env::var_os("POLKA_SYSFS_USB").map(PathBuf::from).unwrap_or_else(|| "/sys/bus/usb/devices".into())
}

fn read(dir: &Path, f: &str) -> String {
    std::fs::read_to_string(dir.join(f)).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Phones plugged in now.
pub fn detect() -> Vec<Phone> {
    let root = sysfs();
    let Ok(rd) = std::fs::read_dir(&root) else { return Vec::new() };
    let mut out = Vec::new();
    let mut devices: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.join("idVendor").exists()).collect();
    devices.sort();
    for d in devices {
        let vendor = read(&d, "idVendor");
        let product = read(&d, "product");
        let base = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        // Its interfaces: "3-10:1.0" and so on.
        let ifaces: Vec<(String, String, String, String)> = std::fs::read_dir(&root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(&format!("{base}:")))
            .map(|e| {
                let p = e.path();
                (read(&p, "bInterfaceClass"), read(&p, "bInterfaceSubClass"), read(&p, "bInterfaceProtocol"), read(&p, "interface"))
            })
            .collect();
        let mtp = ifaces.iter().any(|(c, s, p, name)| name.contains("MTP") || (c == "06" && s == "01" && p == "01" && !name.contains("PTP")));
        let adb = ifaces.iter().any(|(c, s, _, _)| c == "ff" && s == "42");
        let apple = vendor == "05ac" && ["iPhone", "iPad", "iPod"].iter().any(|n| product.contains(n));
        if !(mtp || adb || apple) {
            continue;
        }
        let kind = if apple { Kind::Apple } else { Kind::Android };
        out.push(Phone {
            kind,
            name: if product.is_empty() { "Phone".into() } else { product },
            maker: read(&d, "manufacturer"),
            serial: read(&d, "serial"),
            bus: read(&d, "busnum").parse().unwrap_or(0),
            dev: read(&d, "devnum").parse().unwrap_or(0),
            files: apple || mtp,
        });
    }
    out
}

/// Something changed on USB (plugged in or out): the devices' names.
pub fn usb_signature() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut names: Vec<String> = std::fs::read_dir(sysfs()).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    names.hash(&mut h);
    h.finish()
}

fn host_ok(script: &str) -> bool {
    let mut c = Command::new("sh");
    c.args(["-c", script]);
    crate::sandbox::host(c).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// The tool is on the system (GVfs: with its backend for the kind).
pub fn available(tool: Tool, kind: Kind) -> bool {
    if cfg!(test) {
        if let Ok(v) = std::env::var("POLKA_PHONE_TOOLS") {
            return v.split(',').any(|t| t == tool.program());
        }
    }
    static FOUND: std::sync::Mutex<Vec<(Tool, Kind, bool)>> = std::sync::Mutex::new(Vec::new());
    if let Some(v) = FOUND.lock().ok().and_then(|f| f.iter().find(|(t, k, _)| *t == tool && *k == kind).map(|x| x.2)) {
        return v;
    }
    let script = match tool {
        Tool::Gio => format!(
            "command -v gio >/dev/null && for d in /usr/lib/gvfs /usr/libexec /usr/libexec/gvfs /usr/lib/*/gvfs; do [ -x \"$d/gvfsd-{}\" ] && exit 0; done; exit 1",
            if kind == Kind::Android { "mtp" } else { "afc" }
        ),
        t => format!("command -v {}", t.program()),
    };
    let ok = host_ok(&script);
    if let Ok(mut f) = FOUND.lock() {
        f.push((tool, kind, ok));
    }
    ok
}

impl Phone {
    /// A stable key (the sidebar, the busy set).
    pub fn key(&self) -> String {
        format!("phone:{}", if self.serial.is_empty() { &self.name } else { &self.serial })
    }

    pub fn tool(&self) -> Option<Tool> {
        self.kind.tools().iter().copied().find(|t| available(*t, self.kind))
    }

    /// Where it's mounted (all but GVfs).
    pub fn mount_dir(&self) -> PathBuf {
        let safe: String = self.name.chars().map(|c| if c.is_alphanumeric() || "-_.()+ ".contains(c) { c } else { '-' }).collect();
        dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("polka/phones").join(safe.trim())
    }

    /// GVfs's folder for it: mtp:host=…serial… or afc:host=UDID.
    pub fn gvfs_dir(&self) -> Option<PathBuf> {
        let run = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| format!("/run/user/{}", unsafe { libc::getuid() }).into());
        let root = run.join("gvfs");
        let scheme = if self.kind == Kind::Android { "mtp:" } else { "afc:" };
        let serial = self.serial.to_lowercase().replace('-', "");
        std::fs::read_dir(root).ok()?.flatten().map(|e| e.path()).find(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().to_lowercase().replace('-', "")).unwrap_or_default();
            n.starts_with(scheme) && !serial.is_empty() && n.contains(&serial)
        })
    }

    /// Mounted: where to open it.
    pub fn mounted(&self, mounts: &[PathBuf]) -> Option<PathBuf> {
        let dir = self.mount_dir();
        if mounts.contains(&dir) {
            return Some(dir);
        }
        self.gvfs_dir()
    }
}

/// Run a tool; what it said if it failed.
fn run(mut c: Command) -> Result<String, String> {
    let out = crate::sandbox::host({
        c.stdin(Stdio::null());
        c
    })
    .stdin(Stdio::null())
    .output()
    .map_err(|e| e.to_string())?;
    let said = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        Ok(said)
    } else {
        Err(said.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or("failed").to_string())
    }
}

/// The iPhone's UDID as libimobiledevice knows it (newer ones have a dash
/// the USB serial lacks).
fn udid(p: &Phone) -> String {
    let mut c = Command::new("idevice_id");
    c.arg("-l");
    let want = p.serial.to_lowercase().replace('-', "");
    run(c).ok().and_then(|s| s.lines().map(str::trim).find(|l| l.to_lowercase().replace('-', "") == want).map(str::to_string)).unwrap_or_else(|| p.serial.clone())
}

/// What went wrong, as the user can act on it.
#[derive(Debug, PartialEq)]
pub enum Trouble {
    /// No tool for it: what to install.
    Missing(&'static str),
    /// An Android phone that's only charging.
    NoFiles,
    /// The iPhone asks whether to trust this computer.
    Trust,
    Failed(String),
}

/// Mount it (blocking: off the UI thread). Where to open it.
pub fn mount(p: &Phone) -> Result<PathBuf, Trouble> {
    if !p.files {
        return Err(Trouble::NoFiles);
    }
    let tool = p.tool().ok_or(Trouble::Missing(p.kind.packages()))?;
    let dir = p.mount_dir();
    if tool != Tool::Gio {
        std::fs::create_dir_all(&dir).map_err(|e| Trouble::Failed(e.to_string()))?;
    }
    let fail = |e: String| Trouble::Failed(e);
    match tool {
        Tool::Aft => {
            let mut c = Command::new("aft-mtp-mount");
            if !p.serial.is_empty() {
                c.args(["-D", &p.serial]);
            }
            c.arg(&dir);
            run(c).map_err(fail)?;
            Ok(dir)
        }
        Tool::Jmtpfs => {
            let mut c = Command::new("jmtpfs");
            c.arg(format!("-device={},{}", p.bus, p.dev)).arg(&dir);
            run(c).map_err(fail)?;
            Ok(dir)
        }
        Tool::SimpleMtpfs => {
            // Its own numbering: the line naming this phone.
            let mut l = Command::new("simple-mtpfs");
            l.arg("-l");
            let list = run(l).map_err(fail)?;
            let n = list.lines().find(|line| line.contains(&p.name)).and_then(|line| line.split(':').next()).map(|s| s.trim().to_string()).unwrap_or_else(|| "1".into());
            let mut c = Command::new("simple-mtpfs");
            c.args(["--device", &n]).arg(&dir);
            run(c).map_err(fail)?;
            Ok(dir)
        }
        Tool::Ifuse => {
            let id = udid(p);
            // Paired first: the phone asks "Trust this computer?".
            let mut v = Command::new("idevicepair");
            v.args(["-u", &id, "validate"]);
            if run(v).is_err() {
                let mut pair = Command::new("idevicepair");
                pair.args(["-u", &id, "pair"]);
                if let Err(e) = run(pair) {
                    let e = e.to_lowercase();
                    return Err(if e.contains("trust") || e.contains("passcode") || e.contains("locked") { Trouble::Trust } else { Trouble::Failed(e) });
                }
            }
            let mut c = Command::new("ifuse");
            c.arg(&dir).args(["-u", &id]);
            run(c).map_err(fail)?;
            Ok(dir)
        }
        Tool::Gio => {
            // Its own name for it, from the volumes it lists.
            let mut l = Command::new("gio");
            l.args(["mount", "-li"]);
            let list = run(l).unwrap_or_default();
            let scheme = if p.kind == Kind::Android { "mtp://" } else { "afc://" };
            let serial = p.serial.to_lowercase().replace('-', "");
            let uri = list
                .lines()
                .filter_map(|l| l.trim().strip_prefix("activation_root=").map(str::to_string))
                .find(|u| u.starts_with(scheme) && u.to_lowercase().replace('-', "").contains(&serial))
                .unwrap_or_else(|| format!("{scheme}{}/", if p.kind == Kind::Apple { udid(p) } else { p.serial.clone() }));
            let mut c = Command::new("gio");
            c.args(["mount", &uri]);
            match run(c) {
                Ok(_) => {}
                Err(e) if e.to_lowercase().contains("already mounted") => {}
                Err(e) if p.kind == Kind::Apple && e.to_lowercase().contains("trust") => return Err(Trouble::Trust),
                Err(e) => return Err(Trouble::Failed(e)),
            }
            p.gvfs_dir().ok_or_else(|| Trouble::Failed("GVfs: mounted, but its folder isn't there".into()))
        }
    }
}

/// Unmount it (before it's unplugged).
pub fn unmount(p: &Phone, at: &Path) -> Result<(), String> {
    if at == p.mount_dir() {
        let mut c = Command::new("sh");
        c.args(["-c", "fusermount3 -u \"$0\" 2>/dev/null || fusermount -u \"$0\""]).arg(at);
        return run(c).map(|_| ());
    }
    let name = at.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // GVfs: mtp:host=X → mtp://X/.
    let uri = match name.split_once(":host=") {
        Some((scheme, host)) => format!("{scheme}://{host}/"),
        None => return Err("unknown mount".into()),
    };
    let mut c = Command::new("gio");
    c.args(["mount", "-u", &uri]);
    run(c).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_phones() {
        let root = std::env::temp_dir().join(format!("polka-usb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dev = |name: &str, files: &[(&str, &str)]| {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            for (f, v) in files {
                std::fs::write(d.join(f), format!("{v}\n")).unwrap();
            }
        };
        dev("3-10", &[("idVendor", "2717"), ("product", "Redmi Note 14 Pro+ 5G"), ("manufacturer", "Xiaomi"), ("serial", "9f3bcb68"), ("busnum", "3"), ("devnum", "13")]);
        dev("3-10:1.0", &[("bInterfaceClass", "06"), ("bInterfaceSubClass", "01"), ("bInterfaceProtocol", "01"), ("interface", "MTP")]);
        dev("3-11", &[("idVendor", "05ac"), ("product", "iPhone"), ("serial", "00008030001A35E83E8A802E"), ("busnum", "3"), ("devnum", "14")]);
        dev("3-11:1.0", &[("bInterfaceClass", "06"), ("bInterfaceSubClass", "01"), ("bInterfaceProtocol", "01"), ("interface", "PTP")]);
        // Charging only, ADB on.
        dev("3-12", &[("idVendor", "18d1"), ("product", "Pixel 7"), ("serial", "ABC"), ("busnum", "3"), ("devnum", "15")]);
        dev("3-12:1.0", &[("bInterfaceClass", "ff"), ("bInterfaceSubClass", "42"), ("bInterfaceProtocol", "01")]);
        // A mouse.
        dev("3-8", &[("idVendor", "046d"), ("product", "USB Receiver")]);
        dev("3-8:1.0", &[("bInterfaceClass", "03"), ("bInterfaceSubClass", "01"), ("bInterfaceProtocol", "02")]);
        unsafe { std::env::set_var("POLKA_SYSFS_USB", &root) };
        let found = detect();
        unsafe { std::env::remove_var("POLKA_SYSFS_USB") };
        let _ = std::fs::remove_dir_all(&root);
        let names: Vec<(&str, Kind, bool)> = found.iter().map(|p| (p.name.as_str(), p.kind, p.files)).collect();
        assert_eq!(names, [("Redmi Note 14 Pro+ 5G", Kind::Android, true), ("iPhone", Kind::Apple, true), ("Pixel 7", Kind::Android, false)]);
        assert_eq!((found[0].bus, found[0].dev, found[0].serial.as_str()), (3, 13, "9f3bcb68"));
    }
}
