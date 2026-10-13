//! `addict-exp5 <set.json> <results.jsonl>`: runs the check set of EXP-5
//! against DeepL with the key of the user and writes the results file.
//!
//! Exit 0 when every request of every item was answered, whatever the
//! verdicts; 1 when the run stopped or a request got no answer; 2 when
//! nothing was asked: wrong arguments, an invalid set, a results file that
//! exists, or no usable key.

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use addict_deepl::{DeepL, key};
use addict_exp5::{Failure, LIMIT, explain, load_set, run};

/// The run stopped, or a request of an item got no answer.
const STOPPED: u8 = 1;

/// Nothing was asked.
const NOT_RUN: u8 = 2;

fn main() -> ExitCode {
    let mut arguments = std::env::args_os().skip(1).map(PathBuf::from);
    let (Some(set_path), Some(results), None) =
        (arguments.next(), arguments.next(), arguments.next())
    else {
        eprintln!("usage: addict-exp5 <set.json> <results.jsonl>");
        return ExitCode::from(NOT_RUN);
    };
    let set = match load_set(&set_path) {
        Ok(set) => set,
        Err(error) => {
            eprintln!("error: {}: {error}; nothing was asked", set_path.display());
            return ExitCode::from(NOT_RUN);
        }
    };
    if results.symlink_metadata().is_ok() {
        eprintln!(
            "error: the results file {} exists already and is not overwritten; nothing was asked",
            results.display()
        );
        return ExitCode::from(NOT_RUN);
    }
    let key_file = key::key_path();
    if let Err(message) = usable_key(key_file.as_deref()) {
        eprintln!("error: {message}; nothing was asked");
        return ExitCode::from(NOT_RUN);
    }
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("error: no runtime: {}", error.kind());
            return ExitCode::from(NOT_RUN);
        }
    };

    println!(
        "set {} ({} items, SHA-256 {})",
        set_path.display(),
        set.set.items.len(),
        set.sha256
    );
    let client = DeepL::new();
    match runtime.block_on(run(&client, &set, &results, LIMIT)) {
        Ok(summary) => {
            println!(
                "usage: {} of {} before, {} of {} after",
                summary.before.character_count,
                summary.before.character_limit,
                summary.after.character_count,
                summary.after.character_limit
            );
            println!(
                "matched with context: {} of {}; without: {} of {}",
                summary.matched_with_context,
                summary.items,
                summary.matched_without_context,
                summary.items
            );
            println!("results: {}", results.display());
            if summary.refused == 0 {
                ExitCode::SUCCESS
            } else {
                eprintln!(
                    "error: {} requests got no answer; their lines hold the refusals",
                    summary.refused
                );
                ExitCode::from(STOPPED)
            }
        }
        Err(failure) => {
            eprintln!("error: {}", explain(&failure, key_file.as_deref()));
            // These two come before any request.
            let code = match failure {
                Failure::Exists(_) | Failure::Results(_) => NOT_RUN,
                _ => STOPPED,
            };
            ExitCode::from(code)
        }
    }
}

/// Whether the key file gives a key, checked before a results file is begun.
/// The message names the file and holds nothing of it.
fn usable_key(key_file: Option<&Path>) -> Result<(), String> {
    let Some(path) = key_file else {
        return Err(
            "no configuration directory is known, so there is no key file: \
             set HOME or XDG_CONFIG_HOME"
                .to_owned(),
        );
    };
    let why = match key::read(path) {
        Ok(_) => return Ok(()),
        Err(key::KeyError::Missing) => "does not exist",
        Err(key::KeyError::Unreadable) => "cannot be read",
        Err(key::KeyError::TooOpen) => "must have mode 0600",
        Err(key::KeyError::Empty) => "holds no key",
    };
    Err(format!("the key file {} {why}", path.display()))
}
