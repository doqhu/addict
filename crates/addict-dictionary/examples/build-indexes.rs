//! Builds the assets of the data release of the indexes, which the
//! application downloads on its first run: for each of the sources in
//! `addict_dictionary::sources` the upstream file is downloaded from the URL
//! of its row and imported, as `addict import` does, and the index is
//! compressed with `xz`. Then `manifest.json`, `WNDB_License.txt` and
//! `release-body.md` are written beside them. Nothing is uploaded: the
//! monthly release job (`.github/workflows/indexes.yml`) or the maintainer
//! does that, as `docs/index-release.md` says.
//!
//! ```text
//! cargo run -p addict-dictionary --example build-indexes -- [--out <DIR>] [--previous <FILE>]
//! cargo run -p addict-dictionary --example build-indexes -- --print-tag
//! ```
//!
//! `--out` defaults to `target/indexes`. A source that fails (its download,
//! its import or its compression) does not stop the run: its downloaded
//! file and any asset of an earlier run are removed and the next source is
//! built. The manifest and the release page list the indexes built, and are
//! written even when every source failed. The paths of the assets go to
//! stdout, and one line per failed source, naming its index, to stderr.
//!
//! `--previous` names the `manifest.json` of the published release. The
//! index of a failed source that it lists is kept: its entry goes into the
//! new manifest unchanged, at the position of its row, and its notice into
//! the release page, so uploading the new manifest does not hide the
//! published index from the application. Its asset is not in `--out`; the
//! one published stays. A previous manifest that cannot be read, or is of
//! another format, stops the run before anything is built.
//!
//! `--print-tag`, alone, prints the tag of the release of this index format,
//! `indexes-v<VERSION>`, and builds nothing.
//!
//! The exit status is 0 when every source was built (and for `--print-tag`),
//! 1 when any failed, kept or not, or the run could not start (the previous
//! manifest could not be read, the `--out` directory or the HTTP client could
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

/// The usage line printed with a usage error.
const USAGE: &str = "usage: build-indexes [--out <DIR>] [--previous <FILE>] | --print-tag";

fn main() -> ExitCode {
    ExitCode::from(run(
        std::env::args().skip(1),
        &mut io::stdout(),
        &mut io::stderr(),
    ))
}

/// Runs the builder with the arguments `args`, writing to `stdout` and
/// `stderr`; gives the exit status.
fn run(args: impl Iterator<Item = String>, stdout: &mut dyn Write, stderr: &mut dyn Write) -> u8 {
    let options = match Options::parse(args) {
        Ok(options) => options,
        Err(message) => {
            let _ = writeln!(stderr, "build-indexes: {message}");
            let _ = writeln!(stderr, "{USAGE}");
            return 2;
        }
    };
    if options.print_tag {
        let _ = writeln!(stdout, "{}", release::tag());
        return 0;
    }
    let out = &options.out;
    let report = options
        .previous
        .as_deref()
        .map(read_previous)
        .transpose()
        .and_then(|previous| {
            let http = client()?;
            build(out, sources::ROWS, &http, previous.as_ref())
        });
    match report {
        Ok(report) => print_report(&report, out, stdout, stderr),
        Err(message) => {
            let _ = writeln!(stderr, "build-indexes: {message}");
            1
        }
    }
}

/// Prints the assets of `report` in `out` to `stdout` and its failed and
/// kept indexes to `stderr`; gives the exit status. The release job parses
/// these lines for its summary.
fn print_report(report: &Report, out: &Path, stdout: &mut dyn Write, stderr: &mut dyn Write) -> u8 {
    // Only what is in `out`: a kept index has no asset there.
    for published in &report.manifest.indexes {
        if !report.kept.contains(&published.name.as_str()) {
            let _ = writeln!(stdout, "{}", out.join(published.asset()).display());
        }
    }
    for name in [MANIFEST, WNDB_LICENSE, RELEASE_BODY] {
        let _ = writeln!(stdout, "{}", out.join(name).display());
    }
    if report.failed.is_empty() {
        return 0;
    }
    for (index, error) in &report.failed {
        let _ = writeln!(stderr, "build-indexes: {index}: {error}");
    }
    for index in &report.kept {
        let _ = writeln!(
            stderr,
            "build-indexes: {index}: keeping the published index"
        );
    }
    let names: Vec<&str> = report.failed.iter().map(|(index, _)| *index).collect();
    let _ = writeln!(
        stderr,
        "build-indexes: {} of {} sources failed: {}",
        names.len(),
        sources::ROWS.len(),
        names.join(", ")
    );
    1
}

