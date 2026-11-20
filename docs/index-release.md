# Publishing the dictionary indexes

Settings → Dictionaries → Download dictionaries fetches a fresh manifest and
checks every published index through the index reader, including lexical
records. Only missing or unreadable files are offered, in manifest order;
healthy files and manually imported editions are kept. The check is repeated
under the shared installation permit before recovery downloads an asset, so a
manual edition imported after the offer also stays. This explicit action works
after Not now and does not change the remembered first-run choice. Empty or
failed checks are shown in Settings and the same action retries them. Manual
file import remains available beside it.

On its first run, with no index installed, addict offers to download every
dictionary the manifest of the data release lists. The app downloads prebuilt
indexes from a GitHub data release of the separate data repository
[`doqhu/addict-indexes`](https://github.com/doqhu/addict-indexes), one after
another. Each index has its own row in the dialog, with its progress and a
Cancel button; an index that fails (a missing asset, a network error, a wrong
size or checksum, an unknown name or format, a file the app cannot open) says
why on its row, and the other indexes still install. A network failure is
shown in plain words, such as `cannot download oewn: cannot reach the server`,
`no answer in time` (when the server does not answer in time, or an answer
goes 60 seconds without a byte) or `the connection was interrupted`, and the
full error of the HTTP library is written to standard error as
`addict: cannot download <index or the index manifest>: <error>`. A manifest
that lists no index offers nothing. This page is the contract between that
release and the app, and the steps the maintainer follows to publish it. The
schema lives in `crates/addict-dictionary/src/release.rs`, which both the
builder and the app read.

## The release

- **Tag:** `indexes-v<VERSION>`, where `<VERSION>` is
  `addict_dictionary::format::VERSION` (now `3`, so the tag is `indexes-v3`).
  The data release lives in `doqhu/addict-indexes`, apart from the app's
  `vX.Y.Z` releases in `doqhu/addict`. An
  `indexes-v2` release existed only as a test before any app read it, and
  was deleted; no released app downloads indexes of format 2.
- **URLs:** the app fetches only
  `https://github.com/doqhu/addict-indexes/releases/download/indexes-v<VERSION>/<asset>`
  (`BASE_URL` in `crates/addict-engine/src/download.rs`).
  It never calls the GitHub REST API and never uses `releases/latest`.
- **Why a separate repository:** `doqhu/addict` has immutable releases on, so
  that a published app release can never change. A data release must be
  refreshed every month, so it lives in `doqhu/addict-indexes`, where
  immutable releases are off.
- **The frozen release of `doqhu/addict`:** apps 0.2.x and 0.4.x read the
  `indexes-v3` release of `doqhu/addict` itself. That `indexes-v3` release,
  with `manifest.json`, `manifest-2.json` and their indexes, stays as it is:
  it is immutable, is never refreshed, and is never edited or deleted. The
  app releases of `doqhu/addict` stay immutable too.
- **Assets:**
  - `<name>.index.xz`: the index `<name>.index`, compressed with xz. The
    name is the index name of `addict_dictionary::sources`, such as `oewn`.
  - `manifest-3.json`: asset sizes and SHA-256 checksums for the current
    inventory of 20 dictionary indexes, `kaikki-ru-Russkiy` among them.
  - `manifest-2.json`: the same fields for the 19 indexes of 0.4.1, read by
    apps through 0.4.x. Frozen: it never gains an index.
  - `manifest.json`: the same fields for the 13 indexes supported by apps
    through 0.2.x. Frozen as well.
  - `WNDB_License.txt`: the Princeton WordNet license, which ships with any
    copy of the OEWN index.
  - The release notes (`release-body.md`) hold each index's credit, author,
    license, license URI, source link, upstream file, a "modified" notice and
    a warranty disclaimer: the license notice the index carries in its own
    file (see below).
- **Older formats:** a release that any released app reads is never changed
  or deleted. Apps that read only that format keep downloading from it. The
  one exception is a release that no released app ever read, such as the
  `indexes-v2` test release, which may be deleted. A new index format gets a
  new tag.
- **Order:** upload the indexes and license before the manifests. Each
  manifest must point to assets already on the release.

## The manifests

First run, Settings recovery and checksum refresh all try `manifest-3.json`
first, fall back to `manifest-2.json` on HTTP 404, and to `manifest.json` when
that is 404 too. Only a 404 moves to the next older manifest: a malformed or
incompatible manifest, a network failure, or any other HTTP error follows the
existing download error path.

All three manifests use this schema:

```json
{
  "format": 3,
  "indexes": [
    {
      "name": "oewn",
      "format": 3,
      "size": 5935000,
      "sha256": "430dc0432ffdf44e4803c44598e9bb8bec7b0c14dcbb09d3c29b95e0307fc4f3"
    }
  ]
}
```

| Field | Meaning |
|---|---|
| `format` | The index format of the release. It must equal the `<VERSION>` in the tag. |
| `indexes[].name` | The index name, without `.index`. The asset is `<name>.index.xz`. |
| `indexes[].format` | The format of that index. |
| `indexes[].size` | The size of the `.index.xz` asset, in bytes. |
| `indexes[].sha256` | The SHA-256 of the `.index.xz` asset, in lowercase hex. |

The app refuses a manifest of another format, and an index whose name it does
not know or whose format it cannot read. It checks the size and the SHA-256 of
an asset before it decompresses it. It then writes the index to a temporary
file in the data directory, opens it as an index, and only then renames it
over the old one. Cancellation or timeout before commit prevents the rename.
If the file has already been committed, its resident availability refresh
finishes independently of queue cancellation. A temporary file that another,
killed, run of the app left for that index is removed before the install.

When the SHA-256 of an asset does not match, the app fetches the manifest once
more, because an asset can be newer than the manifest read before it (see
**Order** above). If the fresh manifest gives that index another entry, the app
downloads the index again by it; otherwise, or when the manifest cannot be
fetched, the checksum error stands. This happens at most once per index.

## The license notice in each index

Since format 3, every index the importer writes carries a license notice, so
the CC BY / CC BY-SA duties travel with the file itself and not only with the
release page. It is the last section of the file; the app reads it when it
opens the index and refuses the index when the notice is damaged. Its fields,
in their stored order:

| Field | Value |
|---|---|
| `credit` | The credit line of the source, as in `CREDITS`. |
| `author` | The authors of the data. |
| `license` | The license, as in `CREDITS`. |
| `license_uri` | The address of the license text. |
| `link` | The address of the source. |
| `modified` | That the index was converted from the upstream file (`sources::MODIFIED`). |
| `disclaimer` | The warranty disclaimer (`sources::DISCLAIMER`). |
| `edition` | The edition of the upstream file, also the source id of the index. For a pinned file it is the edition in `import::KNOWN`; for a kaikki file accepted by its content it is `<index>-<date>`, the date being the day of the download's `Last-Modified` header (the file's modification time on a manual import), for example `kaikki-en-Spanish-2026-11-02`. |
| `sha256` | The SHA-256 of the upstream file the index was built from, in lowercase hex. |

