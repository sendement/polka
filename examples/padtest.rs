#![allow(dead_code)]
//! Prints actions from the gamepad thread; for checking mappings.
#[path = "../src/input.rs"]
mod input;

fn main() {
    let (tx, rx) = crossbeam_channel::unbounded();
    input::spawn_gamepad(tx, || {});
    let secs: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(10);
    let end = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while let Some(left) = end.checked_duration_since(std::time::Instant::now()) {
        if let Ok(i) = rx.recv_timeout(left) {
            println!("{:?}", i);
        }
    }
}
