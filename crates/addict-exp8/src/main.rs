//! `addict-exp8 <manifest.json> <results.jsonl>`: runs the OCR grid of EXP-8
//! on the screenshots of the manifest and writes the results file.
//!
//! Exit 0 when every recognition of the grid ran, whatever it gave; 1 when
//! the run stopped (no Tesseract, no eng data, a line or a scaled image that
//! could not be written, SIGINT or SIGTERM); 2 when nothing was recognized: wrong arguments, a
//! refused manifest, a results file that exists or no temporary directory.

use std::{path::PathBuf, process::ExitCode};

use tokio::signal::unix::{SignalKind, signal};

use addict_exp8::{LIMIT, PAGE_SEGMENTATIONS, SCALES, load_manifest, run};

/// The run stopped after recognitions began.
const STOPPED: u8 = 1;

/// Nothing was recognized.
const NOT_RUN: u8 = 2;

fn main() -> ExitCode {
    let mut arguments = std::env::args_os().skip(1).map(PathBuf::from);
    let (Some(manifest_path), Some(results), None) =
        (arguments.next(), arguments.next(), arguments.next())
    else {
        eprintln!("usage: addict-exp8 <manifest.json> <results.jsonl>");
        return ExitCode::from(NOT_RUN);
    };
    let manifest = match load_manifest(&manifest_path) {
        Ok(manifest) => manifest,
        Err(error) => {
            eprintln!(
                "error: {}: {error}; nothing was recognized",
                manifest_path.display()
            );
            return ExitCode::from(NOT_RUN);
        }
    };
    if results.symlink_metadata().is_ok() {
        eprintln!(
            "error: the results file {} exists already and is not overwritten; nothing was recognized",
            results.display()
        );
        return ExitCode::from(NOT_RUN);
    }
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("error: no runtime: {error}; nothing was recognized");
            return ExitCode::from(NOT_RUN);
        }
    };

    let shots = manifest.screenshots.len();
    println!(
        "manifest {} ({shots} screenshots, SHA-256 {}); {} recognitions",
        manifest_path.display(),
        manifest.sha256,
        shots * SCALES.len() * PAGE_SEGMENTATIONS.len()
    );
    // An interrupt drops the run, so its guards kill the child and remove
    // the scaled images; the lines written so far stay.
    let outcome = runtime.block_on(async {
        let (mut interrupt, mut terminate) = match (
            signal(SignalKind::interrupt()),
            signal(SignalKind::terminate()),
        ) {
            (Ok(interrupt), Ok(terminate)) => (interrupt, terminate),
            (Err(error), _) | (_, Err(error)) => return Err(error),
        };
        Ok(tokio::select! {
            outcome = run(&manifest, &results, LIMIT) => Some(outcome),
            _ = interrupt.recv() => None,
            _ = terminate.recv() => None,
        })
    });
    let outcome = match outcome {
        Ok(Some(outcome)) => outcome,
        Ok(None) => {
            eprintln!(
                "error: the run was interrupted; the results so far are in {}",
                results.display()
            );
            return ExitCode::from(STOPPED);
        }
        Err(error) => {
            eprintln!("error: no signal handler: {error}; nothing was recognized");
            return ExitCode::from(NOT_RUN);
        }
    };
    match outcome {
        Ok(summary) => {
            println!(
                "{} recognitions, {} without text; results: {}",
                summary.recognitions,
                summary.failures,
                results.display()
            );
            ExitCode::SUCCESS
        }
        Err(failure) => {
            eprintln!("error: {failure}");
            ExitCode::from(if failure.before_any_recognition() {
                NOT_RUN
            } else {
                STOPPED
            })
        }
    }
}
