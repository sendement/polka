//! Multi-volume archives.
//!
//! Two kinds are recognised:
//! * plain splits — `name.7z.001`, `.002`… (7-Zip `-v`, HJSplit, `split`):
//!   the volumes are one archive cut into pieces, so they're read as one
//!   stream ([`Joined`]) and libarchive does the rest;
//! * RAR volumes — `name.part1.rar`, `.part2.rar`… or `name.rar`, `.r00`,
//!   `.r01`…: every volume has its own headers, so these go through `7z`.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Split,
    Rar,
}

/// What a file name says about being a volume.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    pub kind: Kind,
    /// Name of the whole archive: `name.7z`, `name.rar`.
    pub archive: String,
    /// 0-based position in the set.
    pub index: usize,
}

fn digits(s: &str) -> Option<usize> {
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten()
}

/// Parse a file name. A bare `name.rar` is volume 0 of an old-style set;
/// it only counts as one when `.r00` siblings exist (see [`set_of`]).
pub fn parse(name: &str) -> Option<Volume> {
    let lower = name.to_lowercase();
    // name.part1.rar / name.part01.rar
    if let Some(stem) = lower.strip_suffix(".rar") {
        if let Some(pos) = stem.rfind(".part") {
            if let Some(n) = digits(&stem[pos + 5..]).filter(|n| *n >= 1) {
                return Some(Volume { kind: Kind::Rar, archive: format!("{}.rar", &name[..pos]), index: n - 1 });
            }
        }
        return Some(Volume { kind: Kind::Rar, archive: name.to_string(), index: 0 });
    }
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() {
        return None;
    }
    let ext_l = ext.to_lowercase();
    // name.r00 → second volume (after name.rar)
    if ext_l.len() == 3 && ext_l.starts_with('r') {
        if let Some(n) = digits(&ext_l[1..]) {
            return Some(Volume { kind: Kind::Rar, archive: format!("{stem}.rar"), index: n + 1 });
        }
    }
    // name.001
    if ext.len() == 3 {
        if let Some(n) = digits(ext).filter(|n| *n >= 1) {
            return Some(Volume { kind: Kind::Split, archive: stem.to_string(), index: n - 1 });
        }
    }
    None
}

/// Volume info for drawing one entry of a listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    /// Same for every volume of a set (the archive name plus kind).
    pub key: String,
    pub archive: String,
    pub index: usize,
    /// Volumes found.
    pub count: usize,
    /// Some volume before the last one found is missing.
    pub gap: bool,
}

fn key(v: &Volume) -> String {
    format!("{:?}:{}", v.kind, v.archive)
}

/// Group a folder listing into volume sets. Only names that really belong
/// to a set appear in the result.
pub fn groups<'a>(names: impl Iterator<Item = &'a str>) -> std::collections::HashMap<String, Member> {
    use std::collections::HashMap;
    let mut sets: HashMap<String, Vec<(String, Volume)>> = HashMap::new();
    for n in names {
        if let Some(v) = parse(n) {
            sets.entry(key(&v)).or_default().push((n.to_string(), v));
        }
    }
    let mut out = HashMap::new();
    for (k, mut vols) in sets {
        let real = match vols[0].1.kind {
            // A lone .001 is still a (one piece) split archive.
            Kind::Split => vols.iter().any(|(_, v)| v.index == 0),
            // A lone name.rar is just a RAR; .partN / .rNN make a set.
            Kind::Rar => vols.len() > 1 || vols[0].1.index > 0,
        };
        if !real {
            continue;
        }
        vols.sort_by_key(|(_, v)| v.index);
        let last = vols.last().map_or(0, |(_, v)| v.index);
        let gap = vols.len() != last + 1;
        let count = vols.len();
        for (name, v) in vols {
            out.insert(name, Member { key: k.clone(), archive: v.archive, index: v.index, count, gap });
        }
    }
    out
}

/// A complete-enough volume set on disk.
#[derive(Clone, Debug)]
pub struct Set {
    pub kind: Kind,
    /// Name of the whole archive (`name.7z`).
    pub archive: String,
    /// Volumes in order.
    pub files: Vec<PathBuf>,
}

