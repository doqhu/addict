//! Builds the assets of the data release of the indexes, which the
//! application downloads on its first run: for each of the sources in
//! `addict_dictionary::sources` the upstream file is downloaded from the URL
//! of its row and imported, as `addict import` does, and the index is
//! compressed with `xz`. Then `manifest.json`, `WNDB_License.txt` and
//! `release-body.md` are written beside them. Nothing is uploaded: the
//! maintainer does that, as `docs/index-release.md` says.
//!
//! ```text
//! cargo run -p addict-dictionary --example build-indexes -- --out <DIR>
//! ```
//!
//! `--out` defaults to `target/indexes`. A source that fails (its download,
//! its import or its compression) does not stop the run: its downloaded
//! file is removed and the next source is built. The manifest and the
//! release page list only the indexes built, and are written even when
//! every source failed. The paths of the assets go to stdout, and one line
//! per failed source, naming its index, to stderr.
//!
//! The exit status is 0 when every source was built, 1 when any failed or
//! the run could not start (the `--out` directory or the HTTP client could
//! not be made) and 2 for a usage error.

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    time::Duration,
};

use addict_dictionary::{
    format,
    import::{self, Day, KNOWN, Origin},
    release::{self, MANIFEST, Manifest, Published},
    sources::{self, Row},
};

/// Where the assets go without `--out`.
const DEFAULT_OUT: &str = "target/indexes";

/// The Princeton WordNet license, which ships beside the OEWN index.
const WNDB_LICENSE: &str = "WNDB_License.txt";

/// The text of the release page.
const RELEASE_BODY: &str = "release-body.md";

/// The directory inside `--out` that holds the downloads while they are
/// imported; it is removed when the run ends, if empty.
const DOWNLOADS: &str = ".downloads";

/// How long one download may take.
const DOWNLOAD_LIMIT: Duration = Duration::from_secs(30 * 60);

fn main() -> ExitCode {
    let out = match out_dir(std::env::args().skip(1)) {
        Ok(out) => out,
        Err(message) => {
            eprintln!("build-indexes: {message}");
            eprintln!("usage: build-indexes [--out <DIR>]");
            return ExitCode::from(2);
        }
    };
    let report = client().and_then(|http| build(&out, sources::ROWS, &http));
    let report = match report {
        Ok(report) => report,
        Err(message) => {
            eprintln!("build-indexes: {message}");
            return ExitCode::FAILURE;
        }
    };
    for published in &report.manifest.indexes {
        println!("{}", out.join(published.asset()).display());
    }
    for name in [MANIFEST, WNDB_LICENSE, RELEASE_BODY] {
        println!("{}", out.join(name).display());
    }
    if report.failed.is_empty() {
        return ExitCode::SUCCESS;
    }
    for (index, error) in &report.failed {
        eprintln!("build-indexes: {index}: {error}");
    }
    let names: Vec<&str> = report.failed.iter().map(|(index, _)| *index).collect();
    eprintln!(
        "build-indexes: {} of {} sources failed: {}",
        names.len(),
        sources::ROWS.len(),
        names.join(", ")
    );
    ExitCode::FAILURE
}

/// What a run built and what failed.
#[derive(Debug)]
struct Report {
    /// The manifest written, of the indexes built.
    manifest: Manifest,
    /// Each source that failed: its index and the error.
    failed: Vec<(&'static str, String)>,
}

/// The directory of `--out <DIR>`, or the default.
fn out_dir(mut args: impl Iterator<Item = String>) -> Result<PathBuf, String> {
    let mut out = PathBuf::from(DEFAULT_OUT);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = args.next().ok_or("--out needs a directory")?.into(),
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    Ok(out)
}

/// The HTTP client of the downloads.
fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(concat!("addict-build-indexes/", env!("CARGO_PKG_VERSION")))
        .timeout(DOWNLOAD_LIMIT)
        .build()
        .map_err(|error| format!("cannot start the HTTP client: {error}"))
}

/// Builds the index of every row of `rows` into `out`, going on after a
/// source that fails, then writes the manifest, the Princeton license and
/// the release page of the indexes built. The error is one that stops the
/// whole run: `out` cannot be made or an asset beside the indexes cannot be
/// written.
fn build(out: &Path, rows: &[Row], http: &reqwest::blocking::Client) -> Result<Report, String> {
    fs::create_dir_all(out).map_err(|error| format!("cannot make {}: {error}", out.display()))?;
    let mut manifest = Manifest {
        format: format::VERSION,
        indexes: Vec::new(),
    };
    let mut built: Vec<(&Row, String)> = Vec::new();
    let mut failed = Vec::new();
    for row in rows {
        match build_one(http, row, out) {
            Ok((published, edition)) => {
                manifest.indexes.push(published);
                built.push((row, edition));
            }
            Err(error) => failed.push((row.index, error)),
        }
    }
    let _ = fs::remove_dir(out.join(DOWNLOADS));
    write(&out.join(MANIFEST), manifest.to_json().as_bytes())?;
    write(
        &out.join(WNDB_LICENSE),
        include_bytes!("WNDB_License.txt").as_slice(),
    )?;
    write(&out.join(RELEASE_BODY), release_body(&built).as_bytes())?;
    Ok(Report { manifest, failed })
}

