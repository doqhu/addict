//! Shared helper of the platform tests that need a child process: the test
//! re-runs its own test binary for one test, with the environment given at
//! spawn, so no process changes its own environment.
#![allow(dead_code)]

use std::ffi::OsStr;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Runs the test `test_name` of this test binary in a child process with the
/// `env` pairs set, and asserts that it passed within `timeout`.
pub fn run_child(test_name: &str, timeout: Duration, env: &[(&str, &OsStr)]) {
    let mut child = Command::new(std::env::current_exe().expect("test binary path"))
        .args(["--exact", test_name, "--nocapture", "--test-threads=1"])
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