The first five come from the source's row in
`crates/addict-dictionary/src/sources.rs`, the same table `CREDITS` is made
from, and the builder writes the same texts into `release-body.md`. Indexes of
format 1 or 2 have no notice; the app still opens them.

## Building and uploading

Build the assets. This needs `xz` on `PATH` and network access, and takes
minutes: it downloads all 20 upstream files, each once, from the URLs in
`crates/addict-dictionary/src/sources.rs`.

```bash
mkdir -p target
cargo run --release -p addict-dictionary --example build-indexes -- \
  --out target/indexes > target/indexes-assets.txt
```

`target/indexes` then holds one `<name>.index.xz` per rebuilt index,
`manifest.json`, `manifest-2.json`, `manifest-3.json`, `WNDB_License.txt` and
`release-body.md`.
The builder reports their paths on stdout. The uncompressed `.index` files
stay local.

A source that fails (its upstream file is gone, the importer refuses it, `xz`
fails) does not stop the run. The builder goes on with the next source and
exits with status 1. It prints one `build-indexes: <index>: <error>` line per
failed source to stderr, then a line naming all of them. A partial build for
an existing release needs the published manifests to retain failed sources.
Use the refresh commands below with the previous manifest files, or fix the
failed sources and rebuild before uploading.

After a successful build for a new format, create the release with the
indexes, then upload the manifests. `--latest=false` keeps the indexes
release from being marked as the latest one; the app never uses
`releases/latest` anyway:

