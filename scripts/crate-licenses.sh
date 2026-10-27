#!/usr/bin/env bash
# Regenerates packaging/licenses/crate-licenses.txt from Cargo.lock: the
# licenses of the crates compiled into addict, which the Flatpak installs
# under share/licenses. Run it after every change to Cargo.lock. Needs
# cargo-about 0.9.3 (`cargo install --locked cargo-about@0.9.3`); another
# version may write a different file.
set -euo pipefail

cd "$(dirname "$0")/.."

# Offline after the fetch: the license texts come from the crate files alone,
# so the list depends on Cargo.lock and nothing else.
cargo fetch --locked --quiet
cargo about generate --frozen \
    --config packaging/licenses/about.toml \
    --output-file packaging/licenses/crate-licenses.txt \
    packaging/licenses/crates.hbs
# Some crates' texts have CRLF line ends; git would store them as LF, and the
# committed file would then differ from a fresh run.
sed -i 's/\r$//' packaging/licenses/crate-licenses.txt
