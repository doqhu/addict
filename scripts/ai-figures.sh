#!/usr/bin/env bash
# Prints the figures that AI-DISCLOSURE.md and the README quote, for a revision
# (default HEAD): how many commits are AI-assisted, and what share of the lines
# in the tree at that revision were last changed by one.
#
# AI-assisted means the message has a `Co-Authored-By: Claude ...` or
# `Co-Authored-By: Codex ...` trailer (any case), or the commit is one of the
# six the maintainer confirmed on 2026-11-12 were written with Codex but carry
# no trailer. Lines are counted with `git blame` over every tracked file except
# *.png and Cargo.lock.
set -euo pipefail

cd "$(dirname "$0")/.."
rev=$(git rev-parse --verify "${1:-HEAD}^{commit}")

untrailered=(537c78a 1b3df8e 7860f00 026c7bc f0c7928 2248ffa)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

for s in "${untrailered[@]}"; do git rev-parse --verify "$s^{commit}"; done >"$tmp/ai"
git rev-list "$rev" | while read -r h; do
    msg=$(git show -s --format=%B "$h")
    grep -qiE '^Co-Authored-By: *(Claude|Codex)' <<<"$msg" && echo "$h"
done >>"$tmp/ai" || true
sort -u "$tmp/ai" -o "$tmp/ai"
# Only commits that are ancestors of $rev count (the six may lie outside it).
git rev-list "$rev" | sort >"$tmp/all"
comm -12 "$tmp/ai" "$tmp/all" >"$tmp/ai.rev"

git ls-tree -r --name-only "$rev" | grep -viE '\.png$|(^|/)Cargo\.lock$' |
    while read -r f; do
        git blame --line-porcelain "$rev" -- "$f" | grep -aE '^[0-9a-f]{40} ' | cut -d' ' -f1
    done >"$tmp/blame"

commits=$(wc -l <"$tmp/all")
ai_commits=$(wc -l <"$tmp/ai.rev")
lines=$(wc -l <"$tmp/blame")
ai_lines=$(grep -cFf "$tmp/ai.rev" "$tmp/blame" || true)
echo "revision:     $rev"
claude=$(git rev-list "$rev" -i --grep='^Co-Authored-By: *Claude' | wc -l)
echo "commits:      $ai_commits of $commits AI-assisted ($claude with a Claude trailer, the rest Codex)"
echo "lines:        $ai_lines of $lines from AI-assisted commits"
awk -v a="$ai_lines" -v t="$lines" 'BEGIN { printf "share:        %.2f%%\n", 100 * a / t }'