/// What a run built and what failed.
#[derive(Debug)]
struct Report {
    /// The manifest written: of the indexes built and the ones kept.
    manifest: Manifest,
    /// Each source that failed: its index and the error.
    failed: Vec<(&'static str, String)>,
    /// The index of each failed source whose entry of the previous manifest
    /// was kept, in row order.
    kept: Vec<&'static str>,
}

/// The command line.
#[derive(Debug, PartialEq, Eq)]
struct Options {
    /// Where the assets go.
    out: PathBuf,
    /// The manifest of the published release, whose entries of failed
    /// sources are kept.
    previous: Option<PathBuf>,
    /// Print the tag of the release and build nothing.
    print_tag: bool,
}

impl Options {
    /// The options of `args`: `--out <DIR>`, `--previous <FILE>`, or
    /// `--print-tag` alone.
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut options = Self {
            out: PathBuf::from(DEFAULT_OUT),
            previous: None,
            print_tag: false,
        };
        let mut count = 0;
        while let Some(arg) = args.next() {
            count += 1;
            match arg.as_str() {
                "--out" => options.out = args.next().ok_or("--out needs a directory")?.into(),
                "--previous" => {
                    options.previous = Some(args.next().ok_or("--previous needs a file")?.into());
                }
                "--print-tag" => options.print_tag = true,
                _ => return Err(format!("unknown argument {arg}")),
            }
        }
        if options.print_tag && count > 1 {
            return Err("--print-tag takes no other argument".to_owned());
        }
        Ok(options)
    }
}

