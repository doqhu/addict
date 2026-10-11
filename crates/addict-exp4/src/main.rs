use addict_exp4::{
    Evidence, append, broker, desktop, metrics,
    protocol::{Backend, Mode, Origin},
};
use clap::{Parser, Subcommand};
use serde_json::json;
use std::{io, path::PathBuf, time::Duration};
#[derive(Parser)]
#[command(about = "EXP-4 GPUI/GTK4 comparison; live evidence is required for acceptance")]
struct Cli {
    #[command(subcommand)]
    command: Action,
}
#[derive(Subcommand)]
enum Action {
    Broker {
        #[arg(long, value_enum)]
        backend: Backend,
        #[arg(long, value_enum)]
        mode: Mode,
        #[arg(long, value_enum, default_value = "local")]
        origin: Origin,
        #[arg(long)]
        frontend: PathBuf,
        #[arg(long)]
        socket: PathBuf,
        #[arg(long)]
        results: PathBuf,
        #[arg(long)]
        trigger: Option<String>,
        #[arg(long, default_value_t = 1800)]
        timeout_seconds: u64,
        #[arg(long, default_value_t = 60)]
        binding_timeout_seconds: u64,
        #[arg(long)]
        local_only: bool,
        #[arg(long)]
        clock_mapping: Option<PathBuf>,
        #[arg(long, default_value = "experiments/exp4/ocr-fixture.png")]
        fixture: PathBuf,
        /// Restrict the frontend's Vulkan loader to matching drivers, e.g. 'intel_icd*'.
        #[arg(long)]
        vulkan_drivers: Option<String>,
    },
    Invoke {
        #[arg(long)]
        socket: PathBuf,
        #[arg(long, value_enum, default_value = "local")]
        origin: Origin,
    },
    Quit {
        #[arg(long)]
        socket: PathBuf,
    },
    InstallDesktop {
        #[arg(long, value_enum)]
        backend: Backend,
        #[arg(long)]
        directory: Option<PathBuf>,
    },
    UninstallDesktop {
        #[arg(long, value_enum)]
        backend: Backend,
        #[arg(long)]
        directory: Option<PathBuf>,
    },
    Sample {
        #[arg(long, value_enum)]
        backend: Backend,
        #[arg(long, value_enum)]
        mode: Mode,
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
    Summarize {
        #[arg(long)]
        results: PathBuf,
        /// Compute the latency of presentations that carry none, by this certificate.
        #[arg(long)]
        clock_mapping: Option<PathBuf>,
    },
    HumanScenarios {
        #[arg(long, value_enum)]
        backend: Backend,
        #[arg(long, value_enum)]
        mode: Mode,
        #[arg(long, value_enum)]
        origin: Origin,
        #[arg(long)]
        results: PathBuf,
        /// JSON booleans for the seven runbook checks, plus optional notes.
        #[arg(long)]
        checks: PathBuf,
    },
    Environment,
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    match run(Cli::parse()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("EXP-4: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
async fn run(cli: Cli) -> io::Result<()> {
    match cli.command {
        Action::Broker {
            backend,
            mode,
            origin,
            frontend,
            socket,
            results,
            trigger,
            timeout_seconds,
            binding_timeout_seconds,
            local_only,
            clock_mapping,
            fixture,
            vulkan_drivers,
        } => {
            let mapping = clock_mapping
                .map(|p| {
                    std::fs::read_to_string(p)
                        .and_then(|s| serde_json::from_str(&s).map_err(io::Error::from))
                })
                .transpose()?;
            broker::run(broker::Args {
                backend,
                mode,
                origin,
                frontend,
                socket,
                results,
                trigger,
                timeout: Duration::from_secs(timeout_seconds),
                binding_timeout: Duration::from_secs(binding_timeout_seconds),
                local_only,
                clock_mapping: mapping,
                fixture,
                vulkan_drivers,
            })
            .await
        }
        Action::Invoke { socket, origin } => {
            println!(
                "{}",
                serde_json::to_string(&broker::control(&socket, origin, false).await?)?
            );
            Ok(())
        }
        Action::Quit { socket } => {
            println!(
                "{}",
                serde_json::to_string(&broker::control(&socket, Origin::Local, true).await?)?
            );
            Ok(())
        }
        Action::InstallDesktop { backend, directory } => {
            let dir = directory
                .or_else(addict_probe::desktop::applications_dir)
                .ok_or_else(|| io::Error::other("HOME is missing"))?;
            println!(
                "{}",
                desktop::install(&dir, backend, &std::env::current_exe()?)?.display()
            );
            Ok(())
        }
        Action::UninstallDesktop { backend, directory } => {
            let dir = directory
                .or_else(addict_probe::desktop::applications_dir)
                .ok_or_else(|| io::Error::other("HOME is missing"))?;
            println!("{}", desktop::uninstall(&dir, backend)?.display());
            Ok(())
        }
        Action::Sample {
            backend,
            mode,
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
                    backend,
                    mode,
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
            // A certificate the broker would reject at start gives no summary.
            if let Some(mapping) = mapping.as_ref() {
                let environment =
                    addict_probe::record::capture_environment(Duration::from_secs(2)).await;
                mapping
                    .validate_environment(&environment)
                    .map_err(io::Error::other)?;
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&metrics::summarize(
                    &metrics::read_evidence(&results)?,
                    mapping.as_ref()
                ))?
            );
            Ok(())
        }
        Action::HumanScenarios {
            backend,
            mode,
            origin,
            results,
            checks,
        } => {
            let checks: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(checks)?)?;
            let keys = [
                "focus",
                "esc_close_reopen",
                "english_russian_editing_selection",
                "paste_copy",
                "scaling",
                "delayed_work",
                "ocr_cleanup",
            ];
            let all_passed = keys.iter().all(|key| checks[*key] == true);
            append(
                &results,
                &Evidence::new(
                    backend,
                    mode,
                    origin,
                    None,
                    "human-scenarios",
                    json!({"all_passed":all_passed,"checks":checks,"provenance":"explicit human runbook observation"}),
                ),
            )
        }
        Action::Environment => {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &addict_probe::record::capture_environment(Duration::from_secs(2)).await
                )?
            );
            Ok(())
        }
    }
}
