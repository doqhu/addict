//! The run of EXP-8 on the OCR fixture: the whole grid, the scaled images
//! removed, and the refusals that come before any recognition. Needs
//! `tesseract` and the eng data.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use addict_exp8::{
    Failure, Fault, LIMIT, ManifestError, TEMP_PREFIX, load_manifest, run_in, sha256_hex,
};
use serde_json::{Value, json};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../experiments/exp4/ocr-fixture.png"
);

const SENTENCE: &str = "The bank is beside the river.";

/// A fresh directory for one test, removed at its end.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("addict-exp8-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("screenshots")).expect("scratch");
        fs::create_dir(path.join("temp")).expect("temp");
        Self(path)
    }

    /// A manifest of `files`, each copied from its source into
    /// `screenshots/`; gives its path.
    fn manifest(&self, files: &[(&str, &[u8])]) -> PathBuf {
        let entries: Vec<Value> = files
            .iter()
            .map(|(name, bytes)| {
                fs::write(self.0.join("screenshots").join(name), bytes).expect("screenshot");
                json!({
                    "file": name,
                    "sha256": sha256_hex(bytes),
                    "source": {"application": "addict", "material": "the OCR fixture of EXP-4"},
                    "display_scale": 1,
                    "word": "bank",
                    "sentence": SENTENCE
                })
            })
            .collect();
        let path = self.0.join("manifest.json");
        fs::write(&path, serde_json::to_vec_pretty(&entries).expect("json")).expect("manifest");
        path
    }

    fn temp(&self) -> PathBuf {
        self.0.join("temp")
    }

    /// The directories of scaled images left in the temp root.
    fn leftovers(&self) -> Vec<PathBuf> {
        fs::read_dir(self.temp())
            .expect("temp")
            .map(|entry| entry.expect("entry").path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(TEMP_PREFIX))
            })
            .collect()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

fn lines(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .expect("results")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON line"))
        .collect()
}

#[test]
fn the_fixture_runs_the_whole_grid() {
    let scratch = Scratch::new("grid");
    let fixture = fs::read(FIXTURE).expect("fixture");
    let manifest_path = scratch.manifest(&[("fixture.png", &fixture)]);
    let manifest = load_manifest(&manifest_path).expect("manifest");
    let results = scratch.0.join("out").join("results.jsonl");

    let summary = runtime()
        .block_on(run_in(&manifest, &results, LIMIT, &scratch.temp()))
        .expect("the run");
    assert_eq!(summary.recognitions, 9);

    let lines = lines(&results);
    assert_eq!(lines.len(), 10);
    let run = &lines[0];
    assert_eq!(run["kind"], "run");
    assert_eq!(
        run["manifest_sha256"],
        sha256_hex(&fs::read(&manifest_path).expect("manifest"))
    );
    assert_eq!(run["screenshots"][0]["sha256"], sha256_hex(&fixture));
    assert_eq!(run["scales"], json!([1, 2, 3]));
    assert_eq!(run["psms"], json!([3, 4, 6]));
    assert_eq!(run["limit_ms"], 15_000);
    assert!(
        run["tesseract"]
            .as_str()
            .is_some_and(|line| line.starts_with("tesseract "))
    );
    assert!(
        run["runner"]
            .as_str()
            .is_some_and(|r| r.starts_with("addict-exp8/"))
    );

    let (width, height) = (
        run["screenshots"][0]["width"].as_u64().expect("width"),
        run["screenshots"][0]["height"].as_u64().expect("height"),
    );
    let mut grid = Vec::new();
    for line in &lines[1..] {
        assert_eq!(line["kind"], "recognition");
        assert_eq!(line["file"], "fixture.png");
        let scale = line["scale"].as_u64().expect("scale");
        assert_eq!(line["width"].as_u64(), Some(width * scale));
        assert_eq!(line["height"].as_u64(), Some(height * scale));
        assert!(line["elapsed_ms"].is_u64());
        grid.push((scale, line["psm"].as_u64().expect("psm")));
    }
    let expected: Vec<(u64, u64)> = [1, 2, 3]
        .into_iter()
        .flat_map(|scale| [3, 4, 6].map(|psm| (scale, psm)))
        .collect();
    assert_eq!(grid, expected);

    let first = lines[1..]
        .iter()
        .find(|line| line["scale"] == 1 && line["psm"] == 6)
        .expect("1× psm 6");
    assert_eq!(first["text"], SENTENCE);
    assert_eq!(first["failure"], Value::Null);
    assert_eq!(first["sentence_edits"], 0);
    assert_eq!(first["word_found"], true);
    assert!(
        first["peak_rss_bytes"]
            .as_u64()
            .is_some_and(|bytes| bytes > 0)
    );

    assert!(scratch.leftovers().is_empty());
}

