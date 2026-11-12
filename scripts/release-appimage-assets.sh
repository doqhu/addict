#!/usr/bin/env bash
# The AppImage part of the release upload: turns the checked handoff of the
# CI job `appimage` into release assets, checking it again first.
#
#   release-appimage-assets.sh HANDOFF_DIR OUT_DIR
#
# HANDOFF_DIR holds the downloaded artifact: exactly one
# Addict-<version>-x86_64-handoff.tar and its .tar.sha256, as
# scripts/archive-appimage.sh writes them. OUT_DIR must not exist; it is
# created and receives, with NAME = Addict-<version>-x86_64.AppImage:
#
#   NAME, NAME.sha256                    the image and its checksum
#   NAME.build.txt, NAME.inputs.lock.json, NAME.rpms.txt, NAME.layout.txt,
#   NAME.notices.json, NAME.components.sha256        the metadata
#   NAME.sources.tar.00, .01, ...        one tar of NAME.sources/, split
#   NAME.sources.tar.sha256              the checksums of those parts
#
# Before writing any asset it checks the tar against its checksum, the image
# against its .sha256, every source against SHA256SUMS, and the source names
# against the inventory in NAME.notices.json. Every asset must be under
# 2 GiB with a name of only A-Za-z0-9._-, so GitHub renames none. Any failure
# exits non-zero and removes OUT_DIR, so nothing can be uploaded. To keep
# the disk peak low it deletes the handoff tar once extracted, and the
# extracted tree once the assets are written.
#
# For tests: ASSET_LIMIT (bytes, default 2147483648) is the size every asset
# must stay under, PART_SIZE (bytes, default 1.9 GiB) the size of a part.
# scripts/release-tests.sh tests it; docs/release.md describes the assets.
set -Eeuo pipefail

