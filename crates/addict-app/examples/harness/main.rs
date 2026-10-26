//! The acceptance harness of the product, run by `scripts/acceptance.sh`: it
//! judges the evidence `addict --evidence` writes and appends the samples the
//! application cannot take of itself. Not shipped: an example of `addict-app`.
//!
//! The evidence tests of `addict-app` include `format.rs`, `metrics.rs` and
//! `environment.rs`, so the rules here are the ones those tests check.
mod environment;
mod format;
mod metrics;

use clap::{Parser, Subcommand};
use format::{Backend, Evidence, Mode, Origin, append};
use serde_json::json;
use std::{io, path::PathBuf, time::Duration};

#[derive(Parser)]
#[command(about = "Acceptance harness of addict; live evidence is required for acceptance")]
struct Cli {
    #[command(subcommand)]
    command: Action,
}
#[derive(Subcommand)]
enum Action {
    /// Append an idle sample of the PSS, RSS and CPU of `roots` and their descendants.
    Sample {
        #[arg(long, value_enum)]
        origin: Origin,
        #[arg(long)]
        results: PathBuf,
        #[arg(long, value_delimiter = ',', required = true)]
        roots: Vec<u32>,
        /// One of the idle states the summary judges.
        #[arg(long, value_parser = metrics::PRODUCT_PHASES)]
        phase: String,
        #[arg(long, default_value_t = 60)]
        idle_seconds: u64,
        /// Explicitly attest roots include all application helpers, never the compositor.
        #[arg(long)]
        inventory_complete: bool,
    },
    /// Judge the product: Foliate and Android Studio, one process, the idle
    /// states before a call and after closing, idle CPU.
    Summarize {
        #[arg(long)]
        results: PathBuf,
        /// Compute the latency of presentations that carry none, by this certificate.
        #[arg(long)]
        clock_mapping: Option<PathBuf>,
    },
    /// Write the clock certificate of this boot from the presses of the latest session.
    Calibrate {
        #[arg(long)]
        results: PathBuf,
        /// Receives clock-evidence.txt and clock-certificate.json.
        #[arg(long)]
        out: PathBuf,
    },
    /// Append the GPU memory of a recorded session's process and the power state of each card.
    Gpu {
        #[arg(long)]
        results: PathBuf,
        #[arg(long, value_delimiter = ',', required = true)]
        roots: Vec<u32>,
    },
    /// Append what a recorded session measures with: each file's name, size and SHA-256.
    DataSets {
        #[arg(long)]
        results: PathBuf,
        #[arg(long, value_delimiter = ',', required = true)]
        roots: Vec<u32>,
        /// The commit of the measured sources, as the caller names it.
        #[arg(long)]
        commit: String,
        #[arg(long, num_args = 1.., required = true)]
        files: Vec<PathBuf>,
    },
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    match run(Cli::parse()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("harness: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
/// The start of the latest session of the product on boot `boot_id` whose
/// process is one of `roots`: a record about that process carries the labels
/// of its session. A pid is reused across boots and by other programs, so a
/// start of another boot or program is not it.
fn recorded_session(records: &[Evidence], roots: &[u32], boot_id: &str) -> io::Result<Evidence> {
    records
        .iter()
        .rev()
        .find(|r| {
            metrics::is_product_start(r)
                && r.details["boot_id"] == boot_id
                && r.details["pid"]
                    .as_u64()
                    .is_some_and(|pid| roots.iter().any(|&root| u64::from(root) == pid))
        })
        .cloned()
        .ok_or_else(|| io::Error::other("no recorded session of these roots"))
}
/// The recorded session of `roots` in `results`, on this boot.
fn session_of(results: &std::path::Path, roots: &[u32]) -> io::Result<Evidence> {
    recorded_session(&format::read_evidence(results)?, roots, &boot_id()?)
}
fn boot_id() -> io::Result<String> {
    Ok(std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .to_owned())
}
/// A sample filed under another origin than its session's would leave both
/// rows wrong, so it is refused.
fn same_origin(session: &Evidence, origin: Origin) -> io::Result<()> {
    if session.origin == origin {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "the roots' session was started with origin {:?}, not {origin:?}",
        session.origin
    )))
}
/// Writes `contents` to `path` through a sibling file renamed over it, so a
/// failed write never leaves half a certificate.
fn write_whole(path: &std::path::Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(".partial");
    let partial = path.with_file_name(name);
    std::fs::write(&partial, contents)?;
    std::fs::rename(&partial, path)
}
async fn run(cli: Cli) -> io::Result<()> {
    match cli.command {
        Action::Sample {
            origin,
            results,
            roots,
            phase,
            idle_seconds,
            inventory_complete,
        } => {
            if idle_seconds > 300 {
                return Err(io::Error::other("idle wait must be <=300 s"));
            }
            same_origin(&session_of(&results, &roots)?, origin)?;
            let sample = metrics::sample(&roots, idle_seconds, inventory_complete).await?;
            append(
                &results,
                &Evidence::new(
                    Backend::Gtk4,
                    Mode::Resident,
                    origin,
                    None,
                    "memory",
                    json!({"phase":phase,"sample":sample}),
                ),
            )
        }
        Action::Summarize {
            results,
            clock_mapping,
        } => {
            let mapping: Option<metrics::ClockMapping> = clock_mapping
                .map(|p| {
                    std::fs::read_to_string(p)
                        .and_then(|s| serde_json::from_str(&s).map_err(io::Error::from))
                })
                .transpose()?;
            // A certificate of another boot or host state gives no summary.
            if let Some(mapping) = mapping.as_ref() {
                let environment = environment::capture_environment(Duration::from_secs(2)).await;
                mapping
                    .validate_environment(&environment)
                    .map_err(io::Error::other)?;
            }
            let records = format::read_evidence(&results)?;
            let summary = metrics::summarize_product(&records, mapping.as_ref());
            println!("{}", serde_json::to_string_pretty(&summary)?);
            Ok(())
        }
        Action::Calibrate { results, out } => {
            let environment = environment::capture_environment(Duration::from_secs(2)).await;
            // A version that cannot be read is not guessed: no certificate.
            let known = |value: &Option<String>, name: &str| {
                value.clone().ok_or_else(|| {
                    io::Error::other(format!("no certificate written: {name} is unknown"))
                })
            };
            std::fs::create_dir_all(&out)?;
            let out = out.canonicalize()?;
            let evidence_path = out.join("clock-evidence.txt");
            let certificate_path = out.join("clock-certificate.json");
            let host = metrics::CalibrationHost {
                boot_id: boot_id()?,
                session_type: known(&environment.session_type, "the session type")?,
                backend_version: known(
                    &environment.xdg_desktop_portal_gnome,
                    "the xdg-desktop-portal-gnome version",
                )?,
                shell_version: known(&environment.gnome_shell, "the GNOME Shell version")?,
                portal_version: known(
                    &environment.xdg_desktop_portal,
                    "the xdg-desktop-portal version",
                )?,
                environment: serde_json::to_string(&environment)?,
                monotonic_ns: metrics::clock_ns(libc::CLOCK_MONOTONIC),
                boottime_ns: metrics::clock_ns(libc::CLOCK_BOOTTIME),
                realtime_ns: metrics::clock_ns(libc::CLOCK_REALTIME),
                evidence_path: evidence_path.to_string_lossy().into_owned(),
            };
            let calibration = metrics::calibrate(&format::read_evidence(&results)?, &host)
                .map_err(|e| io::Error::other(format!("no certificate written: {e}")))?;
            write_whole(&evidence_path, &calibration.evidence)?;
            let mut certificate = serde_json::to_string_pretty(&calibration.mapping)?;
            certificate.push('\n');
            write_whole(&certificate_path, certificate)?;
            println!("{}", evidence_path.display());
            println!("{}", certificate_path.display());
            Ok(())
        }
        Action::Gpu { results, roots } => {
            let session = session_of(&results, &roots)?;
            append(
                &results,
                &Evidence::new(
                    session.backend,
                    session.mode,
                    session.origin,
                    None,
                    "gpu",
                    serde_json::to_value(metrics::gpu(&roots).await?)?,
                ),
            )
        }
        Action::DataSets {
            results,
            roots,
            commit,
            files,
        } => {
            let session = session_of(&results, &roots)?;
            append(
                &results,
                &Evidence::new(
                    session.backend,
                    session.mode,
                    session.origin,
                    None,
                    "data-sets",
                    metrics::data_sets(&files, &commit)?,
                ),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// The command lines `scripts/acceptance.sh` runs, as it runs them.
    const ACCEPTANCE: [&[&str]; 6] = [
        &[
            "harness",
            "summarize",
            "--clock-mapping",
            "c.json",
            "--results",
            "/dev/null",
        ],
        &[
            "harness",
            "sample",
            "--origin",
            "foliate",
            "--roots",
            "42",
            "--phase",
            "after-ocr",
            "--idle-seconds",
            "60",
            "--inventory-complete",
            "--results",
            "r.jsonl",
        ],
        &[
            "harness",
            "calibrate",
            "--results",
            "c.jsonl",
            "--out",
            "clock",
        ],
        &[
            "harness",
            "data-sets",
            "--results",
            "r.jsonl",
            "--roots",
            "42",
            "--commit",
            "c0ffee",
            "--files",
            "a.index",
            "b.index",
        ],
        &["harness", "gpu", "--results", "r.jsonl", "--roots", "42"],
        &[
            "harness",
            "summarize",
            "--results",
            "r.jsonl",
            "--clock-mapping",
            "c.json",
        ],
    ];

    #[test]
    fn the_command_lines_of_the_acceptance_script_parse() {
        for line in ACCEPTANCE {
            assert!(Cli::try_parse_from(line).is_ok(), "{line:?}");
        }
        for phase in metrics::PRODUCT_PHASES {
            let line = [
                "harness",
                "sample",
                "--origin",
                "local",
                "--roots",
                "1",
                "--phase",
                phase,
                "--results",
                "r",
            ];
            assert!(Cli::try_parse_from(line).is_ok(), "{phase}");
        }
    }

    /// A phase the summary does not judge would be recorded and never read.
    #[test]
    fn a_phase_the_summary_does_not_judge_is_refused() {
        let line = [
            "harness",
            "sample",
            "--origin",
            "local",
            "--roots",
            "1",
            "--phase",
            "peak",
            "--results",
            "r",
        ];
        let error = Cli::try_parse_from(line).err().expect("refused");
        assert_eq!(error.kind(), clap::error::ErrorKind::InvalidValue);
    }

    fn start(application: &str, boot: &str, pid: u32, origin: Origin) -> Evidence {
        Evidence::new(
            Backend::Gtk4,
            Mode::Resident,
            origin,
            None,
            "broker-started",
            json!({"application": application, "boot_id": boot, "pid": pid}),
        )
    }

    /// Only the product's latest start of this boot with one of the roots
    /// is the session; a reused pid of another boot or program is not.
    #[test]
    fn a_session_is_the_products_of_this_boot() {
        let records = [
            start("addict", "boot", 42, Origin::Foliate),
            start("addict", "old", 42, Origin::Local),
            start("exp4", "boot", 42, Origin::Local),
            start("addict", "boot", 7, Origin::Local),
        ];
        let session = recorded_session(&records, &[42], "boot").unwrap();
        assert_eq!(session.origin, Origin::Foliate);
        assert_eq!(
            recorded_session(&records, &[7, 42], "boot")
                .unwrap()
                .details["pid"],
            7
        );
        assert!(recorded_session(&records[1..3], &[42], "boot").is_err());
        assert!(recorded_session(&records, &[99], "boot").is_err());
    }

    #[test]
    fn a_sample_of_another_origin_than_its_session_is_refused() {
        let session = start("addict", "boot", 42, Origin::Foliate);
        assert!(same_origin(&session, Origin::Foliate).is_ok());
        let error = same_origin(&session, Origin::Local).unwrap_err();
        assert!(error.to_string().contains("Foliate"), "{error}");
    }

    #[test]
    fn a_whole_write_leaves_no_partial_file() {
        let directory =
            std::env::temp_dir().join(format!("addict-harness-write-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("clock-certificate.json");
        std::fs::write(&path, "old").unwrap();
        write_whole(&path, "new").unwrap();
        let names: Vec<_> = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&directory).unwrap();

        assert_eq!(text, "new");
        assert_eq!(names, ["clock-certificate.json"]);
    }
}
