//! Which archives need a password, found out in the background (the lock
//! badge on their icons). Results are kept per path and modification time.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};

#[derive(Default)]
struct State {
    known: HashMap<PathBuf, (i64, bool)>,
    queued: VecDeque<(PathBuf, i64)>,
    wake: Option<egui::Context>,
}

fn state() -> &'static (Mutex<State>, Condvar) {
    static S: OnceLock<(Mutex<State>, Condvar)> = OnceLock::new();
    S.get_or_init(|| {
        std::thread::Builder::new()
            .name("polka-locks".into())
            .spawn(worker)
            .ok();
        (Mutex::new(State::default()), Condvar::new())
    })
}

fn worker() {
    loop {
        let (path, mtime) = {
            let (m, cv) = state();
            let mut st = m.lock().unwrap();
            loop {
                if let Some(job) = st.queued.pop_front() {
                    break job;
                }
                st = cv.wait(st).unwrap();
            }
        };
        let locked = crate::archive_rw::probe_encrypted(&path).unwrap_or(false);
        let (m, _) = state();
        let mut st = m.lock().unwrap();
        st.known.insert(path, (mtime, locked));
        if let Some(c) = &st.wake {
            c.request_repaint();
        }
    }
}

/// Repaint `ctx` when answers come in.
pub fn set_waker(ctx: egui::Context) {
    state().0.lock().unwrap().wake = Some(ctx);
}

/// Is this archive password-protected? None until known (it gets queued).
pub fn locked(path: &Path, mtime: i64) -> Option<bool> {
    let (m, cv) = state();
    let mut st = m.lock().unwrap();
    match st.known.get(path) {
        Some(&(t, v)) if t == mtime => Some(v),
        _ => {
            if !st.queued.iter().any(|(p, _)| p == path) {
                st.queued.push_back((path.to_path_buf(), mtime));
                cv.notify_one();
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_encrypted_archives() {
        if std::process::Command::new("7z").arg("i").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("polka-locks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.txt"), "x").unwrap();
        let make = |name: &str, args: &[&str]| {
            let ok = std::process::Command::new("7z").arg("a").args(args).arg(dir.join(name)).arg("src").current_dir(&dir).stdout(std::process::Stdio::null()).status().unwrap().success();
            assert!(ok);
            dir.join(name)
        };
        let cases = [
            (make("plain.zip", &["-tzip"]), false),
            (make("zc.zip", &["-tzip", "-pq", "-mem=ZipCrypto"]), true),
            (make("aes.zip", &["-tzip", "-pq", "-mem=AES256"]), true),
            (make("plain.7z", &[]), false),
            (make("data.7z", &["-pq"]), true),
            (make("names.7z", &["-pq", "-mhe=on"]), true),
            (make("split.7z", &["-pq", "-v1k"]).with_file_name("split.7z.001"), true),
        ];
        for (p, want) in cases {
            assert_eq!(crate::archive_rw::probe_encrypted(&p).unwrap_or(false), want, "{p:?}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
