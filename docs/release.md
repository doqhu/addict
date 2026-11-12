# Releasing the app

Merging the release pull request of
[release-please](https://github.com/googleapis/release-please) tags the
release `vX.Y.Z` and publishes its GitHub Release at once. The push of that
tag starts the workflow `.github/workflows/release.yml`, which attaches the
builds to the Release. This page says what that run does and what the
maintainer does when it fails. The dictionary indexes have their own data
release, `indexes-v<N>`, which [index-release.md](index-release.md)
describes; this workflow never touches it.

## A tag run

- **Trigger:** only a tag that matches `v[0-9]+.[0-9]+.[0-9]+`, such as
  `v0.2.0`. `indexes-v3`, `v0.2.0-rc1` or `v1.2` start no run, and no tag
  starts CI (`ci.yml` runs on branch pushes and pull requests only). A guard
  job checks the tag again with `scripts/release-ref.sh`, and checks that it
  is `v` followed by the version in `Cargo.toml`.
- **Checks:** the run calls `ci.yml`, so every CI job runs on the tag's
  commit; they report as `ci / test`, `ci / flatpak` and so on.
- **Upload:** only when every job passed. The upload job takes the Flatpak
  bundle that the run's `flatpak` job built and linted, with no second build,
  and `scripts/release-upload.sh` uploads it to the Release of the tag. It is
  the only job with `contents: write`.
- **One run at a time:** a run on the same tag waits for the running one.
  GitHub keeps only one waiting run per group: a newer waiting run cancels
  an older one.
- **Missing Release:** release-please creates the Release before the tag's
  run starts. If the run finds no Release for the tag, it fails with a
  message that names the tag, and creates nothing.

## The assets

- `Addict-<version>-x86_64.flatpak`: the Flatpak bundle of the app, with the
  version from `Cargo.toml`, such as `Addict-0.2.0-x86_64.flatpak`.
- `Addict-<version>-x86_64.flatpak.sha256`: its SHA-256, in the format of
  `sha256sum`. Check a download with

  ```sh
  sha256sum --check Addict-0.2.0-x86_64.flatpak.sha256
  ```

An upload replaces an asset of the same name, so a second run on the same
tag succeeds.

## When a tag run fails

A failed check leaves the Release published without assets: release-please
has no draft mode here. Then:

- If the failure is not in the code (a network error, a runner problem),
  re-run the failed run in Actions (**Re-run failed jobs**). It builds the
  same commit and uploads to the same Release.
- If the fix needs a commit, merge it to `main` and ship it as the next patch
  version through the next release pull request.

A tag is never moved or deleted, and its Release is not deleted either.

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
own bundle and checksum and records its branch, commit and run in the notes.
All dry runs, from any branch, share one concurrency group, so a dry run
waits for the running one rather than uploading over it; a newer waiting dry
run cancels an older waiting one.
If a release named `dry-run` is published rather than a draft, the dry run
refuses to upload.

To check a dry run:

```sh
gh release download dry-run --dir dry-run --clobber
cd dry-run
sha256sum --check Addict-*-x86_64.flatpak.sha256
flatpak install --user Addict-*-x86_64.flatpak
```

To delete the draft when it is no longer needed (the next dry run creates it
again):

```sh
gh release delete dry-run --yes
```

## Tests

`scripts/release-tests.sh` runs locally and in CI's job `workflows`, with no
network and no GitHub access. It checks:

- the guard `scripts/release-ref.sh` over the tag names above;
- in the workflow files: the release trigger is exactly the tag filter
  `v[0-9]+.[0-9]+.[0-9]+`, CI's push has a branch filter, the upload job
  needs both `guard` and `ci` with no `if:` (so a failed check skips it), it
  is the only job with `contents: write`, and the concurrency group;
- `scripts/release-upload.sh`, the upload job's logic, against a stub `gh`
  that keeps its releases in a temporary directory and records its calls: a
  real release, a second run, a dry run with and without an older draft, a
  missing Release, a published `dry-run` and an unexpected `gh` error.