```bash
if grep -Fxq target/indexes/manifest.json target/indexes-assets.txt &&
   grep -Fxq target/indexes/manifest-2.json target/indexes-assets.txt &&
   grep -Fxq target/indexes/manifest-3.json target/indexes-assets.txt; then
  mapfile -t rebuilt < <(grep '\.index\.xz$' target/indexes-assets.txt)
  (( ${#rebuilt[@]} > 0 )) &&
  xz -t "${rebuilt[@]}" &&
  gh release create indexes-v3 --repo doqhu/addict-indexes --target main --latest=false \
    --title "Dictionary indexes, format 3" \
    --notes-file target/indexes/release-body.md \
    "${rebuilt[@]}" target/indexes/WNDB_License.txt &&
  gh release upload indexes-v3 --repo doqhu/addict-indexes target/indexes/manifest.json &&
  gh release upload indexes-v3 --repo doqhu/addict-indexes target/indexes/manifest-2.json &&
  gh release upload indexes-v3 --repo doqhu/addict-indexes target/indexes/manifest-3.json
fi
```

To refresh an existing release, download the available manifests into a
fresh directory and pass each to the matching builder option. Omit the
download and the option of a manifest the release does not have yet, such as
`manifest-3.json` when publishing it for the first time.

```bash
previous=$(mktemp -d)
gh release download indexes-v3 --repo doqhu/addict-indexes \
  --pattern manifest.json --pattern manifest-2.json --pattern manifest-3.json \
  --dir "$previous"
mkdir -p target
cargo run --release -p addict-dictionary --example build-indexes -- \
  --out target/indexes \
  --previous "$previous/manifest.json" --previous-2 "$previous/manifest-2.json" \
  --previous-3 "$previous/manifest-3.json" > target/indexes-assets.txt
```

Once the builder reports the three manifests, upload the rebuilt indexes and
license, then the manifests one at a time, oldest first and `manifest-3.json`
last, then the notes. During the upload, an app that
fetches a rebuilt index using an old manifest may report a checksum mismatch.

```bash
if grep -Fxq target/indexes/manifest.json target/indexes-assets.txt &&
   grep -Fxq target/indexes/manifest-2.json target/indexes-assets.txt &&
   grep -Fxq target/indexes/manifest-3.json target/indexes-assets.txt; then
  mapfile -t rebuilt < <(grep '\.index\.xz$' target/indexes-assets.txt)
  gh release upload indexes-v3 --repo doqhu/addict-indexes --clobber \
    "${rebuilt[@]}" target/indexes/WNDB_License.txt &&
  gh release upload indexes-v3 --repo doqhu/addict-indexes --clobber target/indexes/manifest.json &&
  gh release upload indexes-v3 --repo doqhu/addict-indexes --clobber target/indexes/manifest-2.json &&
  gh release upload indexes-v3 --repo doqhu/addict-indexes --clobber target/indexes/manifest-3.json &&
  gh release edit indexes-v3 --repo doqhu/addict-indexes --notes-file target/indexes/release-body.md
fi
```

After the upload, check by hand that the first-run download works from a
clean profile: with no index and no choice file, start addict and press
Download. Every listed index must appear in the data directory, for example
`oewn.index`, each row must end as Installed, and the window's index line must
update. The choice file and the data directory are:

- native build: `~/.config/addict/first-run-download` and
  `~/.local/share/addict`;