/// The set `path` belongs to, if it's a volume.
pub fn set_of(path: &Path) -> Option<Set> {
    let name = path.file_name()?.to_str()?;
    let v = parse(name)?;
    let dir = path.parent()?;
    let mut vols: Vec<(usize, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().into_string().ok()?;
            let o = parse(&n)?;
            (o.kind == v.kind && o.archive == v.archive && e.file_type().ok()?.is_file()).then_some((o.index, e.path()))
        })
        .collect();
    vols.sort();
    vols.dedup_by_key(|(i, _)| *i);
    let set = Set { kind: v.kind, archive: v.archive, files: vols.into_iter().map(|(_, p)| p).collect() };
    match set.kind {
        Kind::Split => (parse(set.files.first()?.file_name()?.to_str()?)?.index == 0).then_some(set),
        Kind::Rar => (set.files.len() > 1 || v.index > 0).then_some(set),
    }
}

/// First volume of the set `path` belongs to (or `path` itself).
pub fn first(path: &Path) -> PathBuf {
    set_of(path).and_then(|s| s.files.into_iter().next()).unwrap_or_else(|| path.to_path_buf())
}

/// Name to judge the archive format by: `x.7z` for `x.7z.001`.
pub fn archive_name(path: &Path) -> String {
    set_of(path).map_or_else(|| path.file_name().unwrap_or_default().to_string_lossy().into_owned(), |s| s.archive)
}

/// Volumes of a plain split, joined into one seekable stream.
pub struct Joined {
    parts: Vec<(File, u64)>,
    /// Start offset of each part.
    starts: Vec<u64>,
    len: u64,
    pos: u64,
}

impl Joined {
    pub fn open(files: &[PathBuf]) -> io::Result<Self> {
        let mut parts = Vec::new();
        let mut starts = Vec::new();
        let mut len = 0;
        for f in files {
            let file = File::open(f)?;
            let size = file.metadata()?.len();
            starts.push(len);
            len += size;
            parts.push((file, size));
        }
        Ok(Joined { parts, starts, len, pos: 0 })
    }
}

impl Read for Joined {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let i = self.starts.partition_point(|s| *s <= self.pos) - 1;
        let (file, size) = &mut self.parts[i];
        let off = self.pos - self.starts[i];
        file.seek(SeekFrom::Start(off))?;
        let want = buf.len().min((*size - off) as usize);
        let n = file.read(&mut buf[..want])?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for Joined {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let p = match to {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::End(d) => self.len as i128 + d as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        if p < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek before start"));
        }
        self.pos = p as u64;
        Ok(self.pos)
    }
}

/// Anything libarchive can read from.
pub trait Source: Read + Seek + Send {}
impl<T: Read + Seek + Send> Source for T {}

/// Open an archive for reading: a plain file, or the joined volumes of a
/// split. (RAR volume sets aren't joinable; callers send those to `7z`.)
pub fn open(path: &Path) -> io::Result<Box<dyn Source>> {
    match set_of(path) {
        Some(s) if s.kind == Kind::Split => Ok(Box::new(Joined::open(&s.files)?)),
        _ => Ok(Box::new(File::open(path)?)),
    }
}

/// RAR volume set: libarchive can't follow it, `7z` can.
pub fn is_rar_set(path: &Path) -> bool {
    set_of(path).is_some_and(|s| s.kind == Kind::Rar)
}

/// Newest modification time over all volumes (for cache invalidation).
pub fn stamp(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    let files = set_of(path).map_or_else(|| vec![path.to_path_buf()], |s| s.files);
    let mut newest = None;
    let mut total = 0;
    for f in &files {
        let m = std::fs::metadata(f).ok()?;
        total += m.len();
        let t = m.modified().ok()?;
        newest = Some(newest.map_or(t, |n: std::time::SystemTime| n.max(t)));
    }
    newest.map(|t| (t, total))
}

