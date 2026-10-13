//! X cursor files (`<theme>/cursors/*`): a table of contents pointing at
//! image chunks, one per nominal size and animation frame, with ARGB pixels
//! (premultiplied alpha).

use std::path::Path;

const MAGIC: &[u8; 4] = b"Xcur";
const IMAGE: u32 = 0xfffd_0002;

pub struct Frame {
    pub size: u32,
    pub width: u32,
    pub height: u32,
    pub hot: (u32, u32),
    /// Milliseconds before the next frame (animated cursors).
    pub delay: u32,
    pub rgba: Vec<u8>,
}

/// Cursor files live in a theme's `cursors` folder and start with "Xcur".
pub fn is_cursor(path: &Path) -> bool {
    use std::io::Read;
    if path.parent().and_then(|p| p.file_name()).is_none_or(|n| n != "cursors") {
        return false;
    }
    let mut magic = [0u8; 4];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && &magic == MAGIC
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// Every image in the file, in file order.
pub fn parse(bytes: &[u8]) -> Option<Vec<Frame>> {
    if bytes.get(..4)? != MAGIC {
        return None;
    }
    let header = u32_at(bytes, 4)? as usize;
    let ntoc = u32_at(bytes, 12)? as usize;
    let mut out = Vec::new();
    for i in 0..ntoc.min(4096) {
        let e = header + i * 12;
        if u32_at(bytes, e)? != IMAGE {
            continue;
        }
        let pos = u32_at(bytes, e + 8)? as usize;
        let size = u32_at(bytes, pos + 8)?;
        let (w, h) = (u32_at(bytes, pos + 16)?, u32_at(bytes, pos + 20)?);
        if w == 0 || h == 0 || w > 1024 || h > 1024 {
            continue;
        }
        let (xh, yh, delay) = (u32_at(bytes, pos + 24)?, u32_at(bytes, pos + 28)?, u32_at(bytes, pos + 32)?);
        let px = bytes.get(pos + 36..pos + 36 + (w * h * 4) as usize)?;
        let mut rgba = Vec::with_capacity(px.len());
        for p in px.chunks_exact(4) {
            // Stored as little-endian ARGB, premultiplied.
            let (b, g, r, a) = (p[0], p[1], p[2], p[3]);
            let un = |c: u8| if a == 0 { 0 } else { ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 };
            rgba.extend_from_slice(&[un(r), un(g), un(b), a]);
        }
        out.push(Frame { size, width: w, height: h, hot: (xh, yh), delay, rgba });
    }
    (!out.is_empty()).then_some(out)
}

/// The frames of the largest size.
pub fn largest(path: &Path) -> Option<Vec<Frame>> {
    let all = parse(&std::fs::read(path).ok()?)?;
    let size = all.iter().map(|f| f.size).max()?;
    Some(all.into_iter().filter(|f| f.size == size).collect())
}

/// All nominal sizes in the file.
pub fn sizes(path: &Path) -> Vec<u32> {
    let mut s: Vec<u32> = std::fs::read(path).ok().and_then(|b| parse(&b)).map(|f| f.iter().map(|f| f.size).collect()).unwrap_or_default();
    s.sort();
    s.dedup();
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-size, two-frame cursor built by hand.
    #[test]
    fn parses_sizes_and_frames() {
        let mut chunks: Vec<(u32, u32, u8)> = vec![(16, 16, 0x80), (32, 32, 0xff), (32, 32, 0x40)];
        let mut b = Vec::new();
        b.extend_from_slice(b"Xcur");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&0x10000u32.to_le_bytes());
        b.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
        let toc_end = 16 + chunks.len() * 12;
        let mut pos = toc_end;
        for (size, w, _) in &chunks {
            b.extend_from_slice(&IMAGE.to_le_bytes());
            b.extend_from_slice(&size.to_le_bytes());
            b.extend_from_slice(&(pos as u32).to_le_bytes());
            pos += 36 + (w * w * 4) as usize;
        }
        for (size, w, alpha) in chunks.drain(..) {
            for v in [36u32, IMAGE, size, 1, w, w, 3, 4, 50] {
                b.extend_from_slice(&v.to_le_bytes());
            }
            for _ in 0..w * w {
                // Premultiplied half-blue.
                let blue = (200u32 * alpha as u32 / 255) as u8;
                b.extend_from_slice(&[blue, 0, 0, alpha]);
            }
        }
        let f = parse(&b).unwrap();
        assert_eq!(f.len(), 3);
        assert_eq!((f[1].width, f[1].hot, f[1].delay), (32, (3, 4), 50));
        assert!((f[0].rgba[2] as i32 - 200).abs() <= 1 && f[0].rgba[3] == 0x80, "un-premultiplied: {:?}", &f[0].rgba[..4]);
        let dir = std::env::temp_dir().join(format!("polka-cur-{}/cursors", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("left_ptr"), &b).unwrap();
        assert!(is_cursor(&dir.join("left_ptr")));
        assert_eq!(largest(&dir.join("left_ptr")).unwrap().len(), 2);
        assert_eq!(sizes(&dir.join("left_ptr")), [16, 32]);
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }
}
