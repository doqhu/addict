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

pub const ENTRY: &str = "io.github.doqhu.Addict.desktop";

/// What an older `addict install` left in the autostart directory.
pub const LEGACY: &str = "[Desktop Entry]\nExec=addict --background\nX-Addict=true\n";

pub const ICON_SIZES: [u32; 4] = [64, 128, 256, 512];

/// The icon of `size` pixels in the data directory `data`.
pub fn icon_in(data: &Path, size: u32) -> PathBuf {
    data.join(format!(
        "icons/hicolor/{size}x{size}/apps/io.github.doqhu.Addict.png"
    ))
}

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

    /// Where an older build wrote its login entry; `install` no longer does.
    pub fn autostart_entry(&self) -> PathBuf {
        self.config_home.join("autostart").join(ENTRY)
    }

    /// The icon of `size` pixels, under the data home.
    pub fn icon(&self, size: u32) -> PathBuf {
        icon_in(&self.data_home, size)
    }

    /// The four icons, smallest first: the order of the output after the
    /// entry or entries.
    pub fn icons(&self) -> [PathBuf; 4] {
        ICON_SIZES.map(|size| self.icon(size))
    }

    /// Where the state of login startup is saved.
    pub fn startup_file(&self) -> PathBuf {
        self.config_home.join("addict/login-startup")
    }

    /// `addict <subcommand>` with no display.
    fn command(&self, subcommand: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_addict"));
        command
            .arg(subcommand)
            .env("HOME", &self.home)
            .env("XDG_DATA_HOME", &self.data_home)
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY");
        command
    }

    /// Runs `addict <subcommand>` with no session bus. The address is set to
    /// nothing rather than removed: without it the program would find the
    /// user's real bus in `XDG_RUNTIME_DIR`, and their real portal there.
    pub fn run(&self, subcommand: &str) -> Output {
        let mut command = self.command(subcommand);
        command.env(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent/addict-test-bus",
        );
        run_with_timeout(command).expect("addict did not exit within the timeout")
    }

    /// Runs `addict <subcommand>` on the session bus of this process, which
    /// must be a private one.
    pub fn run_on_bus(&self, subcommand: &str) -> Output {
        assert!(std::env::var_os(PRIVATE_BUS).is_some(), "not a private bus");
        run_with_timeout(self.command(subcommand)).expect("addict did not exit within the timeout")
    }
}

/// Set in a child that runs on a private bus.
const PRIVATE_BUS: &str = "ADDICT_TEST_PRIVATE_BUS";

/// Whether this test is the child on a private D-Bus daemon. When not, runs
/// itself as one and asserts that it passed. The bus has no service directory:
/// nothing can activate the desktop's real portal on it.
pub fn on_private_bus(test: &str) -> bool {
    if std::env::var_os(PRIVATE_BUS).is_some() {
        return true;
    }
    let dir = TempDir::new("private-bus");
    let config = dir.path().join("bus.conf");
    std::fs::write(
        &config,
        "<busconfig><type>session</type>\
         <listen>unix:tmpdir=/tmp</listen><auth>EXTERNAL</auth>\
         <policy context=\"default\"><allow send_destination=\"*\"/>\
         <allow receive_sender=\"*\"/><allow own=\"*\"/></policy></busconfig>\n",
    )
    .expect("write the bus configuration");
    let mut command = Command::new("dbus-run-session");
    command
        .arg("--config-file")
        .arg(&config)
        .arg("--")
        .arg(std::env::current_exe().expect("test binary path"))
        .args(["--exact", test, "--nocapture", "--test-threads=1"])
        .env(PRIVATE_BUS, "1");
    let output = run_with_timeout(command).expect("the child did not exit within the timeout");
    let stdout = text(&output.stdout);
    assert!(
        output.status.success(),
        "child failed with {}\nstdout: {stdout}\nstderr: {}",
        output.status,
        text(&output.stderr)
    );
    assert!(stdout.contains("1 passed"), "child ran no test: {stdout}");
    false
}

/// Writes `content` to `path`, with the directories it needs.
pub fn place(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("create dir");
    std::fs::write(path, content).expect("write");
}

/// The lines of a captured stdout.
pub fn lines(bytes: &[u8]) -> Vec<String> {
    text(bytes).lines().map(str::to_owned).collect()
}

pub fn shown(path: &Path) -> String {
    path.display().to_string()
}
