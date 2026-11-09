//! Builds the assets of the data release of the indexes, which the
//! application downloads on its first run: for each source with a download
//! URL in `addict_dictionary::sources` (only OEWN for now) the upstream file
//! is downloaded and imported, as `addict import` does, and the index is
//! compressed with `xz`. Then `manifest.json`, `WNDB_License.txt` and
//! `release-body.md` are written beside them. Nothing is uploaded: the
//! maintainer does that, as `docs/index-release.md` says.
//!
//! ```text
//! cargo run -p addict-dictionary --example build-indexes -- --out <DIR>
//! ```
//!
//! `--out` defaults to `target/indexes`. Any error ends the run with a
//! non-zero status and a line naming the source.

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    time::Duration,
};

use addict_dictionary::{
    format,
    import::{self, KNOWN},
    release::{self, MANIFEST, Manifest, Published},
    sources::{self, Row},
};

/// Where the assets go without `--out`.
const DEFAULT_OUT: &str = "target/indexes";

/// The Princeton WordNet license, which ships beside the OEWN index.
const WNDB_LICENSE: &str = "WNDB_License.txt";

/// The text of the release page.
const RELEASE_BODY: &str = "release-body.md";

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
    match build(&out) {
        Ok(manifest) => {
            for published in &manifest.indexes {
                println!("{}", out.join(published.asset()).display());
            }
            for name in [MANIFEST, WNDB_LICENSE, RELEASE_BODY] {
                println!("{}", out.join(name).display());
            }
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("build-indexes: {message}");
            ExitCode::FAILURE
        }
    }
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

/// Builds every asset into `out`; the error names the source.
fn build(out: &Path) -> Result<Manifest, String> {
    fs::create_dir_all(out).map_err(|error| format!("cannot make {}: {error}", out.display()))?;
    let http = reqwest::blocking::Client::builder()
        .user_agent(concat!("addict-build-indexes/", env!("CARGO_PKG_VERSION")))
        .timeout(DOWNLOAD_LIMIT)
        .build()
        .map_err(|error| format!("cannot start the HTTP client: {error}"))?;
    let mut manifest = Manifest {
        format: format::VERSION,
        indexes: Vec::new(),
    };
    let built: Vec<&Row> = sources::ROWS
        .iter()
        .filter(|row| row.url.is_some())
        .collect();
    for row in &built {
        let published =
            build_one(&http, row, out).map_err(|error| format!("{}: {error}", row.index))?;
        manifest.indexes.push(published);
    }
    write(&out.join(MANIFEST), manifest.to_json().as_bytes())?;
    write(
        &out.join(WNDB_LICENSE),
        include_bytes!("WNDB_License.txt").as_slice(),
    )?;
    write(&out.join(RELEASE_BODY), release_body(&built).as_bytes())?;
    Ok(manifest)
}

/// Downloads, imports and compresses the index of `row` into `out`.
fn build_one(http: &reqwest::blocking::Client, row: &Row, out: &Path) -> Result<Published, String> {
    let url = row.url.ok_or("no download URL")?;
    let name = url.rsplit('/').next().unwrap_or(url);
    let downloads = out.join(".downloads");
    fs::create_dir_all(&downloads)
        .map_err(|error| format!("cannot make {}: {error}", downloads.display()))?;
    let file = downloads.join(name);
    download(http, url, &file)?;
    let imported = import::import(&file, out).map_err(|error| error.to_string())?;
    let expected = out.join(format!("{}.index", row.index));
    if imported.path != expected {
        return Err(format!(
            "{url} was imported as {}, not as {}",
            imported.path.display(),
            expected.display()
        ));
    }
    let _ = fs::remove_file(&file);
    let _ = fs::remove_dir(&downloads);
    xz(&imported.path)?;
    let asset = out.join(release::asset_name(row.index));
    let bytes =
        fs::read(&asset).map_err(|error| format!("cannot read {}: {error}", asset.display()))?;
    Ok(Published::of(row.index, &bytes))
}

/// Downloads `url` into `file`, through a temporary file beside it.
fn download(http: &reqwest::blocking::Client, url: &str, file: &Path) -> Result<(), String> {
    let mut response = http
        .get(url)
        .send()
        .map_err(|error| format!("cannot download {url}: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("cannot download {url}: HTTP {status}"));
    }
    let partial = file.with_extension("part");
    let written = fs::File::create(&partial).and_then(|mut target| {
        io::copy(&mut response, &mut target)?;
        target.flush()?;
        target.sync_all()
    });
    match written.and_then(|()| fs::rename(&partial, file)) {
        Ok(()) => Ok(()),
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

/// The address of the license text of the index `name`, for the release
/// page. Every source the builder builds has one.
fn license_uri(name: &str) -> Option<&'static str> {
    match name {
        "oewn" => Some("https://creativecommons.org/licenses/by/4.0/"),
        _ => None,
    }
}

/// The text of the release page: per index its credit, its license, its
/// upstream file and that it was modified, then the warranty disclaimer.
fn release_body(rows: &[&Row]) -> String {
    let mut body = String::new();
    body.push_str(&format!(
        "Dictionary indexes of [addict](https://github.com/doqhu/addict) in index format {}.\n\n",
        format::VERSION
    ));
    body.push_str(
        "The application downloads these files on its first run. Each `.index.xz` is an xz-compressed \
         index; `manifest.json` gives the size and the SHA-256 of each.\n",
    );
    for row in rows {
        let edition = KNOWN
            .iter()
            .find(|known| sources::row(known.source).index == row.index)
            .map_or("", |known| known.edition);
        body.push_str(&format!("\n## {}\n\n", release::asset_name(row.index)));
        body.push_str(&format!("- Credit: {}\n", row.credit));
        body.push_str(&format!("- License: {}\n", row.license));
        if let Some(uri) = license_uri(row.index) {
            body.push_str(&format!("- License text: {uri}\n"));
        }
        body.push_str(&format!(
            "- Upstream: {} ({})\n",
            row.url.unwrap_or(row.upstream),
            edition
        ));
        body.push_str(
            "- Modified: converted from the upstream file to the addict index format, \
             which keeps only what addict shows.\n",
        );
        if row.index == "oewn" {
            body.push_str(&format!(
                "- The Princeton WordNet license ships beside the index as `{WNDB_LICENSE}`.\n"
            ));
        }
    }
    body.push_str(
        "\n## No warranty\n\n\
         The indexes are provided \"as is\", without warranty of any kind, express or implied, \
         including but not limited to the warranties of merchantability, fitness for a particular \
         purpose and noninfringement. The licensors of the upstream data and the authors of addict \
         are not liable for any claim, damages or other liability arising from the use of the \
         indexes.\n",
    );
    body
}
