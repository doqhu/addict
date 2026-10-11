//! Shared helpers of the binary tests: a private temporary directory, a
//! bounded run of a child process and the private directories of the entry
//! commands.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

pub const TIMEOUT: Duration = Duration::from_secs(20);

pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "addict-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir(&path).expect("create temp dir");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs the command with captured output; `None` when it had to be killed at
/// the timeout. The output is expected to fit a pipe buffer.
pub fn run_with_timeout(mut command: Command) -> Option<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn addict");
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.try_wait().expect("poll addict") {
            Some(_) => return Some(child.wait_with_output().expect("collect output")),
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

pub fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

pub const ENTRY: &str = "dev.addict.Addict.desktop";

/// The private directories an entry command runs against. All three are set
/// on every run: a variable left to the caller's environment would let the
/// command write under the real home directory.
pub struct Homes {
    pub home: PathBuf,
    pub data_home: PathBuf,
    pub config_home: PathBuf,
}

impl Homes {
    /// `data` and `config` inside `root`, which is `HOME`.
    pub fn inside(root: &Path) -> Self {
        Self {
            home: root.to_path_buf(),
            data_home: root.join("data"),
            config_home: root.join("config"),
        }
    }

    pub fn applications_entry(&self) -> PathBuf {
        self.data_home.join("applications").join(ENTRY)
    }

    pub fn autostart_entry(&self) -> PathBuf {
        self.config_home.join("autostart").join(ENTRY)
    }

    /// Runs `addict <subcommand>` with no display and no session bus.
    pub fn run(&self, subcommand: &str) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_addict"));
        command
            .arg(subcommand)
            .env("HOME", &self.home)
            .env("XDG_DATA_HOME", &self.data_home)
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .env_remove("DBUS_SESSION_BUS_ADDRESS");
        run_with_timeout(command).expect("addict did not exit within the timeout")
    }
}

/// The lines of a captured stdout.
pub fn lines(bytes: &[u8]) -> Vec<String> {
    text(bytes).lines().map(str::to_owned).collect()
}

pub fn shown(path: &Path) -> String {
    path.display().to_string()
}
