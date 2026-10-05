use std::{
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};

use addict_context::{
    bus, context,
    record::{self, Accessibility, EXIT_RECORD_NOT_WRITTEN, Finish, Outcome, Record},
    tree,
};
use addict_probe::record::{Bounded, Progress, bounded, capture_environment};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};

const EXPECTED_STATUS: &str = "outcome reported: the accessibility bus address, \
     org.a11y.Status IsEnabled and ScreenReaderEnabled, and every registered application with \
     toolkit, version and pid, as the running session answers";
const EXPECTED_TREE: &str = "outcome reported: node count, roles and every Text object of the \
     application with its character and selection counts, and whether the walk was cut by its \
     node or time limit";

/// Share of `--timeout` the tree walk may use before it is cut.
const WALK_SHARE: f64 = 0.7;
/// Longest wait for one application or one node below the root.
const PER_CALL: Duration = Duration::from_secs(2);

/// EXP-1/2 probe: what an application exposes over AT-SPI and, when a word
/// is selected, the word and its sentence. Read-only; every run appends one
/// JSON record to --out.
///
/// Exit codes: 0 reported/context, 1 error, 2 usage, 3 timeout,
/// 4 app-not-found, 5 unavailable, 6 no-text, 7 no-selection, 8 word-only,
/// 9 record not written, 10 ambiguous.
#[derive(Debug, Parser)]
#[command(version, verbatim_doc_comment)]
struct Cli {
    /// Seconds before the run ends with outcome `timeout`, counted after
    /// --delay. Defaults: status 10, tree 20, context 20.
    #[arg(long, global = true, value_parser = parse_timeout)]
    timeout: Option<Duration>,

    /// JSON-lines file the record is appended to.
    #[arg(
        long,
        global = true,
        env = "ADDICT_CONTEXT_OUT",
        default_value = "addict-context-results.jsonl"
    )]
    out: PathBuf,

    /// Free text copied into the record, e.g. the scenario and what was
    /// selected.
    #[arg(long, global = true)]
    note: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Record the accessibility bus, its status and the registered applications.
    Status,
    /// Walk one application and record roles and Text objects.
    Tree {
        #[command(flatten)]
        target: Target,
    },
    /// Read the selected word and its sentence from one application.
    Context {
        #[command(flatten)]
        target: Target,
        /// Seconds to wait before anything is read, to return focus to the
        /// application.
        #[arg(long, default_value = "0", value_parser = parse_delay)]
        delay: Duration,
        /// The word that was selected; adds `word_match` to the record.
        #[arg(long)]
        expect_word: Option<String>,
        /// The sentence around it; adds `sentence_match` to the record.
        #[arg(long)]
        expect_sentence: Option<String>,
        /// Characters read on each side of the selection to derive a sentence.
        #[arg(long, default_value_t = 600)]
        context_chars: usize,
    },
}

#[derive(Debug, clap::Args)]
struct Target {
    /// Registered application name, compared without case; a name that
    /// only contains the text matches when none equals it.
    #[arg(long)]
    app: String,
    /// Nodes read before the walk is cut.
    #[arg(long, default_value_t = 5000)]
    max_nodes: usize,
}

fn parse_seconds(text: &str, allow_zero: bool) -> Result<Duration, String> {
    let seconds: f64 = text
        .parse()
        .map_err(|_| "not a number of seconds".to_owned())?;
    let low_enough = if allow_zero {
        seconds >= 0.0
    } else {
        seconds > 0.0
    };
    if seconds.is_finite() && low_enough && seconds <= 86_400.0 {
        Ok(Duration::from_secs_f64(seconds))
    } else if allow_zero {
        Err("must be between 0 and 86400 seconds".to_owned())
    } else {
        Err("must be more than 0 and at most 86400 seconds".to_owned())
    }
}

fn parse_timeout(text: &str) -> Result<Duration, String> {
    parse_seconds(text, false)
}

fn parse_delay(text: &str) -> Result<Duration, String> {
    parse_seconds(text, true)
}

impl Command {
    fn name(&self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Tree { .. } => "tree",
            Self::Context { .. } => "context",
        }
    }

    fn default_timeout(&self) -> Duration {
        Duration::from_secs(match self {
            Self::Status => 10,
            Self::Tree { .. } | Self::Context { .. } => 20,
        })
    }

    fn expected(&self) -> &'static str {
        match self {
            Self::Status => EXPECTED_STATUS,
            Self::Tree { .. } => EXPECTED_TREE,
            Self::Context { .. } => context::EXPECTED,
        }
    }

    fn delay(&self) -> Duration {
        match self {
            Self::Context { delay, .. } => *delay,
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
            eprintln!("addict-context: cannot start the runtime: {e}");
            std::process::exit(1);
        }
    };
    let code = runtime.block_on(run(cli));
    // Exit without waiting for helper threads that may still be blocked on a
    // bus that never answered.
    std::process::exit(i32::from(code));
}

