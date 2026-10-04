use std::{
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};

use addict_probe::{
    caps, clipboard, desktop,
    record::{self, Bounded, EXIT_RECORD_NOT_WRITTEN, Finish, Outcome, Progress, Record, bounded},
    screenshot, shortcut,
};
use clap::{Parser, Subcommand};
use serde_json::json;

/// The throwaway app id of the registered run.
const DEFAULT_APP_ID: &str = "dev.addict.Probe";

/// EXP-3 probe: portal GlobalShortcuts, interactive Screenshot and bounded
/// clipboard reads. Every run appends one JSON record to --out.
///
/// Exit codes: 0 reported/activated/captured/text/installed/removed/cancelled,
/// 1 error, 2 usage, 3 timeout, 4 not-bound, 5 unavailable, 6 empty,
/// 7 refused, 9 record not written.
#[derive(Debug, Parser)]
#[command(version, verbatim_doc_comment)]
struct Cli {
    /// Seconds before the run ends with outcome `timeout`. Defaults: caps 10,
    /// shortcut 120, screenshot 60, clipboard 5.
    #[arg(long, global = true, value_parser = parse_timeout)]
    timeout: Option<Duration>,

    /// JSON-lines file the record is appended to.
    #[arg(
        long,
        global = true,
        env = "ADDICT_PROBE_OUT",
        default_value = "addict-probe-results.jsonl"
    )]
    out: PathBuf,

    /// Host app id. For `shortcut` and `screenshot` it is registered with the
    /// portal before the first call; without it nothing is registered. For
    /// the desktop subcommands it defaults to dev.addict.Probe.
    #[arg(long, global = true)]
    app_id: Option<String>,

    /// Free text copied into the record, e.g. which application had focus.
    #[arg(long, global = true)]
    note: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Record portal versions, Screenshot targets, Registry and Wayland globals.
    Caps,
    /// Bind one global shortcut and wait for it to be pressed.
    Shortcut {
        /// Preferred trigger in the XDG shortcuts syntax; the dialog may
        /// offer another.
        #[arg(long, default_value = "CTRL+ALT+d")]
        trigger: String,
        /// Send no preferred trigger at all.
        #[arg(long, conflicts_with = "trigger")]
        no_trigger: bool,
    },
    /// Take one interactive screenshot through the portal.
    Screenshot,
    /// Read the clipboard and the primary selection, bounded.
    Clipboard {
        /// Characters of text kept in the record; 0 keeps none.
        #[arg(long, default_value_t = 40)]
        preview_chars: usize,
    },
    /// Write ~/.local/share/applications/<app-id>.desktop for this binary.
    InstallDesktop,
    /// Remove the desktop entry written by install-desktop.
    UninstallDesktop,
}

fn parse_timeout(text: &str) -> Result<Duration, String> {
    let seconds: f64 = text
        .parse()
        .map_err(|_| "not a number of seconds".to_owned())?;
    if seconds.is_finite() && seconds > 0.0 && seconds <= 86_400.0 {
        Ok(Duration::from_secs_f64(seconds))
    } else {
        Err("must be more than 0 and at most 86400 seconds".to_owned())
    }
}

impl Command {
    fn name(&self) -> &'static str {
        match self {
            Self::Caps => "caps",
            Self::Shortcut { .. } => "shortcut",
            Self::Screenshot => "screenshot",
            Self::Clipboard { .. } => "clipboard",
            Self::InstallDesktop => "install-desktop",
            Self::UninstallDesktop => "uninstall-desktop",
        }
    }

    fn default_timeout(&self) -> Duration {
        Duration::from_secs(match self {
            Self::Shortcut { .. } => 120,
            Self::Screenshot => 60,
            Self::Clipboard { .. } => 5,
            Self::Caps | Self::InstallDesktop | Self::UninstallDesktop => 10,
        })
    }

    fn expected(&self) -> &'static str {
        match self {
            Self::Caps => caps::EXPECTED,
            Self::Shortcut { .. } => shortcut::EXPECTED,
            Self::Screenshot => screenshot::EXPECTED,
            Self::Clipboard { .. } => clipboard::EXPECTED,
            Self::InstallDesktop => desktop::EXPECTED_INSTALL,
            Self::UninstallDesktop => desktop::EXPECTED_UNINSTALL,
        }
    }

    /// Slack after the timeout before the runner gives up on a probe that
    /// bounds its own waits and wants to report what each of them found.
    fn grace(&self) -> Duration {
        match self {
            Self::Clipboard { .. } => Duration::from_millis(300),
            _ => Duration::ZERO,
        }
    }
}

