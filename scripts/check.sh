#!/usr/bin/env bash
# One command that proves the product crates are healthy: format, lint, tests
# and a guard that keeps GTK out of addict-core, addict-platform,
# addict-dictionary and addict-deepl.
set -euo pipefail

cd "$(dirname "$0")/.."

PRODUCT=(addict-core addict-platform addict-dictionary addict-deepl addict-app)
GUARDED=(addict-core addict-platform addict-dictionary addict-deepl)
PKG_ARGS=()
for crate in "${PRODUCT[@]}"; do
    PKG_ARGS+=(-p "$crate")
done

# GTK, GDK, gio, glib and their -sys crates.
FORBIDDEN='^(gtk4|gtk4-sys|gdk4|gdk4-[a-z0-9-]+|gio|gio-sys|glib|glib-sys|gobject-sys)$'

echo "==> fmt"
cargo fmt "${PKG_ARGS[@]}" -- --check

echo "==> clippy"
cargo clippy "${PKG_ARGS[@]}" --all-targets -- -D warnings

# Without the dev-dependencies and their features: the product must build
# as it is shipped, where the sample dictionary of the tests does not exist.
echo "==> product build"
cargo check "${PKG_ARGS[@]}"

echo "==> test"
cargo test "${PKG_ARGS[@]}"

echo "==> no-GTK guard"
for crate in "${GUARDED[@]}"; do
    # Own assignment: a cargo tree failure aborts the script (set -e).
    tree=$(cargo tree -p "$crate" --prefix none --edges normal,build,dev --target all)
    leaked=$(printf '%s\n' "$tree" | awk '{print $1}' | grep -E "$FORBIDDEN" | sort -u || true)
    if [[ -n "$leaked" ]]; then
        echo "error: $crate depends on forbidden packages:" >&2
        printf '  %s\n' $leaked >&2
        exit 1
    fi
    echo "$crate: ok"
done

echo "ok"
