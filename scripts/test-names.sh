#!/usr/bin/env bash
# Proves that no test was lost in a move: the names of the functions marked
# `#[test]` or `#[tokio::test(...)]` under crates/ at a tag (v0.5.0 unless
# another revision is given) must all still be there in the working tree, as
# often as they were. Prints every lost name and exits 1 if there is any.
set -euo pipefail

cd "$(dirname "$0")/.."

BASE=${1:-v0.5.0}
PATTERN='#\[(tokio::)?test(\]|\()'

# The name of the first function after each test attribute of the Rust
# source on stdin; other attributes and doc comments may stand between.
names() {
    awk '
        /^[[:space:]]*#\[(tokio::)?test(\]|\()/ { pending = 1; next }
        pending && match($0, /fn[[:space:]]+[A-Za-z0-9_]+/) {
            name = substr($0, RSTART, RLENGTH)
            sub(/^fn[[:space:]]+/, "", name)
            print name
            pending = 0
        }
    '
}

base=$(mktemp)
now=$(mktemp)
trap 'rm -f "$base" "$now"' EXIT

if ! git rev-parse --verify --quiet "$BASE^{commit}" >/dev/null; then
    echo "error: $BASE is not a revision" >&2
    exit 1
fi

# grep's exit 1 is only "no match": the empty list is caught below.
{ git grep -l -E "$PATTERN" "$BASE" -- crates || [[ $? -eq 1 ]]; } | while read -r file; do
    git show "$file" | names
done | sort >"$base"

# Every Rust file of the working tree, tracked or not.
find crates -name '*.rs' -not -path '*/target/*' -print0 |
    { xargs -0 -r grep -l -E "$PATTERN" || [[ $? -eq 123 ]]; } | while read -r file; do
    names <"$file"
done | sort >"$now"

if [[ ! -s "$base" ]]; then
    echo "error: no test found at $BASE" >&2
    exit 1
fi

# comm compares the sorted lists line by line: a name that is there less
# often now is lost as many times.
lost=$(comm -23 "$base" "$now")
if [[ -n "$lost" ]]; then
    echo "error: tests lost since $BASE:" >&2
    printf '  %s\n' $lost >&2
    exit 1
fi
echo "$(wc -l <"$base") test names of $BASE are all there"
