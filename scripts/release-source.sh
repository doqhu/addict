#!/usr/bin/env bash
# The source tarball of the release: the tree of the checked-out commit with
# every crate vendored, so it builds with no network.
#
#   release-source.sh VERSION OUT_DIR
#
# Run it from the checkout. OUT_DIR must not exist or be empty; it receives,
# with NAME = addict-VERSION-vendored.tar.xz:
#
#   NAME           the tree under the one top directory addict-VERSION/: the
#                  `git archive` of HEAD, plus vendor/ (`cargo vendor --locked`
#                  of its Cargo.lock) and .cargo/config.toml, which replaces
#                  crates-io with `directory = "vendor"`
#   NAME.sha256    its SHA-256, in the format of sha256sum
#
# With the same cargo, tar and xz, the same commit gives the same bytes:
# names sorted, the commit time as every mtime, owner and group 0,
# normalised modes, single-threaded xz. Their versions are not pinned, so a
# later run on a newer runner image may give other bytes.
# Before writing anything it unpacks the tarball and runs
# `cargo metadata --offline --locked` in it with an empty CARGO_HOME. It
# refuses a Cargo.lock with a source other than crates.io, and a tree that
# already has .cargo/config.toml, .cargo/config or vendor/. Any failure exits
# non-zero and leaves OUT_DIR as it was.
#
# `cargo vendor` needs the network (or a filled CARGO_HOME). For tests:
# ASSET_LIMIT (bytes, default 2147483648) is the size the tarball must stay
# under. scripts/release-tests.sh tests it; docs/release.md describes the
# asset.
set -Eeuo pipefail

if [[ $# != 2 ]]; then
    echo "usage: $0 VERSION OUT_DIR" >&2
    exit 2
fi
version=$1
out=$(realpath -m -- "$2")
limit=${ASSET_LIMIT:-2147483648}

die() { echo "::error::$*" >&2; exit 1; }

[[ $version =~ ^[A-Za-z0-9._-]+$ ]] || die "unusable version '$version': no source tarball is written."
if [[ -e $out ]]; then
    [[ -d $out && -z $(ls -A -- "$out") ]] || die "$out exists and is not an empty directory: no source tarball is written."
fi
top=addict-$version
name=$top-vendored.tar.xz

commit=$(git rev-parse --verify HEAD^{commit})
mtime=$(git log -1 --format=%ct "$commit")

work=$(mktemp -d)
cleanup() { rm -rf -- "$work"; }
trap cleanup EXIT

# The tree of the commit, as git has it: no untracked or ignored file.
mkdir -- "$work/tree"
git archive --format=tar --prefix="$top/" "$commit" | tar -x -C "$work/tree"
src=$work/tree/$top

for p in .cargo/config.toml .cargo/config vendor; do
    if [[ -e $src/$p || -L $src/$p ]]; then
        die "the tree already has $p: no source tarball is written."
    fi
done
[[ -f $src/Cargo.lock ]] || die "the tree has no Cargo.lock: no source tarball is written."
crates_io='registry+https://github.com/rust-lang/crates.io-index'
crates_io_sparse='sparse+https://index.crates.io/'
while IFS= read -r line; do
    source=${line#source = \"}
    source=${source%\"}
    if [[ $source != "$crates_io" && $source != "$crates_io_sparse" ]]; then
        die "Cargo.lock has a source other than crates.io: $source: no source tarball is written."
    fi
done < <(grep -E '^source = ' -- "$src/Cargo.lock" || true)

# The crates. cargo prints a configuration with an absolute path; the
# tarball carries its own, relative one, so it works wherever it is
# unpacked, also with the top directory stripped.
(cd -- "$src" && cargo vendor --locked --quiet vendor > /dev/null)
mkdir -p -- "$src/vendor" "$src/.cargo"
cat > "$src/.cargo/config.toml" <<'TOML'
# Written by scripts/release-source.sh: the crates come from vendor/, so the
# tree builds with no network (cargo build --offline --locked).
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
TOML

LC_ALL=C tar -C "$work/tree" --sort=name --format=gnu \
    --owner=0 --group=0 --numeric-owner --mtime="@$mtime" \
    --mode='u+rwX,go+rX,go-w' \
    -cf - -- "$top" \
    | xz -6 --threads=1 -c > "$work/$name"

size=$(stat -c %s -- "$work/$name")
((size < limit)) || die "the source tarball has $size bytes, not under $limit: no source tarball is written."

# The tarball itself, unpacked elsewhere, resolves with no network and no
# crate outside it.
mkdir -- "$work/check" "$work/cargo-home"
tar -C "$work/check" -xJf "$work/$name"
[[ $(ls -A -- "$work/check") == "$top" ]] || die "the tarball has more than the top directory $top: no source tarball is written."
(cd -- "$work/check/$top" && CARGO_HOME=$work/cargo-home cargo metadata --offline --locked --format-version 1 > /dev/null) \
    || die "cargo metadata --offline --locked fails in the unpacked tarball: no source tarball is written."

(cd -- "$work" && sha256sum -- "$name" > "$name.sha256")
mkdir -p -- "$out"
mv -- "$work/$name" "$work/$name.sha256" "$out/"
echo "Source tarball of $commit in $out:"
cat -- "$out/$name.sha256"
ls -l -- "$out"