/// Downloads, imports and compresses the index of `row` into `out`; gives
/// the published index and the edition of the file it was built from. The
/// asset of an earlier run is removed first, so a source that fails leaves
/// no asset in `out` that the manifest does not list.
fn build_one(
    http: &reqwest::blocking::Client,
    row: &Row,
    out: &Path,
) -> Result<(Published, String), String> {
    let asset = out.join(release::asset_name(row.index));
    match fs::remove_file(&asset) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot remove {}: {error}", asset.display())),
    }
    let url = row.url.ok_or("no download URL")?;
    let source = KNOWN
        .iter()
        .map(|known| known.source)
        .find(|source| sources::row(*source).index == row.index)
        .ok_or("no source has this index")?;
    let name = url.rsplit('/').next().unwrap_or(url);
    let downloads = out.join(DOWNLOADS);
    fs::create_dir_all(&downloads)
        .map_err(|error| format!("cannot make {}: {error}", downloads.display()))?;
    let file = downloads.join(name);
    let imported = download(http, url, &file).and_then(|published| {
        import::import(&file, out, Origin::Download { source, published })
            .map_err(|error| error.to_string())
    });
    // The downloaded file is removed whether or not it was imported.
    let _ = fs::remove_file(&file);
    let _ = fs::remove_file(file.with_extension("part"));
    let imported = imported?;
    let expected = out.join(format!("{}.index", row.index));
    if imported.path != expected {
        return Err(format!(
            "{url} was imported as {}, not as {}",
            imported.path.display(),
            expected.display()
        ));
    }
    xz(&imported.path)?;
    let asset = out.join(release::asset_name(row.index));
    let bytes =
        fs::read(&asset).map_err(|error| format!("cannot read {}: {error}", asset.display()))?;
    Ok((Published::of(row.index, &bytes), imported.edition))
}

