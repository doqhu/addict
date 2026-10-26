## Use of AI in addict

addict was built with heavy use of generative AI. Below is each part of the
project, with how much of it AI wrote. The figures come from the git history
as of October 2026.

### Code

Almost all of the code was written by AI: Claude (Anthropic's Opus and Sonnet
models), through Claude Code. About 93% of the lines in the current source
tree come from AI-assisted commits. 103 of the project's 109 commits were
AI-assisted. This covers all crates and their tests.

The maintainer chose the features, directed the work and tested the result in
use. The maintainer did not review every line of the AI-written code.

### Documentation

The README, CONTRIBUTING and this disclosure are drafted by AI and edited by
the maintainer. Code comments were written by AI along with the code.

### Packaging, CI and project configuration

- **Flathub manifest:** written by the maintainer, with no AI-generated or
  AI-assisted content. It is separate from the development manifest below.
- **Flathub submission:** the pull request, its description, commit messages
  and replies are written by the maintainer, without AI.
- **Everything else:** the CI workflows, the secret-scan configuration, the
  check and acceptance scripts, the development Flatpak manifest
  (`packaging/flatpak`), its sources script (`scripts/flatpak-sources.sh`),
  the desktop entry (`data`), the AppImage recipe and the project
  configuration files (`Cargo.toml`, `REUSE.toml`, `.gitignore`) are written
  by AI under the maintainer's direction.

### Artwork

The app icon was made with an image-generation tool. The smaller sizes are
built from it by a script.

### Not counted above

AI was also used for research, discussion and debugging. That use did not add
anything to the repository by itself.
