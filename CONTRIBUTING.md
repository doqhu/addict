# Contributing to addict

Thank you for your interest in addict. Bug reports, ideas and pull requests
are welcome.

## Reporting a problem

Open an issue on GitHub. Say what you did, what you expected and what
happened, and add:

- your distribution and its version, and your desktop (GNOME, KDE Plasma)
  with its version;
- how you installed addict: from source (with the commit) or from a
  package;
- what addict printed on the terminal, if you started it from one.

Do not paste your DeepL key, and check that a screenshot does not show it.

## Proposing a change

For anything larger than a small fix, open an issue first, so we can agree on
the change before you spend time on it. Then:

1. Fork the repository and make a branch from `master`.
2. Make the change, with tests for new behaviour.
3. Run the checks below.
4. Open a pull request that says what the change does and why.

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/):
`feat(window): …`, `fix(ocr): …`, `docs: …`, `ci: …`, with a short subject
in the imperative.

### AI-assisted contributions

Code, docs or other content written with an AI tool are welcome, if you
disclose it: add a `Co-Authored-By` trailer that names the tool or model to
each commit with such content, for example

```
Co-Authored-By: Claude <noreply@anthropic.com>
```

You are responsible for what you submit: read it, test it and be ready to
explain it. The project's own use of AI is described in
[AI-DISCLOSURE.md](AI-DISCLOSURE.md), and Flathub requires it to stay
accurate.

## Building and checking

Build addict as the [README](README.md#build-from-source) says. Before you
open a pull request, run:

```sh
scripts/check.sh
```

It runs `cargo fmt --check`, `cargo clippy` with warnings as errors, a build
of the product without the test features, all the tests, and a guard that
keeps GTK out of every crate but `addict-app`. Some tests run Tesseract, so
install it with its English data. The portal and forwarded-launch tests also
need `dbus-run-session` and `dbus-daemon` (`sudo dnf install dbus-daemon` on
Fedora). They create isolated session buses and do not use your desktop portal.

CI on GitHub runs the format and lint checks, the tests, `reuse lint`, a
secret scan of the whole history, and builds the Flatpak and checks it with
`flatpak-builder-lint`, but not the build without the test features or the
GTK guard, so run `scripts/check.sh` yourself.

The Flatpak build has no network, so it takes the crates from
`packaging/flatpak/cargo-sources.json`. If your change touches `Cargo.lock`,
regenerate that file with `scripts/flatpak-sources.sh` (it needs
`flatpak-cargo-generator` from flatpak-builder-tools) and commit it too.
Regenerate the list of the crates' licenses the same way, with
`scripts/crate-licenses.sh` (it needs `cargo-about` 0.9.3), and commit
`packaging/licenses/crate-licenses.txt`. CI regenerates both files and fails
when either differs from the committed one.

Before you try a Flatpak you built from `packaging/flatpak`, run
`addict uninstall` and quit the running addict, as the
[README](README.md#build-from-source) says.

## The workspace

- `crates/addict-core` -- the product identity and the domain types.
- `crates/addict-platform` -- the desktop adapters: portals, screen capture
  and Tesseract recognition.
- `crates/addict-dictionary` -- the dictionary index format, its reader and
  the importers of each dictionary source.
- `crates/addict-deepl` -- the DeepL client and its key file.
- `crates/addict-app` -- the `addict` program: the resident process, the
  window and the commands.

Only `addict-app` may depend on GTK.

## License

addict is licensed under the MIT License. By contributing, you agree that
your contribution is licensed under it too. `REUSE.toml` covers every file,
so a new file needs no license header. Data from another source needs its own
entry in `crates/addict-dictionary/src/sources.rs`, which `CREDITS` is
generated from. Any other file with third-party content, such as a screenshot
that shows dictionary text, needs its own annotation in `REUSE.toml`; so does
a file whose format asks for another license, such as the MetaInfo
(CC0-1.0).