fn main() {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("addict-probe: cannot start the runtime: {e}");
            std::process::exit(1);
        }
    };
    let code = runtime.block_on(run(cli));
    // Exit without waiting for helper threads that may still be blocked on a
    // compositor or a bus that never answered.
    std::process::exit(i32::from(code));
}

async fn run(cli: Cli) -> u8 {
    let started = Instant::now();
    let now = SystemTime::now();
    let timeout = cli.timeout.unwrap_or_else(|| cli.command.default_timeout());
    let deadline = started + timeout;
    let progress = Progress::new();

    let mut inputs = json!({
        "timeout_s": timeout.as_secs_f64(),
        "app_id": cli.app_id,
        "note": cli.note,
    });
    let probe = async {
        match &cli.command {
            Command::Caps => caps::run(&progress).await,
            Command::Shortcut {
                trigger,
                no_trigger,
            } => {
                let trigger = (!no_trigger).then(|| trigger.clone());
                inputs["shortcut_id"] = json!(shortcut::SHORTCUT_ID);
                inputs["preferred_trigger"] = json!(trigger);
                let args = shortcut::Args {
                    app_id: cli.app_id.clone(),
                    trigger,
                };
                shortcut::run(&args, &progress).await
            }
            Command::Screenshot => {
                inputs["options_sent"] = json!(screenshot::SENT_OPTIONS);
                inputs["interactive"] = json!(true);
                let args = screenshot::Args {
                    app_id: cli.app_id.clone(),
                };
                screenshot::run(&args, &progress).await
            }
            Command::Clipboard { preview_chars } => {
                inputs["preview_chars"] = json!(preview_chars);
                let args = clipboard::Args {
                    preview_chars: *preview_chars,
                    deadline,
                };
                clipboard::run(&args, &progress).await
            }
            Command::InstallDesktop | Command::UninstallDesktop => {
                let app_id = cli.app_id.as_deref().unwrap_or(DEFAULT_APP_ID);
                inputs["app_id"] = json!(app_id);
                if app_id == DEFAULT_APP_ID {
                    desktop_command(&cli.command, app_id, &progress)
                } else {
                    // The probe writes and removes only its own entry.
                    Finish::with_error(
                        Outcome::Refused,
                        format!(
                            "the desktop subcommands handle only {DEFAULT_APP_ID}, not {app_id:?}; \
                             nothing was touched"
                        ),
                    )
                }
            }
        }
    };
    let limit = timeout + cli.command.grace();
    let bounded_probe = async {
        match bounded(limit, probe).await {
            Bounded::Done(finish) => finish,
            Bounded::TimedOut => Finish::with_error(
                Outcome::Timeout,
                format!("no result within {} s", timeout.as_secs_f64()),
            ),
        }
    };
    let environment_limit = timeout.min(Duration::from_secs(2));
    let (environment, finish) = tokio::join!(
        record::capture_environment(environment_limit),
        bounded_probe
    );

    let outcome = finish.outcome;
    let record = Record::new(
        cli.command.name(),
        environment,
        inputs,
        cli.command.expected(),
        finish,
        progress.snapshot(),
        started.elapsed(),
        now,
    );
    println!("{}", record.to_line());
    let mut code = outcome.exit_code();
    match record::append(&cli.out, &record) {
        Ok(()) => eprintln!(
            "addict-probe {}: outcome {} (exit {code}); record appended to {}",
            cli.command.name(),
            outcome.as_str(),
            cli.out.display()
        ),
        Err(e) => {
            eprintln!(
                "addict-probe {}: outcome {}, but the record was NOT written to {}: {e}",
                cli.command.name(),
                outcome.as_str(),
                cli.out.display()
            );
            code = EXIT_RECORD_NOT_WRITTEN;
        }
    }
    code
}

fn desktop_command(command: &Command, app_id: &str, progress: &Progress) -> Finish {
    let Some(dir) = desktop::applications_dir() else {
        return Finish::with_error(Outcome::Error, "HOME is not set");
    };
    match command {
        Command::InstallDesktop => {
            let exec = match std::env::current_exe().and_then(|p| p.canonicalize()) {
                Ok(exec) => exec,
                Err(e) => {
                    return Finish::with_error(
                        Outcome::Error,
                        format!("cannot resolve the path of this binary: {e}"),
                    );
                }
            };
            desktop::install(&dir, app_id, &exec, progress)
        }
        _ => desktop::uninstall(&dir, app_id, progress),
    }
}
