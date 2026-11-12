#!/usr/bin/env bash
# The guard of the release workflow: exits 0 only when its argument is the name
# of an app release tag, vX.Y.Z with three numbers. It repeats the tag filter
# of .github/workflows/release.yml as a regular expression, because a filter
# cannot be run outside Actions; scripts/release-tests.sh checks both.
set -euo pipefail

if [[ $# != 1 ]]; then
    echo "usage: $0 REF_NAME" >&2
    exit 2
fi

if [[ $1 =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    exit 0
fi

printf '%s: %q is not a release tag of the form vX.Y.Z\n' "$0" "$1" >&2
exit 1