if [[ $# != 2 ]]; then
    echo "usage: $0 HANDOFF_DIR OUT_DIR" >&2
    exit 2
fi
handoff=$1
out=$(realpath -m -- "$2")
limit=${ASSET_LIMIT:-2147483648}
part_size=${PART_SIZE:-2040109465}

die() { echo "::error::$*" >&2; exit 1; }

[[ -d $handoff ]] || die "$handoff is not a directory: nothing is published."
[[ ! -e $out ]] || die "$out already exists: nothing is published."

shopt -s nullglob
tars=("$handoff"/Addict-*-x86_64-handoff.tar)
shopt -u nullglob
[[ ${#tars[@]} == 1 ]] || die "$handoff holds ${#tars[@]} handoff tars, not one: nothing is published."
tar_file=${tars[0]}
tar_name=$(basename -- "$tar_file")
version=${tar_name#Addict-}
version=${version%-x86_64-handoff.tar}
[[ $version =~ ^[A-Za-z0-9._-]+$ ]] || die "$tar_name has no usable version: nothing is published."
name=Addict-$version-x86_64.AppImage
metadata=(build.txt inputs.lock.json rpms.txt layout.txt notices.json components.sha256)

mkdir -p -- "$(dirname -- "$out")"
work=$(mktemp -d -- "$(dirname -- "$out")/.appimage-assets.XXXXXX")
mkdir -- "$out"
done_ok=false
cleanup() {
    rm -rf -- "$work"
    if [[ $done_ok != true ]]; then rm -rf -- "$out"; fi
}
trap cleanup EXIT

# The handoff tar against its checksum.
[[ -f $tar_file.sha256 ]] || die "$tar_name.sha256 is missing: nothing is published."
(cd -- "$handoff" && sha256sum --check --strict --quiet -- "$tar_name.sha256") \
    || die "$tar_name does not match $tar_name.sha256: nothing is published."

# GNU tar refuses members with `..` and strips a leading `/`; the tree is
# checked member by member below.
tar --no-same-owner --no-same-permissions -C "$work" -xf "$tar_file"
rm -f -- "$tar_file"

# The tree, the image, the sources and the inventory. Each failure names the
# file it concerns.
python3 - "$work" "$name" "${metadata[@]}" <<'PY'
import hashlib, json, pathlib, re, sys

work, name, metadata = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3:]

def fail(message):
    raise SystemExit('::error::' + message + ': nothing is published.')

def digest(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()

files = [name, name + '.sha256'] + [name + '.' + m for m in metadata]
expected = set(files) | {name + '.sources'}
found = {p.name for p in work.iterdir()}
for extra in sorted(found - expected):
    fail('unexpected entry in the handoff: ' + extra)
for missing in sorted(expected - found):
    fail('missing from the handoff: ' + missing)
for f in files:
    path = work / f
    if path.is_symlink() or not path.is_file() or not path.stat().st_size:
        fail('not a non-empty regular file in the handoff: ' + f)

if (work / (name + '.sha256')).read_text() != digest(work / name) + '  ' + name + '\n':
    fail('the image does not match its checksum: ' + name)

try:
    inventory = json.loads((work / (name + '.notices.json')).read_text())
    sources = {}
    for component in inventory['components'].values():
        for item in component['source'].get('files', []):
            if sources.setdefault(item['file'], item['sha256']) != item['sha256']:
                fail('two checksums in the inventory for ' + item['file'])
except (ValueError, KeyError, TypeError, AttributeError) as e:
    fail('unreadable inventory in ' + name + '.notices.json (' + repr(e) + ')')
if not sources:
    fail('the inventory in ' + name + '.notices.json lists no source')

source_dir = work / (name + '.sources')
if source_dir.is_symlink() or not source_dir.is_dir():
    fail('not a directory in the handoff: ' + source_dir.name)
present = {}
for p in source_dir.iterdir():
    if p.is_symlink() or not p.is_file():
        fail('not a regular file in the source set: ' + p.name)
    present[p.name] = p
sums_path = present.pop('SHA256SUMS', None)
if sums_path is None:
    fail('missing from the source set: SHA256SUMS')
sums = {}
for line in sums_path.read_text().splitlines():
    m = re.fullmatch(r'([0-9a-f]{64})  ([^/]+)', line)
    if not m or m.group(2) in sums:
        fail('malformed line in SHA256SUMS: ' + line)
    sums[m.group(2)] = m.group(1)

for f in sorted(set(sums) - set(present)):
    fail('missing corresponding source: ' + f)
for f in sorted(set(present) - set(sums)):
    fail('corresponding source not in SHA256SUMS: ' + f)
for f in sorted(set(sources) - set(sums)):
    fail('the source set differs from the inventory in ' + name
         + '.notices.json: missing ' + f)
for f in sorted(set(sums) - set(sources)):
    fail('the source set differs from the inventory in ' + name
         + '.notices.json: not in the inventory: ' + f)
for f in sorted(sums):
    if sums[f] != sources[f]:
        fail('SHA256SUMS differs from the inventory for ' + f)
    if digest(present[f]) != sums[f]:
        fail('corrupt corresponding source: ' + f)
print('Checked %s and %d corresponding sources.' % (name, len(sums)))
PY

# The source set: one tar, split into parts.
tar -C "$work" --sort=name --owner=0 --group=0 --numeric-owner --format=gnu \
    -cf - -- "$name.sources" \
    | split --bytes="$part_size" --numeric-suffixes=0 --suffix-length=2 - "$out/$name.sources.tar."
(cd -- "$out" && sha256sum -- "$name".sources.tar.[0-9][0-9] > "$name.sources.tar.sha256")

for f in "$name" "$name.sha256" "${metadata[@]/#/$name.}"; do
    mv -- "$work/$f" "$out/$f"
done
rm -rf -- "$work"

# Every asset: its name and its size.
for asset in "$out"/*; do
    base=$(basename -- "$asset")
    [[ $base =~ ^[A-Za-z0-9._-]+$ ]] || die "asset name with a character GitHub would change: $base"
    size=$(stat -c %s -- "$asset")
    ((size < limit)) || die "asset of $size bytes, not under $limit: $base"
done

done_ok=true
echo "AppImage assets in $out:"
(cd -- "$out" && ls -l)