- Flatpak: `~/.var/app/io.github.doqhu.Addict/config/addict/first-run-download`
  and `~/.var/app/io.github.doqhu.Addict/data/addict`.

## The monthly release job

The workflow lives only in `doqhu/addict-indexes`, as
`.github/workflows/indexes.yml`; `doqhu/addict` has no indexes workflow. It
builds and uploads the indexes once a month, on the 2nd at 03:17 UTC, from
`main` of `doqhu/addict`. To start it by hand, open Actions → Dictionary
indexes → Run workflow in `doqhu/addict-indexes`, or run:

```sh
gh workflow run indexes.yml --repo doqhu/addict-indexes
```

The input `ref` (default `main`) is the branch, tag or commit of
`doqhu/addict` the builder is built from, for example
`gh workflow run indexes.yml --repo doqhu/addict-indexes -f ref=feat/russian-article-tracer`.
The monthly run always builds `main`.

The job:

1. Checks out `doqhu/addict` at `ref` (a public repository, read with no
   secret) and builds the builder from it.
2. Asks the builder for the tag (`build-indexes --print-tag`), so it writes
   only the release of the current index format. A release of an older
   format is never read or written.
3. Downloads `manifest.json`, `manifest-2.json` and `manifest-3.json` when
   present. Failure to download a listed manifest stops publication; a
   release without `manifest-3.json` builds without `--previous-3`.
4. Passes available manifests to the builder through `--previous`,
   `--previous-2` and `--previous-3`, respectively.
5. Requires the current build to report the three manifest paths. Creates
   the release if needed with `--latest=false` and `--target main` (the
   `main` of `doqhu/addict-indexes`, which holds only the workflow and a
   README), uploads the rebuilt indexes and `WNDB_License.txt`, uploads the
   manifests one at a time, oldest first and `manifest-3.json` last, then
   replaces the release notes with `release-body.md`. It writes with the
   job's own `GITHUB_TOKEN`; there is no long-lived token.
6. Lists the indexes built, kept and failed, with each failure's error, in
   the run's summary.
7. Re-enables its own workflow, whatever happened before
   (`gh api -X PUT repos/doqhu/addict-indexes/actions/workflows/indexes.yml/enable`,
   with `actions: write`). GitHub disables a scheduled workflow of a public
   repository after 60 days without repository activity; this step is meant
   to keep the monthly run going, but it is not proven to reset that timer.
   If the workflow shows as disabled in Actions, enable it there by hand.

**Kept indexes.** When a source fails, each manifest can keep an eligible
entry from its own published copy at the source's position in
`sources::ROWS`. The published asset stays in place, and the release notes
credit it as an index from an earlier run. When publishing a manifest for
the first time, such as `manifest-3.json`, it lists that run's successful
builds of its inventory.

**A red run.** The job ends with the builder's status: red when any source
failed, kept or not, after the other indexes were published. Read the
summary, fix the source (a new pin in `import::KNOWN` and a new URL in
`sources.rs`, for example), and start the job again.

## Setting up the data repository

The maintainer does this once; the app and this repository do not depend on
anything else:

1. Create the public repository `doqhu/addict-indexes` with a `main` branch.
2. In its Settings → General → Releases, leave immutable releases off.
3. Optionally, in its Settings → Actions → General → Workflow permissions,
   choose "Read and write permissions". This is a precaution, not a
   requirement: the job itself requests `contents: write` and
   `actions: write`.
4. Push the workflow `.github/workflows/indexes.yml` and a short `README.md`
   to its `main`.
5. Dispatch the first run against the branch that carries manifest-3, for
   example `-f ref=feat/russian-article-tracer`, while that branch is not yet
   merged; once it is, the default `main` is right. The first run creates
   `indexes-v3` there with the 20 indexes, `WNDB_License.txt` and the three
   manifests. A second run must refresh it with `--clobber` and no error.
6. Run the ignored live test:
   `cargo test -p addict-engine download::tests::live -- --ignored`.
