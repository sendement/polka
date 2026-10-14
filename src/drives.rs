//! Drives for the sidebar: partitions from `lsblk`, mounted or not
//! (system ones left out), and mounting, unmounting and ejecting through
//! udisks (`udisksctl`), which asks polkit when it needs a password.

use std::path::PathBuf;

use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Drive {
    /// The partition: /dev/sdb1.
    pub dev: String,
    /// Its whole disk (/dev/sdb), what gets powered off on eject.
    pub disk: String,
    pub label: Option<String>,
    pub uuid: Option<String>,
    pub size: u64,
    pub fstype: String,
    pub mount: Option<PathBuf>,
    /// USB sticks, card readers, external disks.
    pub removable: bool,
    pub optical: bool,
    pub ssd: bool,
}

impl Drive {
    /// Key for its chosen icon: the same whether mounted or not.
    pub fn key(&self) -> String {
        format!("dev:{}", self.uuid.as_deref().unwrap_or(&self.dev))
    }

    pub fn icon(&self) -> &'static [&'static str] {
        if self.optical {
            &["media-optical", "drive-optical", "drive-removable-media"]
        } else if self.removable {
            &["drive-removable-media-usb", "drive-removable-media", "media-removable", "drive-harddisk"]
        } else if self.ssd {
            &["drive-harddisk-solidstate", "drive-harddisk", "drive-harddisk-root"]
        } else {
            &["drive-harddisk", "drive-harddisk-root"]
        }
    }
}

/// Where desktop mounts go; anything mounted elsewhere is the system's.
const USER_MOUNTS: &[&str] = &["/run/media/", "/media/", "/mnt/"];
/// File systems that aren't for browsing.
const SKIP_FS: &[&str] = &["swap", "LVM2_member", "linux_raid_member", "crypto_LUKS", "zfs_member", "BitLocker", "ddf_raid_member", "isw_raid_member"];

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).filter(|x| !x.is_empty()).map(str::to_string)
}

/// lsblk prints true/false, older ones "1"/"0".
fn flag(v: &Value, k: &str) -> bool {
    match v.get(k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(x)) => x == "1",
        Some(Value::Number(n)) => n.as_u64() == Some(1),
        _ => false,
    }
}

fn walk(v: &Value, parent: Option<&Value>, out: &mut Vec<Drive>) {
    let kind = s(v, "type").unwrap_or_default();
    let fstype = s(v, "fstype");
    let mounts: Vec<PathBuf> = v.get("mountpoints").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(PathBuf::from).collect()).unwrap_or_default();
    let user_mount = mounts.iter().find(|m| USER_MOUNTS.iter().any(|u| m.starts_with(u))).cloned();
    let removable = flag(v, "rm") || flag(v, "hotplug") || parent.is_some_and(|p| flag(p, "rm") || flag(p, "hotplug"));
    let size = v.get("size").and_then(Value::as_u64).unwrap_or(0);
    let take = match &fstype {
        Some(f) if !SKIP_FS.contains(&f.as_str()) && kind != "loop" || user_mount.is_some() => {
            // Mounted by the system (/, /boot, /home…): not ours to show.
            let system = !mounts.is_empty() && user_mount.is_none();
            // Unmounted small internal partitions: EFI, recovery.
            let tiny = mounts.is_empty() && !removable && size < 1_200_000_000;
            !system && !tiny && fstype.is_some()
        }
        _ => false,
    };
    if take {
        let dev = s(v, "path").unwrap_or_default();
        let disk = parent.and_then(|p| s(p, "path")).unwrap_or_else(|| dev.clone());
        out.push(Drive {
            label: s(v, "label").or_else(|| s(v, "partlabel")),
            uuid: s(v, "uuid"),
            size,
            fstype: fstype.unwrap_or_default(),
            mount: user_mount,
            removable,
            optical: kind == "rom",
            ssd: !flag(v, "rota") && !removable,
            dev,
            disk,
        });
    }
    if let Some(children) = v.get("children").and_then(Value::as_array) {
        // Partitions belong to the whole disk.
        let p = if kind == "disk" || kind == "rom" { Some(v) } else { parent };
        for c in children {
            walk(c, p, out);
        }
    }
}

pub fn parse(json: &str) -> Vec<Drive> {
    let mut out = Vec::new();
    if let Ok(v) = serde_json::from_str::<Value>(json) {
        for d in v.get("blockdevices").and_then(Value::as_array).into_iter().flatten() {
            walk(d, None, &mut out);
        }
    }
    out
}

/// The drives to show. Tests read `POLKA_LSBLK` (a saved listing) and
/// otherwise see none.
pub fn list() -> Vec<Drive> {
    if let Some(f) = std::env::var_os("POLKA_LSBLK") {
        return parse(&std::fs::read_to_string(f).unwrap_or_default());
    }
    if cfg!(test) {
        return Vec::new();
    }
    let out = std::process::Command::new("lsblk")
        .args(["-J", "-b", "-o", "NAME,PATH,LABEL,PARTLABEL,UUID,SIZE,FSTYPE,MOUNTPOINTS,RM,HOTPLUG,TYPE,ROTA,PKNAME"])
        .output();
    match out {
        Ok(o) if o.status.success() => parse(&String::from_utf8_lossy(&o.stdout)),
        _ => Vec::new(),
    }
}

