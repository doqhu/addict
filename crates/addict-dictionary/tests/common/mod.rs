//! Shared helpers of the tests that need the imported indexes: where the
//! environment says a file is, and the memory of the test process.
#![allow(dead_code)]

use std::path::PathBuf;

/// The path the environment variable `variable` holds; the test fails when
/// it is not set.
pub fn path_from(variable: &str) -> PathBuf {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("set {variable}"))
}

/// The proportional set size of this process, in kB.
pub fn pss_kb() -> u64 {
    let rollup = std::fs::read_to_string("/proc/self/smaps_rollup").expect("smaps_rollup");
    rollup
        .lines()
        .find_map(|line| line.strip_prefix("Pss:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|kb| kb.parse().ok())
        .expect("a Pss line")
}