async fn probe(command: &Command, timeout: Duration, progress: &Progress) -> Finish {
    let connection = match bus::open(progress).await {
        Ok(connection) => connection,
        Err(finish) => return finish,
    };
    let per_call = PER_CALL.min(timeout / 4);
    let (target, context_args) = match command {
        Command::Status => {
            return match bus::applications(&connection, per_call, progress).await {
                Ok(applications) => {
                    progress.put("application_count", applications.len());
                    progress.put("applications", &applications);
                    progress.phase("done");
                    Finish::ok(Outcome::Reported)
                }
                Err(e) => Finish::with_error(Outcome::Unavailable, e),
            };
        }
        Command::Tree { target } => (target, None),
        Command::Context {
            target,
            expect_word,
            expect_sentence,
            context_chars,
            ..
        } => (
            target,
            Some(context::Args {
                context_chars: *context_chars,
                expect_word: expect_word.clone(),
                expect_sentence: expect_sentence.clone(),
                per_call,
            }),
        ),
    };

    let matched = match bus::find(&connection, &target.app, per_call, progress).await {
        Ok(matched) => matched,
        Err(finish) => return finish,
    };
    let roots: Vec<bus::Object> = matched
        .iter()
        .map(|a| bus::Object::new(&connection, &a.bus_name, &a.path))
        .collect();
    let limits = tree::Limits {
        max_nodes: target.max_nodes,
        stop_at: Instant::now() + timeout.mul_f64(WALK_SHARE),
        per_node: per_call,
    };
    let walk = tree::walk(&roots, limits, progress).await;
    match context_args {
        None => {
            progress.phase("done");
            Finish::ok(Outcome::Reported)
        }
        Some(args) => context::run(&walk, &args, progress).await,
    }
}

/// First line of a successful command's stdout; `None` when the command is
/// missing, fails or overruns. Read-only commands only.
async fn command_line(limit: Duration, program: &str, args: &[&str]) -> Option<String> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    match bounded(limit, command.output()).await {
        Bounded::Done(Ok(output)) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .map(|line| line.trim().to_owned())
                .filter(|line| !line.is_empty())
        }
        _ => None,
    }
}

async fn run(cli: Cli) -> u8 {
    let started = Instant::now();
    let now = SystemTime::now();
    let delay = cli.command.delay();
    let timeout = cli.timeout.unwrap_or_else(|| cli.command.default_timeout());
    let progress = Progress::new();

    let mut inputs = json!({
        "timeout_s": timeout.as_secs_f64(),
        "note": cli.note,
    });
    match &cli.command {
        Command::Status => {}
        Command::Tree { target } => {
            inputs["app"] = json!(target.app);
            inputs["max_nodes"] = json!(target.max_nodes);
        }
        Command::Context {
            target,
            delay,
            expect_word,
            expect_sentence,
            context_chars,
        } => {
            inputs["app"] = json!(target.app);
            inputs["max_nodes"] = json!(target.max_nodes);
            inputs["delay_s"] = json!(delay.as_secs_f64());
            inputs["expect_word"] = json!(expect_word);
            inputs["expect_sentence"] = json!(expect_sentence);
            inputs["context_chars"] = json!(context_chars);
        }
    }

    // Nothing is asked of any bus before the delay has passed.
    if !delay.is_zero() {
        eprintln!(
            "addict-context: waiting {} s before reading; return focus to the application",
            delay.as_secs_f64()
        );
        tokio::time::sleep(delay).await;
    }
    let waited = started.elapsed();
    if !delay.is_zero() {
        inputs["delay_waited_ms"] = json!(u64::try_from(waited.as_millis()).unwrap_or(u64::MAX));
    }

    let bounded_probe = async {
        match bounded(timeout, probe(&cli.command, timeout, &progress)).await {
            Bounded::Done(finish) => finish,
            Bounded::TimedOut => Finish::with_error(
                Outcome::Timeout,
                format!("no result within {} s", timeout.as_secs_f64()),
            ),
        }
    };
    let side_limit = timeout.min(Duration::from_secs(2));
    let (environment, at_spi2_core, setting, finish) = tokio::join!(
        capture_environment(side_limit),
        command_line(
            side_limit,
            "rpm",
            &[
                "-q",
                "--qf",
                "%{VERSION}-%{RELEASE}.%{ARCH}\n",
                "at-spi2-core"
            ]
        ),
        command_line(
            side_limit,
            "gsettings",
            &[
                "get",
                "org.gnome.desktop.interface",
                "toolkit-accessibility"
            ]
        ),
        bounded_probe
    );

    let mut details = progress.snapshot();
    let mut take = |key: &str| {
        details
            .as_object_mut()
            .and_then(|map| map.remove(key))
            .unwrap_or(Value::Null)
    };
    let mut accessibility: Accessibility =
        serde_json::from_value(take(bus::ACCESSIBILITY_KEY)).unwrap_or_default();
    accessibility.at_spi2_core = at_spi2_core;
    accessibility.toolkit_accessibility_setting = setting;
    let target = take(bus::TARGET_KEY);

    let outcome = finish.outcome;
    let record = Record::new(
        cli.command.name(),
        environment,
        accessibility,
        target,
        inputs,
        cli.command.expected(),
        finish,
        details,
        started.elapsed(),
        now,
    );
    println!("{}", record.to_line());
    let mut code = outcome.exit_code();
    match record::append(&cli.out, &record) {
        Ok(()) => eprintln!(
            "addict-context {}: outcome {} (exit {code}); record appended to {}",
            cli.command.name(),
            outcome.as_str(),
            cli.out.display()
        ),
        Err(e) => {
            eprintln!(
                "addict-context {}: outcome {}, but the record was NOT written to {}: {e}",
                cli.command.name(),
                outcome.as_str(),
                cli.out.display()
            );
            code = EXIT_RECORD_NOT_WRITTEN;
        }
    }
    code
}
