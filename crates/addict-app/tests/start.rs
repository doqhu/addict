//! The scaffold application starts and exits 0 with no display, no session bus
//! and an unreachable runtime directory.

use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(20);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "addict-start-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir(&path).expect("create temp runtime dir");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run_with_timeout(mut command: Command) -> Option<ExitStatus> {
    let mut child = command.spawn().expect("spawn addict");
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.try_wait().expect("poll addict") {
            Some(status) => return Some(status),
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

#[test]
fn starts_and_exits_zero_without_display_or_bus() {
    let runtime_dir = TempDir::new();
    let mut command = Command::new(env!("CARGO_BIN_EXE_addict"));
    command
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .env("XDG_RUNTIME_DIR", &runtime_dir.0)
        .stdin(Stdio::null());
    let status = run_with_timeout(command).expect("addict did not exit within the timeout");
    assert!(status.success(), "addict exited with {status}");
}
