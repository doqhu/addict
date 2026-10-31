//! Shared helper of the platform tests that need a child process: the test
//! re-runs its own test binary for one test, with the environment given at
//! spawn, so no process changes its own environment.
#![allow(dead_code)]

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Runs the test `test_name` of this test binary in a child process with the
/// `env` pairs set, and asserts that it passed within `timeout`.
pub fn run_child(test_name: &str, timeout: Duration, env: &[(&str, &OsStr)]) {
    let mut command = Command::new(std::env::current_exe().expect("test binary path"));
    command.args(["--exact", test_name, "--nocapture", "--test-threads=1"]);
    run_test(command, timeout, env);
}

/// Runs one test with a private D-Bus daemon, never the user's session bus.
pub fn run_private_bus(test_name: &str, timeout: Duration, env: &[(&str, &OsStr)]) {
    run_test(private_bus_command(test_name, None), timeout, env);
}

/// [`run_private_bus`] with the bus configuration in the file `config`.
pub fn run_private_bus_with_config(
    test_name: &str,
    config: &Path,
    timeout: Duration,
    env: &[(&str, &OsStr)],
) {
    run_test(private_bus_command(test_name, Some(config)), timeout, env);
}

/// Writes to `path` a session bus configuration that allows everything. Its
/// only service directory is `service_dir`: with `None`, nothing can be
/// activated on the bus.
pub fn write_bus_config(path: &Path, service_dir: Option<&Path>) {
    let service_dir = service_dir.map_or_else(String::new, |dir| {
        format!("<servicedir>{}</servicedir>", dir.display())
    });
    std::fs::write(
        path,
        format!(
            "<busconfig><type>session</type>\
             <listen>unix:tmpdir=/tmp</listen><auth>EXTERNAL</auth>\
             {service_dir}\
             <policy context=\"default\"><allow send_destination=\"*\"/>\
             <allow receive_sender=\"*\"/><allow own=\"*\"/></policy></busconfig>\n"
        ),
    )
    .expect("write the bus configuration");
}

/// Whether this is the child named by the variable `child`; when not, runs
/// `test` as that child on a private bus with no service directory. `tag`
/// names the directory of the bus configuration.
pub fn on_private_bus(child: &str, tag: &str, test: &str) -> bool {
    if std::env::var_os(child).is_some() {
        return true;
    }
    let dir = std::env::temp_dir().join(format!("addict-{tag}-bus-{}-{test}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create the configuration directory");
    let config = dir.join("bus.conf");
    write_bus_config(&config, None);
    run_private_bus_with_config(
        test,
        &config,
        Duration::from_secs(20),
        &[(child, OsStr::new("1"))],
    );
    std::fs::remove_dir_all(&dir).expect("remove the configuration directory");
    false
}

fn private_bus_command(test_name: &str, config: Option<&Path>) -> Command {
    let mut command = Command::new("dbus-run-session");
    if let Some(config) = config {
        command.arg("--config-file").arg(config);
    }
    command
        .arg("--")
        .arg(std::env::current_exe().expect("test binary path"))
        .args(["--exact", test_name, "--nocapture", "--test-threads=1"]);
    command
}

fn run_test(mut command: Command, timeout: Duration, env: &[(&str, &OsStr)]) {
    let mut child = command
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the test binary");
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait().expect("poll the child") {
            Some(_) => break,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the child did not exit within {timeout:?}");
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    // The output of one test fits a pipe buffer.
    let output = child.wait_with_output().expect("collect the child output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "child failed with {}\nstdout: {stdout}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    // Guards against a filter that matched nothing.
    assert!(stdout.contains("1 passed"), "child ran no test: {stdout}");
}
