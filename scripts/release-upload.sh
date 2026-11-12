#!/usr/bin/env bash
# The upload of the release workflow: puts the given files on a GitHub
# Release, replacing assets of the same name.
#
#   release-upload.sh RELEASE FILE...
#
# The files are uploaded one at a time in the order given, each under its
# base name, so the caller puts the AppImage and its checksum last and a
# failed run never leaves the image on a Release without its sources.
# RELEASE is a vX.Y.Z tag, whose Release must exist (release-please makes it;
# it is never created here), or `dry-run`, the draft a dry run fills: it is
# created when missing, refused when published, and keeps exactly this
# run's assets. Uses gh with GH_TOKEN and GH_REPO from the environment, and
# GITHUB_SHA (plus GITHUB_REF_NAME, GITHUB_SERVER_URL, GITHUB_REPOSITORY and
# GITHUB_RUN_ID for the draft's notes) as Actions sets them.
# scripts/release-tests.sh tests it with a stub gh.
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

if [[ $# -lt 2 ]]; then
    echo "usage: $0 RELEASE FILE..." >&2
    exit 2
fi
release=$1
shift
files=("$@")

if [[ $release != dry-run ]] && ! "$root/scripts/release-ref.sh" "$release"; then
    echo "::error::$release is neither a vX.Y.Z tag nor dry-run: nothing is uploaded."
    exit 1
fi
declare -A keep=()
for file in "${files[@]}"; do
    if [[ ! -f $file ]]; then
        echo "::error::$file does not exist: nothing is uploaded."
        exit 1
    fi
    base=$(basename -- "$file")
    if [[ -n ${keep[$base]:-} ]]; then
        echo "::error::Two files named $base: nothing is uploaded."
        exit 1
    fi
    keep[$base]=1
done

err=$(mktemp)
trap 'rm -f -- "$err"' EXIT

# Find the release. A tag's Release is made by release-please and is never
# created here. The draft `dry-run` is created when missing; a published
# release of that name is refused.
if draft=$(gh release view "$release" --json isDraft --jq .isDraft 2> "$err"); then
    if [[ $release == dry-run && $draft != true ]]; then
        echo "::error::The release dry-run is published, not a draft: nothing is uploaded."
        exit 1
    fi
elif grep -q 'release not found' "$err"; then
    if [[ $release != dry-run ]]; then
        echo "::error::There is no GitHub Release for the tag $release: nothing is uploaded or created."
        exit 1
    fi
    gh release create dry-run --draft --latest=false --target "${GITHUB_SHA:?}" \
        --title "Dry run" --notes "Created by a dry run of the release workflow."
else
    cat "$err" >&2
    exit 1
fi

# One call per file: gh uploads the files of one call concurrently, in no
# fixed order.
for file in "${files[@]}"; do
    gh release upload "$release" --clobber "$file"
done

# After the upload, so a failed upload deletes nothing from the draft: the
# draft keeps exactly this run's assets, and a dry run of another version
# leaves no older asset beside them.
if [[ $release == dry-run ]]; then
    names=$(gh release view dry-run --json assets --jq '.assets[].name')
    while IFS= read -r name; do
        if [[ -n $name && -z ${keep[$name]:-} ]]; then
            gh release delete-asset dry-run "$name" --yes
        fi
    done <<< "$names"
    gh release edit dry-run --target "${GITHUB_SHA:?}" \
        --notes "Dry run of ${GITHUB_REF_NAME:-?} at $GITHUB_SHA, run ${GITHUB_SERVER_URL:-}/${GITHUB_REPOSITORY:-}/actions/runs/${GITHUB_RUN_ID:-}."
fi