/// The manifest of the published release in the file `path`, which must be
/// of this index format.
fn read_previous(path: &Path) -> Result<Manifest, String> {
    let json = fs::read(path).map_err(|error| {
        format!(
            "cannot read the previous manifest {}: {error}",
            path.display()
        )
    })?;
    Manifest::parse(&json).map_err(|error| {
        format!(
            "cannot use the previous manifest {}: {error}",
            path.display()
        )
    })
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
/// the release page of the indexes built. The entry of a failed source that
/// `previous` lists is kept in the manifest, at the position of its row,
/// and its notice in the release page. The error is one that stops the
/// whole run: `out` cannot be made or an asset beside the indexes cannot be
/// written.
fn build(
    out: &Path,
    rows: &[Row],
    http: &reqwest::blocking::Client,
    previous: Option<&Manifest>,
) -> Result<Report, String> {
    fs::create_dir_all(out).map_err(|error| format!("cannot make {}: {error}", out.display()))?;
    let mut manifest = Manifest {
        format: format::VERSION,
        indexes: Vec::new(),
    };
    let mut listed: Vec<(&Row, Upstream)> = Vec::new();
    let mut failed = Vec::new();
    let mut kept = Vec::new();
    for row in rows {
        match build_one(http, row, out) {
            Ok((published, edition)) => {
                manifest.indexes.push(published);
                listed.push((row, Upstream::Built(edition)));
            }
            Err(error) => {
                failed.push((row.index, error));
                let published = previous.and_then(|previous| {
                    previous
                        .indexes
                        .iter()
                        .find(|published| published.name == row.index)
                });
                if let Some(published) = published {
                    manifest.indexes.push(published.clone());
                    listed.push((row, Upstream::Kept));
                    kept.push(row.index);
                }
            }
        }
    }
    let _ = fs::remove_dir(out.join(DOWNLOADS));
    write(
        &out.join(WNDB_LICENSE),
        include_bytes!("WNDB_License.txt").as_slice(),
    )?;
    write(&out.join(RELEASE_BODY), release_body(&listed).as_bytes())?;
    // The manifest last: the release job uploads only when it exists, so it
    // must mean that the run finished and every other asset is written.
    write(&out.join(MANIFEST), manifest.to_json().as_bytes())?;
    Ok(Report {
        manifest,
        failed,
        kept,
    })
}

/// Where a listed index comes from, for its upstream line in the release
/// page.
#[derive(Debug)]
enum Upstream {
    /// Built in this run from the upstream file of this edition.
    Built(String),
    /// Kept from the previous manifest: the index of an earlier run.
    Kept,
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
/// its index was built from, or with the mark of an index kept from an
/// earlier run, whose edition is only in the notice inside its file.
fn release_body(rows: &[(&Row, Upstream)]) -> String {
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
    for (row, upstream) in rows {
        body.push_str(&format!("\n## {}\n\n", release::asset_name(row.index)));
        body.push_str(&format!("- Credit: {}\n", row.credit));
        body.push_str(&format!("- Author: {}\n", row.author));
        body.push_str(&format!("- License: {}\n", row.license));
        body.push_str(&format!("- License text: {}\n", row.license_uri));
        body.push_str(&format!("- Source: {}\n", row.link));
        let url = row.url.unwrap_or(row.upstream);
        match upstream {
            Upstream::Built(edition) => {
                body.push_str(&format!("- Upstream: {url} ({edition})\n"));
            }
            Upstream::Kept => body.push_str(&format!(
                "- Upstream: {url} (the index of an earlier run; its edition is in the notice \
                 inside the file)\n"
            )),
        }
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

        let report = build(&out, &rows, &http, None).expect("the run");

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

        let report =
            build(&out, &rows, &client().expect("the HTTP client"), None).expect("the run");

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

        let report =
            build(&out, &rows, &client().expect("the HTTP client"), None).expect("the run");

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

    /// A published entry of the index `name`, as a previous manifest lists
    /// it.
    fn published(name: &str) -> Published {
        Published {
            name: name.to_owned(),
            format: format::VERSION,
            size: 1234,
            sha256: "ab".repeat(32),
        }
    }

    /// The names of the indexes of `manifest`, in its order.
    fn names(manifest: &Manifest) -> Vec<&str> {
        manifest
            .indexes
            .iter()
            .map(|index| index.name.as_str())
            .collect()
    }

    /// A failed source that the previous manifest lists keeps its entry,
    /// unchanged and at the position of its row, and its notice in the
    /// release page, marked as an earlier run's; an index of the previous
    /// manifest whose row did not run is not listed.
    #[test]
    fn a_failed_source_of_the_previous_manifest_is_kept() {
        const CATALAN: &str = "/kaikki.org-dictionary-Catalan.jsonl.gz";
        const MODIFIED: Option<&str> = Some("Mon, 02 Nov 2026 10:00:00 GMT");
        let base = serve(HashMap::from([(
            CATALAN,
            ("200 OK", kaikki("ca"), MODIFIED),
        )]));
        let rows = [
            at(
                Source::Freedict,
                format!("{base}/freedict-eng-rus-2025.11.23.src.tar.xz"),
            ),
            at(Source::KaikkiEnCatalan, format!("{base}{CATALAN}")),
        ];
        let previous = Manifest {
            format: format::VERSION,
            indexes: vec![published("oewn"), published("freedict")],
        };
        let dir = TempDir::new();
        let out = dir.0.join("indexes");

        let report = build(
            &out,
            &rows,
            &client().expect("the HTTP client"),
            Some(&previous),
        )
        .expect("the run");

        let failed: Vec<&str> = report.failed.iter().map(|(index, _)| *index).collect();
        assert_eq!(failed, ["freedict"]);
        assert_eq!(report.kept, ["freedict"]);
        let json = fs::read(out.join(MANIFEST)).expect("the manifest");
        let manifest = Manifest::parse(&json).expect("a valid manifest");
        assert_eq!(manifest, report.manifest);
        assert_eq!(names(&manifest), ["freedict", "kaikki-en-Catalan"]);
        assert_eq!(manifest.indexes[0], published("freedict"));
        let catalan = fs::read(out.join("kaikki-en-Catalan.index.xz")).expect("the asset");
        assert_eq!(
            manifest.indexes[1],
            Published::of("kaikki-en-Catalan", &catalan)
        );
        assert!(
            !out.join("freedict.index.xz").exists(),
            "a kept asset in out"
        );

        let body = fs::read_to_string(out.join(RELEASE_BODY)).expect("the release page");
        let freedict = body
            .split("\n## ")
            .find(|section| section.starts_with("freedict.index.xz"))
            .expect("the notice of the kept index");
        let row = sources::row(Source::Freedict);
        for line in [
            row.credit,
            row.author,
            row.license,
            row.license_uri,
            row.link,
        ] {
            assert!(freedict.contains(line), "{line} in {freedict}");
        }
        assert!(freedict.contains(sources::MODIFIED), "{freedict}");
        assert!(
            freedict.contains(
                "(the index of an earlier run; its edition is in the notice inside the file)"
            ),
            "{freedict}"
        );
        assert!(body.contains("## kaikki-en-Catalan.index.xz"), "{body}");
        assert!(body.contains("kaikki-en-Catalan-2026-11-02"), "{body}");
        assert!(!body.contains("## oewn.index.xz"), "{body}");
        assert!(body.contains(sources::DISCLAIMER), "{body}");

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = print_report(&report, &out, &mut stdout, &mut stderr);
        assert_eq!(code, 1);
        let stdout = String::from_utf8(stdout).expect("UTF-8");
        let printed: Vec<&str> = stdout.lines().collect();
        let expected: Vec<String> = [
            "kaikki-en-Catalan.index.xz",
            MANIFEST,
            WNDB_LICENSE,
            RELEASE_BODY,
        ]
        .iter()
        .map(|name| out.join(name).display().to_string())
        .collect();
        assert_eq!(printed, expected);
        let stderr = String::from_utf8(stderr).expect("UTF-8");
        let lines: Vec<&str> = stderr.lines().collect();
        assert_eq!(lines.len(), 3, "{stderr}");
        assert!(
            lines[0].starts_with("build-indexes: freedict: "),
            "{stderr}"
        );
        assert_ne!(
            lines[0], "build-indexes: freedict: keeping the published index",
            "{stderr}"
        );
        assert_eq!(
            lines[1],
            "build-indexes: freedict: keeping the published index"
        );
        assert_eq!(
            lines[2],
            format!(
                "build-indexes: 1 of {} sources failed: freedict",
                sources::ROWS.len()
            )
        );
    }

    /// A failed source that the previous manifest does not list is neither
    /// in the manifest nor in the release page.
    #[test]
    fn a_failed_source_missing_from_the_previous_manifest_is_dropped() {
        const CATALAN: &str = "/kaikki.org-dictionary-Catalan.jsonl.gz";
        let base = serve(HashMap::from([(
            CATALAN,
            (
                "200 OK",
                kaikki("ca"),
                Some("Mon, 02 Nov 2026 10:00:00 GMT"),
            ),
        )]));
        let rows = [
            at(
                Source::Freedict,
                format!("{base}/freedict-eng-rus-2025.11.23.src.tar.xz"),
            ),
            at(Source::KaikkiEnCatalan, format!("{base}{CATALAN}")),
        ];
        let previous = Manifest {
            format: format::VERSION,
            indexes: vec![published("oewn")],
        };
        let dir = TempDir::new();
        let out = dir.0.join("indexes");

        let report = build(
            &out,
            &rows,
            &client().expect("the HTTP client"),
            Some(&previous),
        )
        .expect("the run");

        let failed: Vec<&str> = report.failed.iter().map(|(index, _)| *index).collect();
        assert_eq!(failed, ["freedict"]);
        assert!(report.kept.is_empty(), "{:?}", report.kept);
        let json = fs::read(out.join(MANIFEST)).expect("the manifest");
        let manifest = Manifest::parse(&json).expect("a valid manifest");
        assert_eq!(names(&manifest), ["kaikki-en-Catalan"]);
        let body = fs::read_to_string(out.join(RELEASE_BODY)).expect("the release page");
        assert!(!body.contains("## freedict.index.xz"), "{body}");
        assert!(!body.contains("## oewn.index.xz"), "{body}");
    }

    /// Runs the builder with `args`; gives its exit status, stdout and
    /// stderr.
    fn run_with(args: &[&str]) -> (u8, String, String) {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run(
            args.iter().map(|arg| (*arg).to_owned()),
            &mut stdout,
            &mut stderr,
        );
        (
            code,
            String::from_utf8(stdout).expect("UTF-8"),
            String::from_utf8(stderr).expect("UTF-8"),
        )
    }

    /// A previous manifest that is missing, garbled or of another format
    /// stops the run before anything is built, naming the file.
    #[test]
    fn a_bad_previous_manifest_stops_the_run() {
        let dir = TempDir::new();
        let garbled = dir.0.join("garbled.json");
        fs::write(&garbled, b"{ not json").expect("the garbled manifest");
        let older = dir.0.join("older.json");
        fs::write(&older, br#"{"format":2,"indexes":[]}"#).expect("the older manifest");
        let missing = dir.0.join("missing.json");
        for previous in [&missing, &garbled, &older] {
            let out = dir.0.join("indexes");
            let previous = previous.to_str().expect("a UTF-8 path");

            let (code, stdout, stderr) = run_with(&[
                "--out",
                out.to_str().expect("a UTF-8 path"),
                "--previous",
                previous,
            ]);

            assert_eq!(code, 1, "{previous}: {stderr}");
            assert!(stderr.contains(previous), "{stderr}");
            assert!(stdout.is_empty(), "{stdout}");
            assert!(!out.exists(), "{previous}: something was written");
        }
    }

    /// `--print-tag` alone prints the tag of this format and builds nothing;
    /// with any other argument it is a usage error.
    #[test]
    fn print_tag_prints_the_tag_alone() {
        let (code, stdout, stderr) = run_with(&["--print-tag"]);
        assert_eq!((code, stdout.as_str()), (0, "indexes-v3\n"), "{stderr}");
        assert_eq!(stdout, format!("{}\n", release::tag()));

        for args in [
            &["--print-tag", "--out", "x"][..],
            &["--out", "x", "--print-tag"],
            &["--print-tag", "--previous", "manifest.json"],
            &["--print-tag", "--print-tag"],
        ] {
            let (code, stdout, stderr) = run_with(args);
            assert_eq!(code, 2, "{args:?}");
            assert!(stdout.is_empty(), "{stdout}");
            assert!(stderr.contains("usage:"), "{stderr}");
        }
    }
}
