//! Shared EXP-4 wire contract and measurement rules. No translation integration.
#[cfg(feature = "broker")]
pub mod broker;
#[cfg(feature = "broker")]
pub mod desktop;
pub mod metrics;
pub mod protocol;
pub mod work;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::OpenOptions,
    io::{self, Write},
    path::Path,
    time::SystemTime,
};

pub const SCHEMA: u32 = 1;
pub const FIXTURE_INPUT: &str = "The bank is beside the river. Банк рядом с рекой.";
pub const FIXTURE_RESULT: &str = "bank — берег (fixture). Selectable English / Русский.";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub schema: u32,
    pub timestamp: String,
    pub backend: protocol::Backend,
    pub mode: protocol::Mode,
    pub origin: protocol::Origin,
    pub invocation_id: Option<String>,
    pub event: String,
    pub details: Value,
}

impl Evidence {
    pub fn new(
        backend: protocol::Backend,
        mode: protocol::Mode,
        origin: protocol::Origin,
        invocation_id: Option<String>,
        event: impl Into<String>,
        details: Value,
    ) -> Self {
        Self {
            schema: SCHEMA,
            timestamp: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .to_string(),
            backend,
            mode,
            origin,
            invocation_id,
            event: event.into(),
            details,
        }
    }
}

/// One writer (the broker) owns the append-only log. A failed write is fatal.
pub fn append(path: &Path, record: &Evidence) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_vec(record)?;
    line.push(b'\n');
    let mut file = OpenOptions::new().append(true).create(true).open(path)?;
    file.write_all(&line)?;
    file.flush()
}