pub enum Op {
    /// Mount, then open it.
    Mount,
    Unmount,
    /// Unmount every partition of the disk (these), then power it off (or
    /// eject a disc).
    Eject(Vec<Drive>),
}

/// What tests asked udisks to do (nothing really runs; ops run on their
/// own thread, hence not a thread-local).
#[cfg(test)]
pub static RAN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn run(args: &[&str]) -> Result<String, String> {
    #[cfg(test)]
    {
        RAN.lock().unwrap().push(args.join(" "));
        Ok(String::new())
    }
    #[cfg(not(test))]
    {
        let out = std::process::Command::new(args[0]).args(&args[1..]).stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            // udisks: "Error unmounting /dev/sdb1: GDBus.Error:…: Target is busy" → the end.
            let line = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
            Err(line.rsplit(": ").next().unwrap_or(line).to_string())
        }
    }
}

/// "Mounted /dev/sdb1 at /run/media/me/STICK" (with or without a dot).
fn mounted_at(out: &str) -> Option<PathBuf> {
    let (_, at) = out.trim().split_once(" at ")?;
    Some(PathBuf::from(at.trim().trim_end_matches('.')))
}

pub enum Done {
    Mounted(PathBuf),
    /// Where it was mounted.
    Unmounted(Vec<PathBuf>),
    Ejected(Vec<PathBuf>),
}

/// Do it (blocking; call from a thread).
pub fn apply(d: &Drive, op: &Op) -> Result<Done, String> {
    match op {
        Op::Mount => {
            let out = run(&["udisksctl", "mount", "-b", &d.dev])?;
            // Tests: nowhere real; the drive keeps its listed place.
            Ok(Done::Mounted(mounted_at(&out).or_else(|| d.mount.clone()).unwrap_or_else(|| PathBuf::from("/"))))
        }
        Op::Unmount => {
            run(&["udisksctl", "unmount", "-b", &d.dev])?;
            Ok(Done::Unmounted(d.mount.iter().cloned().collect()))
        }
        Op::Eject(mounted) => {
            for m in mounted {
                run(&["udisksctl", "unmount", "-b", &m.dev])?;
            }
            if d.optical {
                run(&["eject", &d.disk])?;
            } else {
                run(&["udisksctl", "power-off", "-b", &d.disk])?;
            }
            Ok(Done::Ejected(mounted.iter().filter_map(|m| m.mount.clone()).collect()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn picks_user_drives() {
        let json = r#"{"blockdevices":[
          {"name":"zram0","path":"/dev/zram0","fstype":"swap","mountpoints":["[SWAP]"],"rm":false,"hotplug":false,"type":"disk","rota":false,"size":100},
          {"name":"nvme0n1","path":"/dev/nvme0n1","fstype":null,"mountpoints":[],"rm":false,"hotplug":false,"type":"disk","rota":false,"size":1000000000000,"children":[
            {"name":"nvme0n1p1","path":"/dev/nvme0n1p1","uuid":"AA","fstype":"vfat","mountpoints":["/boot"],"rm":false,"hotplug":false,"type":"part","rota":false,"size":500000000},
            {"name":"nvme0n1p2","path":"/dev/nvme0n1p2","uuid":"BB","fstype":"btrfs","mountpoints":["/home","/"],"rm":false,"hotplug":false,"type":"part","rota":false,"size":900000000000},
            {"name":"nvme0n1p3","path":"/dev/nvme0n1p3","uuid":"CC","label":"Windows","fstype":"ntfs","mountpoints":[null],"rm":false,"hotplug":false,"type":"part","rota":false,"size":90000000000},
            {"name":"nvme0n1p4","path":"/dev/nvme0n1p4","uuid":"DD","fstype":"ntfs","mountpoints":[null],"rm":false,"hotplug":false,"type":"part","rota":false,"size":700000000}
          ]},
          {"name":"sda","path":"/dev/sda","fstype":null,"mountpoints":[null],"rm":"1","hotplug":"1","type":"disk","rota":"1","size":64000000000,"children":[
            {"name":"sda1","path":"/dev/sda1","uuid":"EE","label":"STICK","fstype":"exfat","mountpoints":["/run/media/me/STICK"],"rm":"1","hotplug":"1","type":"part","rota":"1","size":64000000000}
          ]},
          {"name":"sr0","path":"/dev/sr0","uuid":"FF","label":"DISC","fstype":"iso9660","mountpoints":[],"rm":true,"hotplug":false,"type":"rom","rota":true,"size":700000000}
        ]}"#;
        let d = parse(json);
        let names: Vec<_> = d.iter().map(|d| d.dev.as_str()).collect();
        assert_eq!(names, ["/dev/nvme0n1p3", "/dev/sda1", "/dev/sr0"], "{d:#?}");
        assert!(!d[0].removable && d[0].mount.is_none() && d[0].ssd && d[0].disk == "/dev/nvme0n1");
        assert!(d[1].removable && d[1].mount.as_deref() == Some(Path::new("/run/media/me/STICK")) && d[1].disk == "/dev/sda");
        assert!(d[2].optical && d[2].disk == "/dev/sr0");
        assert_eq!(mounted_at("Mounted /dev/sda1 at /run/media/me/STICK.\n").as_deref(), Some(Path::new("/run/media/me/STICK")));
        assert_eq!(mounted_at("Mounted /dev/sda1 at /run/media/me/My Stick\n").as_deref(), Some(Path::new("/run/media/me/My Stick")));
    }
}