#[test]
fn an_existing_results_file_is_refused_before_any_recognition() {
    let scratch = Scratch::new("exists");
    let fixture = fs::read(FIXTURE).expect("fixture");
    let manifest =
        load_manifest(&scratch.manifest(&[("fixture.png", &fixture)])).expect("manifest");
    let results = scratch.0.join("results.jsonl");
    fs::write(&results, "kept\n").expect("results");

    let failure = runtime()
        .block_on(run_in(&manifest, &results, LIMIT, &scratch.temp()))
        .expect_err("refused");
    assert!(matches!(failure, Failure::Exists(_)));
    assert!(failure.before_any_recognition());
    assert_eq!(fs::read_to_string(&results).expect("results"), "kept\n");
    assert!(scratch.leftovers().is_empty());
}

#[test]
fn an_undecodable_file_is_refused_before_any_recognition() {
    let scratch = Scratch::new("undecodable");
    let fixture = fs::read(FIXTURE).expect("fixture");
    let broken = &fixture[..fixture.len() / 2];
    let manifest_path = scratch.manifest(&[("fixture.png", &fixture), ("broken.png", broken)]);
    assert!(matches!(
        load_manifest(&manifest_path),
        Err(ManifestError::Entry {
            index: 2,
            fault: Fault::Undecodable(_),
            ..
        })
    ));
}

#[test]
fn the_runner_exits_2_and_writes_nothing_when_refused() {
    let scratch = Scratch::new("binary");
    let results = scratch.0.join("results.jsonl");
    let empty = scratch.0.join("manifest.json");
    fs::write(&empty, "[]").expect("manifest");
    let binary = env!("CARGO_BIN_EXE_addict-exp8");

    let output = Command::new(binary)
        .arg(&empty)
        .arg(&results)
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no entry"));
    assert!(!results.exists());

    let output = Command::new(binary).arg(&empty).output().expect("run");
    assert_eq!(output.status.code(), Some(2));

    let fixture = fs::read(FIXTURE).expect("fixture");
    let manifest = scratch.manifest(&[("fixture.png", &fixture)]);
    fs::write(&results, "kept\n").expect("results");
    let output = Command::new(binary)
        .arg(&manifest)
        .arg(&results)
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(fs::read_to_string(&results).expect("results"), "kept\n");
}

#[test]
fn no_tesseract_saves_the_first_line_and_stops_with_exit_1() {
    let scratch = Scratch::new("no-tesseract");
    let fixture = fs::read(FIXTURE).expect("fixture");
    let manifest = scratch.manifest(&[("fixture.png", &fixture)]);
    let results = scratch.0.join("results.jsonl");

    let output = Command::new(env!("CARGO_BIN_EXE_addict-exp8"))
        .arg(&manifest)
        .arg(&results)
        .env("PATH", scratch.0.join("no-programs-here"))
        .env("TMPDIR", scratch.temp())
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("tesseract is not installed"));

    let lines = lines(&results);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["tesseract"], Value::Null);
    assert_eq!(lines[1]["failure"], "tesseract_missing");
    assert_eq!(lines[1]["text"], Value::Null);
    assert_eq!(lines[1]["sentence_edits"], Value::Null);
    assert!(scratch.leftovers().is_empty());
}

#[test]
fn a_recognition_without_text_is_saved_and_the_run_goes_on() {
    let scratch = Scratch::new("goes-on");
    let mut blank = Vec::new();
    image::RgbImage::from_pixel(200, 60, image::Rgb([255, 255, 255]))
        .write_to(
            &mut std::io::Cursor::new(&mut blank),
            image::ImageFormat::Png,
        )
        .expect("png");
    let fixture = fs::read(FIXTURE).expect("fixture");
    let manifest =
        load_manifest(&scratch.manifest(&[("blank.png", &blank), ("fixture.png", &fixture)]))
            .expect("manifest");
    let results = scratch.0.join("results.jsonl");

    let summary = runtime()
        .block_on(run_in(&manifest, &results, LIMIT, &scratch.temp()))
        .expect("the run");
    assert_eq!(summary.recognitions, 18);
    assert_eq!(summary.failures, 9);

    let lines = lines(&results);
    assert_eq!(lines.len(), 19);
    for line in &lines[1..10] {
        assert_eq!(line["file"], "blank.png");
        assert_eq!(line["failure"], "empty");
        assert_eq!(line["sentence_edits"], Value::Null);
    }
    assert!(lines[10..].iter().all(|line| line["file"] == "fixture.png"));
    assert!(scratch.leftovers().is_empty());
}

#[test]
fn no_language_data_saves_the_first_line_and_stops_with_exit_1() {
    let scratch = Scratch::new("no-language");
    let fixture = fs::read(FIXTURE).expect("fixture");
    let manifest = scratch.manifest(&[("fixture.png", &fixture)]);
    let results = scratch.0.join("results.jsonl");
    let tessdata = scratch.0.join("empty-tessdata");
    fs::create_dir(&tessdata).expect("tessdata");

    let output = Command::new(env!("CARGO_BIN_EXE_addict-exp8"))
        .arg(&manifest)
        .arg(&results)
        .env("TESSDATA_PREFIX", &tessdata)
        .env("TMPDIR", scratch.temp())
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("eng language data is not installed"));

    let lines = lines(&results);
    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[0]["tessdata_prefix"],
        tessdata.to_str().expect("utf-8")
    );
    assert_eq!(lines[1]["failure"], "language_missing");
    assert!(scratch.leftovers().is_empty());
}
