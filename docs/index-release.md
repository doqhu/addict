# Publishing the dictionary indexes

On its first run, with no index installed, addict offers to download the
English dictionary (Open English WordNet). The app downloads prebuilt indexes
from a GitHub data release of this repository. This page is the contract
between that release and the app, and the steps the maintainer follows to
publish it. The schema lives in `crates/addict-dictionary/src/release.rs`,
which both the builder and the app read.

## The release

- **Tag:** `indexes-v<VERSION>`, where `<VERSION>` is
  `addict_dictionary::format::VERSION` (now `2`, so the tag is `indexes-v2`).
  The data release is separate from the app's `vX.Y.Z` releases.
- **URLs:** the app fetches only
  `https://github.com/doqhu/addict/releases/download/indexes-v<VERSION>/<asset>`.
  It never calls the GitHub REST API and never uses `releases/latest`.
- **Assets:**
  - `<name>.index.xz`: the index `<name>.index`, compressed with xz. The
    name is the index name of `addict_dictionary::sources`, such as `oewn`.
  - `manifest.json`: the size and the SHA-256 of every `.index.xz` asset.
  - `WNDB_License.txt`: the Princeton WordNet license, which ships with any
    copy of the OEWN index.
  - The release notes (`release-body.md`) hold each index's credit, license,
    upstream link, a "modified" notice and a warranty disclaimer.
- **Older formats:** a release for an older format is never changed or
  deleted. Apps that read only that format keep downloading from it. A new
  index format gets a new tag.
- **Order:** upload `manifest.json` last. The app reads the manifest first,
  so every asset it lists must already be there.

## The manifest

```json
{
  "format": 2,
  "indexes": [
    {
      "name": "oewn",
      "format": 2,
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
over the old one.

## Building and uploading

Build the assets. This needs `xz` on `PATH` and network access:

```sh
cargo run -p addict-dictionary --example build-indexes -- --out target/indexes
xz -t target/indexes/oewn.index.xz
```

`target/indexes` then holds `oewn.index.xz`, `manifest.json`,
`WNDB_License.txt` and `release-body.md`, plus the uncompressed `oewn.index`,
which is not uploaded.

For a new format, create the release with the indexes, then upload the
manifest. `--latest=false` keeps the app's own release marked as the latest
one:

```sh
gh release create indexes-v2 --repo doqhu/addict --target master --latest=false \
  --title "Dictionary indexes, format 2" \
  --notes-file target/indexes/release-body.md \
  target/indexes/oewn.index.xz target/indexes/WNDB_License.txt
gh release upload indexes-v2 --repo doqhu/addict target/indexes/manifest.json
```

To refresh the indexes of an existing release, replace the indexes first, then
the manifest. Between the two uploads, an app that reads the old manifest sees
a checksum mismatch and offers the download again at its next start.

```sh
gh release upload indexes-v2 --repo doqhu/addict --clobber \
  target/indexes/oewn.index.xz target/indexes/WNDB_License.txt
gh release upload indexes-v2 --repo doqhu/addict --clobber target/indexes/manifest.json
gh release edit indexes-v2 --repo doqhu/addict --notes-file target/indexes/release-body.md
```

After the upload, check by hand that the first-run download works from a
clean profile: with no index and no choice file, start addict and press
Download. `oewn.index` must appear in the data directory, and the window's
index line must update. The choice file and the data directory are:

- native build: `~/.config/addict/first-run-download` and
  `~/.local/share/addict`;
- Flatpak: `~/.var/app/io.github.doqhu.Addict/config/addict/first-run-download`
  and `~/.var/app/io.github.doqhu.Addict/data/addict`.
