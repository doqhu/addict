#!/usr/bin/env bash
# Regenerates packaging/flatpak/cargo-sources.json from Cargo.lock: the crates
# the Flatpak build vendors, since flatpak-builder gives the build no network.
# Run it after every change to Cargo.lock. Needs flatpak-cargo-generator
# (flatpak-builder-tools) on PATH, or its path in FLATPAK_CARGO_GENERATOR.
set -euo pipefail

cd "$(dirname "$0")/.."

GENERATOR=${FLATPAK_CARGO_GENERATOR:-flatpak-cargo-generator}

"$GENERATOR" Cargo.lock -o packaging/flatpak/cargo-sources.json
