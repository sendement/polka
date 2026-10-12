//! Prints actions from the gamepad thread; for checking mappings.
#[path = "../src/input.rs"]
mod input;

fn main() {
    let rx = input::spawn_gamepad(|| {});
    let secs: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(10);
    let end = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while let Some(left) = end.checked_duration_since(std::time::Instant::now()) {
        if let Ok(i) = rx.recv_timeout(left) {
            println!("{:?}", i);
        }
    }
}
