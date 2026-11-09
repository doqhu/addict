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
indexes from a GitHub data release of this repository, one after another. Each
index has its own row in the dialog, with its progress and a Cancel button; an
index that fails (a missing asset, a network error, a wrong size or checksum,
an unknown name or format, a file the app cannot open) says why on its row,
and the other indexes still install. A manifest that lists no index offers
nothing. This page is the contract
between that release and the app, and the steps the maintainer follows to
publish it. The schema lives in `crates/addict-dictionary/src/release.rs`,
which both the builder and the app read.

## The release

- **Tag:** `indexes-v<VERSION>`, where `<VERSION>` is
  `addict_dictionary::format::VERSION` (now `3`, so the tag is `indexes-v3`).
  The data release is separate from the app's `vX.Y.Z` releases. An
  `indexes-v2` release existed only as a test before any app read it, and
  was deleted; no released app downloads indexes of format 2.
- **URLs:** the app fetches only
  `https://github.com/doqhu/addict/releases/download/indexes-v<VERSION>/<asset>`.
  It never calls the GitHub REST API and never uses `releases/latest`.
- **Assets:**
  - `<name>.index.xz`: the index `<name>.index`, compressed with xz. The
    name is the index name of `addict_dictionary::sources`, such as `oewn`.
  - `manifest.json`: the size and the SHA-256 of every `.index.xz` asset.
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
- **Order:** upload `manifest.json` last. The app reads the manifest first,
  so every asset it lists must already be there.

## The manifest

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
minutes: it downloads all 13 upstream files, from the URLs in
`crates/addict-dictionary/src/sources.rs`.

```sh
cargo run --release -p addict-dictionary --example build-indexes -- --out target/indexes
xz -t target/indexes/*.index.xz
```

`target/indexes` then holds one `<name>.index.xz` per index, `manifest.json`,
`WNDB_License.txt` and `release-body.md`, plus the uncompressed `.index`
files, which are not uploaded.

A source that fails (its upstream file is gone, the importer refuses it, `xz`
fails) does not stop the run. The builder goes on with the next source and
exits with status 1. It prints one `build-indexes: <index>: <error>` line per
failed source to stderr, then a line naming all of them. `manifest.json` and
`release-body.md` then list only the indexes that were built. Do not upload
that manifest over a release that still serves the failed indexes: the app
would no longer see them. Fix the source (a new pin in `import::KNOWN` and a
new URL in `sources.rs`, for example) and build again, or leave the release as
it is. The monthly release job keeps the published index of a failed source
instead; see [The monthly release job](#the-monthly-release-job).

For a new format, create the release with the indexes, then upload the
manifest. `--latest=false` keeps the app's own release marked as the latest
one:

```sh
gh release create indexes-v3 --repo doqhu/addict --target master --latest=false \
  --title "Dictionary indexes, format 3" \
  --notes-file target/indexes/release-body.md \
  target/indexes/*.index.xz target/indexes/WNDB_License.txt
gh release upload indexes-v3 --repo doqhu/addict target/indexes/manifest.json
```

To refresh the indexes of an existing release, replace the indexes first, then
the manifest. Between the two uploads, an app that reads the old manifest sees
a checksum mismatch, fetches the manifest again and still finds the old entry,
so that index fails on its row and the download is offered again at the next
start when no index got installed.

```sh
gh release upload indexes-v3 --repo doqhu/addict --clobber \
  target/indexes/*.index.xz target/indexes/WNDB_License.txt
gh release upload indexes-v3 --repo doqhu/addict --clobber target/indexes/manifest.json
gh release edit indexes-v3 --repo doqhu/addict --notes-file target/indexes/release-body.md
```

With `--previous <FILE>`, the `manifest.json` of the published release, the
builder keeps the published entry of each failed source that the file lists,
as the monthly release job does. Download it first with
`gh release download indexes-v3 --repo doqhu/addict --pattern manifest.json --dir <DIR>`.

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

`.github/workflows/indexes.yml` builds and uploads the indexes once a month,
on the 2nd at 03:17 UTC. To start it by hand, open Actions → Dictionary
indexes → Run workflow, or run:

```sh
gh workflow run indexes.yml --repo doqhu/addict
```

The job:

1. Asks the builder for the tag (`build-indexes --print-tag`), so it writes
   only the release of the current index format. A release of an older
   format is never read or written.
2. Downloads the release's `manifest.json`, when the release exists and has
   one. If that download fails, the job stops before it uploads anything.
3. Runs the builder with `--previous` set to that manifest.
4. Uploads the indexes built and `WNDB_License.txt`, then `manifest.json`,
   then replaces the release notes with `release-body.md`. A missing release
   is created first, with `--latest=false`. When the builder wrote no
   manifest (it could not start, or the previous manifest was unreadable),
   nothing is uploaded.
5. Lists the indexes built, kept and failed, with each failure's error, in
   the run's summary.

**Kept indexes.** When a source fails and the published manifest lists its
index, the builder copies that entry unchanged into the new manifest, at the
source's place in `sources::ROWS`, and writes its notice into the release
notes. Its upstream line then says that it is the index of an earlier run;
the edition is in the notice inside the file. Its asset is not uploaded, so
the published one stays and still matches the entry. The app goes on
downloading it. A failed source that the published manifest does not list
stays out of the new one.

**A red run.** The job ends with the builder's status: red when any source
failed, kept or not, after the other indexes were published. Read the
summary, fix the source (a new pin in `import::KNOWN` and a new URL in
`sources.rs`, for example), and start the job again.
