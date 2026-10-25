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
        #[arg(long,value_parser=["before-invocation","after-close","after-ocr","peak"])]
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
/// The start of the latest session whose process is one of `roots`: a record
/// about that process carries the labels of its session.
fn recorded_session(results: &std::path::Path, roots: &[u32]) -> io::Result<Evidence> {
    format::read_evidence(results)?
        .into_iter()
        .rev()
        .find(|r| {
            r.event == "broker-started"
                && r.details["pid"]
                    .as_u64()
                    .is_some_and(|pid| roots.iter().any(|&root| u64::from(root) == pid))
        })
        .ok_or_else(|| io::Error::other("no recorded session of these roots"))
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
                boot_id: std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?
                    .trim()
                    .to_owned(),
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
            std::fs::write(&evidence_path, &calibration.evidence)?;
            let mut certificate = serde_json::to_string_pretty(&calibration.mapping)?;
            certificate.push('\n');
            std::fs::write(&certificate_path, certificate)?;
            println!("{}", evidence_path.display());
            println!("{}", certificate_path.display());
            Ok(())
        }
        Action::Gpu { results, roots } => {
            let session = recorded_session(&results, &roots)?;
            append(
                &results,
                &Evidence::new(
                    session.backend,
                    session.mode,
                    session.origin,
                    None,
                    "gpu",
                    serde_json::to_value(metrics::gpu(&roots)?)?,
                ),
            )
        }
        Action::DataSets {
            results,
            roots,
            commit,
            files,
        } => {
            let session = recorded_session(&results, &roots)?;
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
