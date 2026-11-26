#!/usr/bin/env bash
# One command that proves the product crates are healthy: format, lint, tests
# and a guard that keeps GTK out of addict-core, addict-platform,
# addict-dictionary, addict-deepl and addict-engine, and the Linux adapters
# out of addict-engine (AD-17).
#
# `scripts/check.sh guard` runs only the guard and the check of addict-engine
# on its own; CI runs that form.
set -euo pipefail

cd "$(dirname "$0")/.."

MODE=${1:-all}
case "$MODE" in
    all | guard) ;;
    *)
        echo "usage: $0 [guard]" >&2
        exit 2
        ;;
esac

PRODUCT=(addict-core addict-platform addict-dictionary addict-deepl addict-engine addict-app)
GUARDED=(addict-core addict-platform addict-dictionary addict-deepl addict-engine)
PKG_ARGS=()
for crate in "${PRODUCT[@]}"; do
    PKG_ARGS+=(-p "$crate")
done

# GTK, GDK, gio, glib and their -sys crates.
GTK='gtk4|gtk4-sys|gdk4|gdk4-[a-z0-9-]+|gio|gio-sys|glib|glib-sys|gobject-sys'
# The Linux adapters and what only they bring: the portals, D-Bus, the tray.
PLATFORM='addict-platform|ashpd|zbus|ksni'

# The packages `crate` must not reach, as one anchored pattern.
forbidden() {
    case "$1" in
        addict-engine) echo "^($GTK|$PLATFORM)$" ;;
        *) echo "^($GTK)$" ;;
    esac
}

if [[ "$MODE" == all ]]; then
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
fi

echo "==> dependency guard"
for crate in "${GUARDED[@]}"; do
    # Own assignment: a cargo tree failure aborts the script (set -e).
    tree=$(cargo tree -p "$crate" --prefix none --edges normal,build,dev --target all)
    leaked=$(printf '%s\n' "$tree" | awk '{print $1}' | grep -E "$(forbidden "$crate")" | sort -u || true)
    if [[ -n "$leaked" ]]; then
        echo "error: $crate depends on forbidden packages:" >&2
        printf '  %s\n' $leaked >&2
        exit 1
    fi
    echo "$crate: ok"
done

# Alone: a feature the engine needs must not arrive only through the
# feature unification of a workspace build.
echo "==> addict-engine alone"
cargo check -p addict-engine

echo "ok"