/// Cut `whole` into volumes named like the set's current ones (`base.001`…),
/// `size` bytes each, replacing the old volumes.
pub fn resplit(whole: &Path, set: &Set, size: u64) -> io::Result<()> {
    let first = &set.files[0];
    let name = first.file_name().unwrap_or_default().to_string_lossy().into_owned();
    // Keep the number width the set uses (.001 vs .0001).
    let (base, ext) = name.rsplit_once('.').unwrap_or((&name, "001"));
    let width = ext.len();
    let dir = first.parent().unwrap_or(Path::new("/"));
    let mut src = File::open(whole)?;
    let total = src.metadata()?.len();
    let count = total.div_ceil(size).max(1);
    let mut made = Vec::new();
    let tmp_name = |n: u64| dir.join(format!(".{base}.{n:0width$}.polka-part"));
    let r = (|| -> io::Result<()> {
        for n in 1..=count {
            let p = tmp_name(n);
            let mut out = File::create(&p)?;
            made.push(p);
            io::copy(&mut (&mut src).take(size), &mut out)?;
        }
        Ok(())
    })();
    if let Err(e) = r {
        for p in made {
            let _ = std::fs::remove_file(p);
        }
        return Err(e);
    }
    let perms = std::fs::metadata(first).map(|m| m.permissions()).ok();
    for (i, p) in made.iter().enumerate() {
        let dst = dir.join(format!("{base}.{:0width$}", i + 1));
        if let Some(pm) = &perms {
            let _ = std::fs::set_permissions(p, pm.clone());
        }
        std::fs::rename(p, dst)?;
    }
    // Old volumes past the new end.
    for f in set.files.iter().skip(count as usize) {
        std::fs::remove_file(f)?;
    }
    std::fs::remove_file(whole)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let p = |n: &str| parse(n).map(|v| (v.kind, v.archive, v.index));
        assert_eq!(p("a.7z.001"), Some((Kind::Split, "a.7z".into(), 0)));
        assert_eq!(p("a.7z.012"), Some((Kind::Split, "a.7z".into(), 11)));
        assert_eq!(p("a.part1.rar"), Some((Kind::Rar, "a.rar".into(), 0)));
        assert_eq!(p("A.Part03.RAR"), Some((Kind::Rar, "A.rar".into(), 2)));
        assert_eq!(p("a.rar"), Some((Kind::Rar, "a.rar".into(), 0)));
        assert_eq!(p("a.r00"), Some((Kind::Rar, "a.rar".into(), 1)));
        assert_eq!(p("a.000"), None);
        assert_eq!(p("a.txt"), None);
        assert_eq!(p(".001"), None);

        let g = groups(["x.7z.001", "x.7z.002", "x.7z.004", "solo.rar", "y.rar", "y.r00", "z.part2.rar", "note.txt", "w.zip.002"].into_iter());
        assert_eq!(g["x.7z.004"].count, 3);
        assert!(g["x.7z.001"].gap);
        assert_eq!(g["y.r00"].index, 1);
        assert_eq!(g["y.rar"].key, g["y.r00"].key);
        assert!(g.contains_key("z.part2.rar"));
        assert!(!g.contains_key("solo.rar"));
        assert!(!g.contains_key("w.zip.002")); // no first volume
        assert!(!g.contains_key("note.txt"));
    }

    #[test]
    fn joined_reads_across_parts() {
        let dir = std::env::temp_dir().join(format!("polka-vol-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let data: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        for (i, c) in data.chunks(300).enumerate() {
            std::fs::write(dir.join(format!("d.bin.{:03}", i + 1)), c).unwrap();
        }
        let set = set_of(&dir.join("d.bin.003")).unwrap();
        assert_eq!(set.files.len(), 4);
        assert_eq!(first(&dir.join("d.bin.004")), dir.join("d.bin.001"));
        let mut j = Joined::open(&set.files).unwrap();
        let mut all = Vec::new();
        j.read_to_end(&mut all).unwrap();
        assert_eq!(all, data);
        j.seek(SeekFrom::Start(295)).unwrap();
        let mut b = [0u8; 10];
        j.read_exact(&mut b).unwrap();
        assert_eq!(b[..], data[295..305]);
        j.seek(SeekFrom::End(-3)).unwrap();
        let mut b = Vec::new();
        j.read_to_end(&mut b).unwrap();
        assert_eq!(b, data[997..]);

        // Re-split into bigger volumes: the 4th one goes away.
        let whole = dir.join("whole");
        std::fs::write(&whole, &data).unwrap();
        resplit(&whole, &set, 400).unwrap();
        let set = set_of(&dir.join("d.bin.001")).unwrap();
        assert_eq!(set.files.len(), 3);
        let mut all = Vec::new();
        Joined::open(&set.files).unwrap().read_to_end(&mut all).unwrap();
        assert_eq!(all, data);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