/// Downloads `url` into `file`, through a temporary file beside it; gives
/// the day of its `Last-Modified` header, `None` when it is missing or not
/// an HTTP date.
fn download(
    http: &reqwest::blocking::Client,
    url: &str,
    file: &Path,
) -> Result<Option<Day>, String> {
    let mut response = http
        .get(url)
        .send()
        .map_err(|error| format!("cannot download {url}: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("cannot download {url}: HTTP {status}"));
    }
    let published = response
        .headers()
        .get(reqwest::header::LAST_MODIFIED)
        .and_then(|value| value.to_str().ok())
        .and_then(Day::from_http_date);
    let partial = file.with_extension("part");
    let written = fs::File::create(&partial).and_then(|mut target| {
        io::copy(&mut response, &mut target)?;
        target.flush()?;
        target.sync_all()
    });
    match written.and_then(|()| fs::rename(&partial, file)) {
        Ok(()) => Ok(published),
        Err(error) => {
            let _ = fs::remove_file(&partial);
            Err(format!("cannot download {url}: {error}"))
        }
    }
}

/// Compresses `index` with `xz` into `<index>.xz`, keeping `index`.
fn xz(index: &Path) -> Result<(), String> {
    let status = Command::new("xz")
        .args(["-9", "-T0", "--keep", "--force"])
        .arg(index)
        .status()
        .map_err(|error| format!("cannot run xz: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("xz {} failed: {status}", index.display()))
    }
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::write(path, bytes).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

/// The text of the release page: per index its credit, its author, its
/// license and the address of its text, its source, its upstream file and
/// that it was modified, then the warranty disclaimer. Each index carries
/// the same notice in its file. Each row comes with the edition of the file
/// its index was built from.
fn release_body(rows: &[(&Row, String)]) -> String {
    let mut body = String::new();
    body.push_str(&format!(
        "Dictionary indexes of [addict](https://github.com/doqhu/addict) in index format {}.\n\n",
        format::VERSION
    ));
    body.push_str(
        "The application downloads these files on its first run. Each `.index.xz` is an xz-compressed \
         index; `manifest.json` gives the size and the SHA-256 of each. Each index also carries \
         the license notice below in its file.\n",
    );
    for (row, edition) in rows {
        body.push_str(&format!("\n## {}\n\n", release::asset_name(row.index)));
        body.push_str(&format!("- Credit: {}\n", row.credit));
        body.push_str(&format!("- Author: {}\n", row.author));
        body.push_str(&format!("- License: {}\n", row.license));
        body.push_str(&format!("- License text: {}\n", row.license_uri));
        body.push_str(&format!("- Source: {}\n", row.link));
        body.push_str(&format!(
            "- Upstream: {} ({})\n",
            row.url.unwrap_or(row.upstream),
            edition
        ));
        body.push_str(&format!("- Modified: {}\n", sources::MODIFIED));
        if row.index == "oewn" {
            body.push_str(&format!(
                "- The Princeton WordNet license ships beside the index as `{WNDB_LICENSE}`.\n"
            ));
        }
    }
    body.push_str(&format!(
        "\n## No warranty\n\nEach index carries this disclaimer in its license notice:\n\n> {}\n",
        sources::DISCLAIMER
    ));
    body
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        io::{BufRead, BufReader},
        net::TcpListener,
        sync::Arc,
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    use addict_dictionary::import::{MIN_KAIKKI_HEADWORDS, Source};
    use flate2::{Compression, write::GzEncoder};

    use super::*;

    /// What the stub server answers for a path: the status line, the body
    /// and the `Last-Modified` header, if any.
    type Answer = (&'static str, Vec<u8>, Option<&'static str>);

    /// Serves `answers` over HTTP/1.1 on a free port of 127.0.0.1, one
    /// request per connection, `404 Not Found` for any other path; gives the
    /// base URL. The server thread lives as long as the test process.
    fn serve(answers: HashMap<&'static str, Answer>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binding the stub server");
        let base = format!("http://{}", listener.local_addr().expect("its address"));
        let answers = Arc::new(answers);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().expect("the stream"));
                let mut request = String::new();
                if reader.read_line(&mut request).is_err() {
                    continue;
                }
                // The headers end with an empty line.
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
                    line.clear();
                }
                let path = request.split(' ').nth(1).unwrap_or("/");
                let missing = ("404 Not Found", b"not found".to_vec(), None);
                let (status, body, modified) = answers.get(path).unwrap_or(&missing);
                let mut head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.len()
                );
                if let Some(modified) = modified {
                    head.push_str(&format!("Last-Modified: {modified}\r\n"));
                }
                head.push_str("\r\n");
                let _ = stream
                    .write_all(head.as_bytes())
                    .and_then(|()| stream.write_all(body))
                    .and_then(|()| stream.flush());
            }
        });
        base
    }

    /// A Kaikki file of `language` that the importer accepts by its content:
    /// a gzip of just enough entries, each a headword with one gloss.
    fn kaikki(language: &str) -> Vec<u8> {
        let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
        for n in 0..MIN_KAIKKI_HEADWORDS {
            writeln!(
                gz,
                r#"{{"word":"w{n}","lang_code":"{language}","pos":"noun","senses":[{{"glosses":["g{n}"]}}]}}"#
            )
            .expect("gzip");
        }
        gz.finish().expect("gzip")
    }

    /// The row of `source` with the download URL `url`.
    fn at(source: Source, url: String) -> Row {
        Row {
            url: Some(Box::leak(url.into_boxed_str())),
            ..*sources::row(source)
        }
    }

    /// A directory under the system's temporary directory, removed when
    /// dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("the clock")
                .as_nanos();
            let dir = std::env::temp_dir().join(format!(
                "addict-build-indexes-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&dir).expect("making the temporary directory");
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Of four sources, the two good Kaikki files are built and listed, and
    /// the 404 and the FreeDict file of the right size but the wrong hash
    /// fail by name without stopping the run or leaving their downloads.
    #[test]
    fn a_failed_source_is_named_and_the_others_are_built() {
        const CATALAN: &str = "/kaikki.org-dictionary-Catalan.jsonl.gz";
        const SPANISH: &str = "/kaikki.org-dictionary-Spanish.jsonl.gz";
        const FREEDICT: &str = "/freedict-eng-rus-2025.11.23.src.tar.xz";
        const MODIFIED: Option<&str> = Some("Mon, 02 Nov 2026 10:00:00 GMT");
        let freedict_size = KNOWN
            .iter()
            .find(|known| known.source == Source::Freedict)
            .expect("the FreeDict pin")
            .size;
        let base = serve(HashMap::from([
            (CATALAN, ("200 OK", kaikki("ca"), MODIFIED)),
            (SPANISH, ("200 OK", kaikki("es"), MODIFIED)),
            (
                FREEDICT,
                ("200 OK", vec![0; freedict_size as usize], MODIFIED),
            ),
        ]));
        let rows = [
            at(Source::Freedict, format!("{base}{FREEDICT}")),
            at(Source::KaikkiEnSpanish, format!("{base}{SPANISH}")),
            at(Source::Oewn, format!("{base}/english-wordnet-2025.xml.gz")),
            at(Source::KaikkiEnCatalan, format!("{base}{CATALAN}")),
        ];
        let dir = TempDir::new();
        let out = dir.0.join("indexes");
        // An asset of an earlier run, of a source that now fails.
        fs::create_dir_all(&out).expect("the out directory");
        fs::write(out.join("freedict.index.xz"), b"stale").expect("a stale asset");
        let http = client().expect("the HTTP client");

        let report = build(&out, &rows, &http).expect("the run");

        let names = |manifest: &Manifest| -> Vec<String> {
            manifest
                .indexes
                .iter()
                .map(|index| index.name.clone())
                .collect()
        };
        let built = ["kaikki-en-Spanish", "kaikki-en-Catalan"];
        assert_eq!(names(&report.manifest), built);
        let json = fs::read(out.join(MANIFEST)).expect("the manifest");
        let manifest = Manifest::parse(&json).expect("a valid manifest");
        assert_eq!(manifest, report.manifest);
        for published in &manifest.indexes {
            let bytes = fs::read(out.join(published.asset())).expect("the asset");
            assert_eq!(Published::of(&published.name, &bytes), *published);
        }

        let failed: Vec<&str> = report.failed.iter().map(|(index, _)| *index).collect();
        assert_eq!(failed, ["freedict", "oewn"], "{:?}", report.failed);
        let error = |index: &str| {
            &report
                .failed
                .iter()
                .find(|(failed, _)| *failed == index)
                .expect("a failure")
                .1
        };
        assert!(error("oewn").contains("404"), "{}", error("oewn"));
        assert!(
            error("freedict").contains("SHA-256"),
            "{}",
            error("freedict")
        );

        let body = fs::read_to_string(out.join(RELEASE_BODY)).expect("the release page");
        for index in built {
            assert!(body.contains(&format!("## {index}.index.xz")), "{index}");
        }
        for index in failed {
            assert!(!body.contains(&format!("## {index}.index.xz")), "{index}");
        }
        assert!(body.contains("kaikki-en-Catalan-2026-11-02"), "{body}");

        assert!(!out.join(DOWNLOADS).exists(), "downloads left behind");
        let mut assets: Vec<String> = fs::read_dir(&out)
            .expect("the out directory")
            .map(|entry| {
                entry
                    .expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|name| name.ends_with(".index.xz"))
            .collect();
        assets.sort();
        assert_eq!(
            assets,
            ["kaikki-en-Catalan.index.xz", "kaikki-en-Spanish.index.xz"]
        );
    }

    /// When every source is built, nothing fails and the manifest lists
    /// them all, in row order.
    #[test]
    fn a_run_where_every_source_is_built_lists_them_all() {
        const CATALAN: &str = "/kaikki.org-dictionary-Catalan.jsonl.gz";
        const SPANISH: &str = "/kaikki.org-dictionary-Spanish.jsonl.gz";
        const MODIFIED: Option<&str> = Some("Mon, 02 Nov 2026 10:00:00 GMT");
        let base = serve(HashMap::from([
            (CATALAN, ("200 OK", kaikki("ca"), MODIFIED)),
            (SPANISH, ("200 OK", kaikki("es"), MODIFIED)),
        ]));
        let rows = [
            at(Source::KaikkiEnCatalan, format!("{base}{CATALAN}")),
            at(Source::KaikkiEnSpanish, format!("{base}{SPANISH}")),
        ];
        let dir = TempDir::new();
        let out = dir.0.join("indexes");

        let report = build(&out, &rows, &client().expect("the HTTP client")).expect("the run");

        assert!(report.failed.is_empty(), "{:?}", report.failed);
        let json = fs::read(out.join(MANIFEST)).expect("the manifest");
        let names: Vec<String> = Manifest::parse(&json)
            .expect("a valid manifest")
            .indexes
            .into_iter()
            .map(|index| index.name)
            .collect();
        assert_eq!(names, ["kaikki-en-Catalan", "kaikki-en-Spanish"]);
    }

    /// When every source fails, the manifest is still written, empty.
    #[test]
    fn a_run_where_every_source_fails_writes_an_empty_manifest() {
        let base = serve(HashMap::new());
        let rows = [
            at(Source::Oewn, format!("{base}/english-wordnet-2025.xml.gz")),
            at(Source::SinonimsCat, format!("{base}/th_ca_ES.dat")),
        ];
        let dir = TempDir::new();
        let out = dir.0.join("indexes");

        let report = build(&out, &rows, &client().expect("the HTTP client")).expect("the run");

        let failed: Vec<&str> = report.failed.iter().map(|(index, _)| *index).collect();
        assert_eq!(failed, ["oewn", "sinonims-cat"]);
        let json = fs::read(out.join(MANIFEST)).expect("the manifest");
        assert!(
            Manifest::parse(&json)
                .expect("a valid manifest")
                .indexes
                .is_empty()
        );
    }
}
