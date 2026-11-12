#!/usr/bin/env bash
# The upload of the release workflow: puts the Flatpak bundle and its SHA-256
# file on a GitHub Release, replacing assets of the same name.
#
#   release-upload.sh RELEASE BUNDLE CHECKSUM
#
# RELEASE is a vX.Y.Z tag, whose Release must exist (release-please makes it;
# it is never created here), or `dry-run`, the draft a dry run fills: it is
# created when missing, refused when published, and keeps only this run's
# two assets. Uses gh with GH_TOKEN and GH_REPO from the environment, and
# GITHUB_SHA (plus GITHUB_REF_NAME, GITHUB_SERVER_URL, GITHUB_REPOSITORY and
# GITHUB_RUN_ID for the draft's notes) as Actions sets them.
# scripts/release-tests.sh tests it with a stub gh.
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

if [[ $# != 3 ]]; then
    echo "usage: $0 RELEASE BUNDLE CHECKSUM" >&2
    exit 2
fi
release=$1 bundle=$2 checksum=$3

if [[ $release != dry-run ]] && ! "$root/scripts/release-ref.sh" "$release"; then
    echo "::error::$release is neither a vX.Y.Z tag nor dry-run: nothing is uploaded."
    exit 1
fi
for file in "$bundle" "$checksum"; do
    if [[ ! -f $file ]]; then
        echo "::error::$file does not exist: nothing is uploaded."
        exit 1
    fi
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

gh release upload "$release" --clobber "$bundle" "$checksum"

# After the upload, so a failed upload leaves the draft as it was: the draft
# keeps only this run's bundle, and a dry run of another version leaves no
# older bundle beside it.
if [[ $release == dry-run ]]; then
    names=$(gh release view dry-run --json assets --jq '.assets[].name')
    while IFS= read -r name; do
        if [[ -n $name && $name != "$(basename -- "$bundle")" && $name != "$(basename -- "$checksum")" ]]; then
            gh release delete-asset dry-run "$name" --yes
        fi
    done <<< "$names"
    gh release edit dry-run --target "${GITHUB_SHA:?}" \
        --notes "Dry run of ${GITHUB_REF_NAME:-?} at $GITHUB_SHA, run ${GITHUB_SERVER_URL:-}/${GITHUB_REPOSITORY:-}/actions/runs/${GITHUB_RUN_ID:-}."
fi
