# Releasing the app

Merging the release pull request of
[release-please](https://github.com/googleapis/release-please) tags the
release `vX.Y.Z` and creates its GitHub Release as a draft. The config sets
`draft: true` and `force-tag-creation: true`: draft releases alone do not
create tags, so explicit tag creation is needed to start the workflow
`.github/workflows/release.yml`. It builds and checks the assets, attaches
them to the draft, then publishes it. With immutable releases enabled,
publication locks the assets; every upload must finish first. This page says
what that run does and what the maintainer does when it fails. The dictionary
indexes have their own data release, `indexes-v<N>`, in the separate
repository `doqhu/addict-indexes`, which [index-release.md](index-release.md)
describes; this workflow never touches it. The frozen `indexes-v3` release of
this repository, which apps 0.2.x and 0.4.x read, is never touched either.

## A tag run

- **Trigger:** only a tag that matches `v[0-9]+.[0-9]+.[0-9]+`, such as
  `v0.2.0`. `indexes-v3`, `v0.2.0-rc1` or `v1.2` start no run, and no tag
  starts CI (`ci.yml` runs on pull requests, pushes to `main` and manual runs
  only). A guard
  job checks the tag again with `scripts/release-ref.sh`, and checks that it
  is `v` followed by the version in `Cargo.toml`.
- **Checks:** the run calls `ci.yml`, so every CI job runs on the tag's
  commit; they report as `ci / test`, `ci / flatpak` and so on.
- **Source tarball:** the job `source` runs `scripts/release-source.sh`,
  which builds the tarball of the tag's tree with the crates vendored and
  checks that cargo resolves it offline (see below). It has
  `contents: read` only.
- **Upload:** only when every job passed. The upload job takes the Flatpak
  bundle that the run's `flatpak` job built and linted, the source tarball
  that the run's `source` job built and checked, and the handoff that the
  run's `appimage` job built and checked, with no second build.
  `scripts/release-appimage-assets.sh` checks the handoff again and writes
  the AppImage assets, and `scripts/release-upload.sh` uploads everything to
  the draft Release of the tag, then publishes it only after every upload
  succeeds. Published releases are refused before any mutation. It is the
  only job with `contents: write`.
- **One run at a time:** a run on the same tag waits for the running one.
  GitHub keeps only one waiting run per group: a newer waiting run cancels
  an older one.
- **Missing Release:** release-please creates the tag before creating the
  draft. By the time the builds finish, the draft should exist. If draft
  creation failed and the upload job finds no Release for the tag, it fails
  with a message that names the tag, and creates nothing. Restore the draft
  for that existing tag with the correct release notes before retrying.

## The assets

- `Addict-<version>-x86_64.flatpak`: the Flatpak bundle of the app, with the
  version from `Cargo.toml`, such as `Addict-0.2.0-x86_64.flatpak`.
- `Addict-<version>-x86_64.flatpak.sha256`: its SHA-256, in the format of
  `sha256sum`. Check a download with

  ```sh
  sha256sum --check Addict-0.2.0-x86_64.flatpak.sha256
  ```

The source tarball:

- `addict-<version>-vendored.tar.xz`, such as
  `addict-0.2.0-vendored.tar.xz`: the tree of the tag's commit
  (`git archive`), with every crate of its `Cargo.lock` in `vendor/`
  (`cargo vendor --locked`) and a `.cargo/config.toml` that replaces
  crates.io with `directory = "vendor"`, relative to the tree. Everything is
  under one top directory, `addict-<version>/`. 0.1.1's is 28 MB.
- `addict-<version>-vendored.tar.xz.sha256`: its SHA-256, in the format of
  `sha256sum`.

Its URL is the input of the Flathub manifest, as an `archive` source with
its `sha256` and no `cargo-sources.json`:

```text
https://github.com/doqhu/addict/releases/download/v<version>/addict-<version>-vendored.tar.xz
```

With the same cargo, GNU tar and xz, the same commit gives the same bytes:
the names are sorted, every mtime is the commit time, every owner is 0, and
xz runs on one thread. A retry while the release is still a draft can
replace the asset with identical bytes as long as the runner image has the
same versions of those tools; they are not pinned, and a later run on a
newer image may give other bytes. Published assets are never replaced. The
upload job checks the tarball against its `.sha256` before any upload. Before
the upload job gets it, `scripts/release-source.sh` unpacks it and runs
`cargo metadata --offline --locked` in it with an empty `CARGO_HOME`; a
failure uploads nothing. It refuses a `Cargo.lock` with a source other than
crates.io (a git dependency), and a tree that already has
`.cargo/config.toml`, `.cargo/config` or `vendor/`, naming the reason.

To check it and build from it with no network (cargo reads the project's
`.cargo/config.toml` whatever `CARGO_HOME` is; flatpak-builder's `archive`
source strips the top directory, and the configuration still applies):

```sh
sha256sum --check --strict addict-0.2.0-vendored.tar.xz.sha256
tar -xJf addict-0.2.0-vendored.tar.xz
cd addict-0.2.0
CARGO_HOME=$(mktemp -d) cargo build --offline --locked --release -p addict-app
```

To build it locally, from a clean checkout of the commit (`cargo vendor`
needs the network):

```sh
scripts/release-source.sh 0.2.0 "$(mktemp -d)"
```

The AppImage, with `NAME` standing for `Addict-<version>-x86_64.AppImage`:

- `NAME`: the AppImage, such as `Addict-0.2.0-x86_64.AppImage`.
- `NAME.sha256`: its SHA-256, in the format of `sha256sum`.
- `NAME.build.txt`, `NAME.inputs.lock.json`, `NAME.rpms.txt`,
  `NAME.layout.txt`, `NAME.notices.json`, `NAME.components.sha256`: the
  metadata of the build, under their names in the handoff (see
  [appimage-build.md](appimage-build.md)). `.notices.json` is the inventory
  of components, licences and corresponding sources.
- `NAME.sources.tar.00`, `NAME.sources.tar.01`, ...: the corresponding
  sources of the GPL and LGPL components and the rest, as one tar of the
  directory `NAME.sources/` split into parts of at most 1.9 GiB, since a
  GitHub asset must be under 2 GiB. 0.1.1's set is 195 sources, 2.5 GiB, in
  two parts.
- `NAME.sources.tar.sha256`: the SHA-256 of each part.

Every asset name uses only `A-Za-z0-9._-`, so GitHub renames none. Before
any upload, `scripts/release-appimage-assets.sh` checks the handoff tar
against its checksum, the image against its `.sha256`, every source against
`SHA256SUMS`, and the source names against the inventory in
`NAME.notices.json`, and that each asset is under 2 GiB; any failure names
the file and uploads nothing. The image and its checksum are uploaded last,
one file at a time, so a failed run never leaves the image on a Release
without its sources.

To check the image and reassemble and check the source set, in the
directory of the downloaded assets:

```sh
sha256sum --check --strict Addict-0.2.0-x86_64.AppImage.sha256
sha256sum --check --strict Addict-0.2.0-x86_64.AppImage.sources.tar.sha256
cat Addict-0.2.0-x86_64.AppImage.sources.tar.[0-9][0-9] | tar -x
cd Addict-0.2.0-x86_64.AppImage.sources
sha256sum --check --strict SHA256SUMS
```

While the release is a draft, an upload replaces an asset of the same
name, so an interrupted upload can be retried. A run against an already
published release fails before uploading anything.

## When a tag run fails

A failed check or upload leaves the Release as a draft. An interrupted
upload may leave some assets on the draft; a retry replaces them before
publishing. Then:

- If the failure is not in the code (a network error, a runner problem),
  re-run the failed run in Actions (**Re-run failed jobs**). It builds the
  same commit and uploads to the same draft, then publishes after success.
  A failure to publish also leaves the completed draft retryable.
- If the fix needs a commit, merge it to `main` and ship it as the next patch
  version through the next release pull request.

A tag is never moved or deleted, and its Release is not deleted either.

The existing `v0.3.0` release was published before this fix and is immutable
with no uploaded assets. Re-running its old workflow cannot repair it, and
a rerun uses the workflow from its original commit. Ship the fix and app
assets through the next patch release. Its successful build artifacts remain
available from run `44132391755` until their retention expires; these are
workflow artifacts, not published release assets.

## The dry run

Started by hand, the workflow is a dry run. It needs the workflow on `main`
first, since GitHub offers a manual start only for workflows on the default
branch. Start it on a branch that contains this workflow, one cut from or
rebased on `main` after the merge: the run uses the workflow files of that
branch, and an older branch has no `release.yml`, and its `ci.yml` cannot be
called:

```sh
gh workflow run release.yml --ref <branch>
```

A dry run runs the same guard, checks and upload, but uploads to the draft
release `dry-run`, which it creates when missing. A draft has no tag, so a
dry run never creates a tag, never publishes, and never touches a `vX.Y.Z` or
`indexes-v<N>` release. Each dry run replaces the draft's assets with its
own, the Flatpak, source tarball and AppImage assets above, deletes every
other asset, and records its branch, commit and run in the notes.
All dry runs, from any branch, share one concurrency group, so a dry run
waits for the running one rather than uploading over it; a newer waiting dry
run cancels an older waiting one.
If a release named `dry-run` is published rather than a draft, the dry run
refuses to upload.

To check a dry run (about 2.8 GiB to download, and as much again to
reassemble the sources):

```sh
gh release download dry-run --dir dry-run --clobber
cd dry-run
sha256sum --check Addict-*-x86_64.flatpak.sha256
flatpak install --user Addict-*-x86_64.flatpak
sha256sum --check --strict addict-*-vendored.tar.xz.sha256
ls addict-*-vendored.tar.xz  # addict-<version of Cargo.toml>-vendored.tar.xz
tar -xJf addict-*-vendored.tar.xz
(cd addict-*/ && CARGO_HOME=$(mktemp -d) cargo build --offline --locked -p addict-app)
sha256sum --check --strict Addict-*-x86_64.AppImage.sha256
sha256sum --check --strict Addict-*-x86_64.AppImage.sources.tar.sha256
cat Addict-*-x86_64.AppImage.sources.tar.[0-9][0-9] | tar -x
(cd Addict-*-x86_64.AppImage.sources && sha256sum --check --strict SHA256SUMS)
ls Addict-*-x86_64.AppImage.sources | grep -vc '^SHA256SUMS$'  # 195 for 0.1.1
chmod +x Addict-*-x86_64.AppImage && ./Addict-*-x86_64.AppImage
```

A second dry run leaves the same asset names on the draft, once each.

To delete the draft when it is no longer needed (the next dry run creates it
again):

```sh
gh release delete dry-run --yes
```

## Tests

`scripts/release-tests.sh` runs locally and in CI's job `workflows`, with no
network and no GitHub access. It needs `python3` with PyYAML, `git` and
`xz`. It checks:

- the guard `scripts/release-ref.sh` over the tag names above;
- in the workflow files: the release trigger is exactly the tag filter
  `v[0-9]+.[0-9]+.[0-9]+`, release-please enables draft and explicit tag
  creation, CI's push has a branch filter, the upload job
  needs `guard`, `ci` and `source` with no `if:` (so a failed check skips
  it), it is the only job with `contents: write`, the `source` job needs the
  guard and has `contents: read`, and the concurrency group;
- `scripts/release-upload.sh`, the upload job's logic, against a stub `gh`
  that keeps its releases in a temporary directory and records its calls:
  a draft version published only after the final upload, retries after upload
  and publication failures, published releases refused before mutation, dry
  runs with and without an older draft, a missing Release and an unexpected
  `gh` error; and
  with many files, one upload call per file in the order given;
- `scripts/release-source.sh` on a fixture git repository with one crate,
  with a stub `cargo` (a real `cargo vendor` needs the network) whose
  `vendor` writes one fake crate and whose `metadata` records where it ran:
  the tarball and its checksum (one top directory, owner 0 and the commit
  time on every member, no untracked file, the crate in `vendor/`, the
  relative `.cargo/config.toml`, the offline check in the unpacked tarball
  with an empty `CARGO_HOME`), the same bytes from two runs, and the
  refusals that write nothing: a git source in `Cargo.lock`, a tracked
  `.cargo/config.toml` or `.cargo/config`, a tracked `vendor/`, and a
  failing offline check; a `Cargo.lock` with the sparse crates.io index
  passes;
- `scripts/release-appimage-assets.sh` and the upload job's two AppImage
  steps, read from `release.yml`, on a small fixture handoff and the fixture
  source tarball: a complete handoff (every asset uploaded, the Flatpak and
  the source tarball first, the image and its checksum last, every name
  only `A-Za-z0-9._-` and under 2 GiB, the parts reassembling to a set that
  passes `SHA256SUMS`), refusal after publication, a dry run after an older
  one that remains unpublished, and the
  refusals that upload nothing: a source removed, a source and its
  `SHA256SUMS` line removed, a source not in the inventory, a corrupt source,
  a corrupt image, a missing metadata file, a corrupt handoff tar, and an
  asset too large under a lowered limit.
