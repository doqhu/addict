//! The evidence format: one JSON line per fact. The application writes it
//! (`src/evidence.rs`); the harness reads it and appends its own samples.
//!
//! The labels are those of the EXP-4 comparison the format comes from: the
//! product is its GTK4 resident cell, so evidence holds no other backend or
//! mode.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::OpenOptions,
    io::{self, Write},
    path::Path,
    time::SystemTime,
};

/// The version of the record layout.
pub const SCHEMA: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    Gtk4,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Resident,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    Foliate,
    IntelliJ,
    AndroidStudio,
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub schema: u32,
    pub timestamp: String,
    pub backend: Backend,
    pub mode: Mode,
    pub origin: Origin,
    pub invocation_id: Option<String>,
    pub event: String,
    pub details: Value,
}

impl Evidence {
    pub fn new(
        backend: Backend,
        mode: Mode,
        origin: Origin,
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

/// Appends one record as one line. A failed write is fatal to the caller.
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

/// Every line of `path`; a line that does not parse is an error.
pub fn read_evidence(path: &Path) -> io::Result<Vec<Evidence>> {
    let text = std::fs::read_to_string(path)?;
    text.lines()
        .filter(|l| !l.is_empty())
        .map(|line| serde_json::from_str(line).map_err(io::Error::from))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_of_the_recorded_sessions_parse() {
        // As the application and the harness wrote them, in their own field orders.
        let product = r#"{"backend":"gtk4","details":{"application":"addict","boot_id":"b","pid":1,"version":"0.1.0"},"event":"broker-started","invocation_id":null,"mode":"resident","origin":"foliate","schema":1,"timestamp":"1792404046035"}"#;
        let harness = r#"{"schema":1,"timestamp":"1792404048381","backend":"gtk4","mode":"resident","origin":"local","invocation_id":null,"event":"data-sets","details":{"commit":"c"}}"#;
        let path =
            std::env::temp_dir().join(format!("addict-harness-lines-{}", std::process::id()));
        std::fs::write(&path, format!("{product}\n\n{harness}\n")).unwrap();
        let rows = read_evidence(&path);
        std::fs::remove_file(&path).unwrap();
        let rows = rows.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[0].backend, rows[0].mode, rows[0].origin),
            (Backend::Gtk4, Mode::Resident, Origin::Foliate)
        );
        assert_eq!(rows[1].origin, Origin::Local);
        assert!(serde_json::from_str::<Evidence>(&product.replace("gtk4", "gpui")).is_err());
    }
}
