#!/usr/bin/env bash
# Assemble the checked epic-6 handoff only after isolated OCR has succeeded.
set -Eeuo pipefail
[[ $# == 2 ]] || { echo "usage: $0 SMOKE_DIAGNOSTICS NEW_ARCHIVE_DIR" >&2; exit 1; }
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
version=$(python3 -c 'import sys,tomllib; print(tomllib.load(open(sys.argv[1],"rb"))["workspace"]["package"]["version"])' "$root/Cargo.toml")
name=Addict-$version-x86_64.AppImage
out=$(realpath -m -- "$2")
[[ ! -e $out ]] || { echo "archive directory already exists: $out" >&2; exit 1; }
# Compare source membership AND checksums to the checked inventory, so removing
# both a source and its checksum cannot produce an incomplete successful handoff.
python3 - "$root" "$name" "$1" <<'PY'
import hashlib, json, pathlib, sys
root, name, smoke = pathlib.Path(sys.argv[1]), sys.argv[2], pathlib.Path(sys.argv[3])
directory = root/'target/appimage'
image = directory/name
for suffix in ('', '.sha256', '.build.txt', '.inputs.lock.json', '.rpms.txt', '.layout.txt', '.notices.json', '.components.sha256'):
    path = directory/(name+suffix)
    if path.is_symlink() or not path.is_file() or not path.stat().st_size:
        raise SystemExit('missing required output: '+str(path))
digest = hashlib.file_digest(image.open('rb'), 'sha256').hexdigest()
if (directory/(name+'.sha256')).read_text() != digest+'  '+name+'\n':
    raise SystemExit('invalid image checksum or relative filename')
expected = {'eng': 'The bank is beside the river.', 'spa': 'El niño está cerca del río.', 'cat': 'El camí és a prop del riu.'}
if json.loads((smoke/'ocr/PASS.json').read_text()) != expected or (smoke/'ocr/image.sha256').read_text().strip() != digest:
    raise SystemExit('smoke evidence does not match this image')
lock = json.loads((root/'packaging/appimage/inputs.lock.json').read_text())
if json.loads((directory/(name+'.inputs.lock.json')).read_text()) != lock:
    raise SystemExit('artifact input lock differs from checkout')
pins = {i['file']: i for k in ('rpms', 'tools', 'ocr', 'notices', 'srpms', 'static') for i in lock[k]}
inventory = json.loads((directory/(name+'.notices.json')).read_text())
sources = {}
for component in inventory['components'].values():
    for item in component['source'].get('files', []):
        if pins.get(item['file']) != item: raise SystemExit('unpinned source: '+item['file'])
        sources[item['file']] = item['sha256']
source_dir = directory/(name+'.sources')
if source_dir.is_symlink() or {p.name for p in source_dir.iterdir()} != set(sources) | {'SHA256SUMS'} \
        or any(p.is_symlink() or not p.is_file() for p in source_dir.iterdir()):
    raise SystemExit('incomplete corresponding-source set')
manifest = ''.join(sources[n]+'  '+n+'\n' for n in sorted(sources))
if (source_dir/'SHA256SUMS').read_text() != manifest: raise SystemExit('invalid source checksum manifest')
for filename, expected_digest in sources.items():
    path = source_dir/filename
    if path.is_symlink() or hashlib.file_digest(path.open('rb'), 'sha256').hexdigest() != expected_digest:
        raise SystemExit('corrupt corresponding source: '+filename)
PY
"$root/scripts/check-appimage.sh" "$root/target/appimage/$name" >/dev/null
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
(cd "$work" && "$root/target/appimage/$name" --appimage-extract >/dev/null)
cmp "$root/target/appimage/$name.notices.json" "$work/AppDir/share/licenses/components.json"
(cd "$work/AppDir" && find . -type f -print0 | sort -z | xargs -0 sha256sum) > "$work/components.sha256"
cmp --silent "$root/target/appimage/$name.components.sha256" "$work/components.sha256" \
    || { echo 'incomplete or changed component checksum manifest' >&2; exit 1; }
mkdir -p -- "$out"
archive=Addict-$version-x86_64-handoff.tar
tar -C "$root/target/appimage" -cf "$out/$archive.part" -- "$name" "$name".{sha256,build.txt,inputs.lock.json,rpms.txt,layout.txt,notices.json,components.sha256,sources}
mv -- "$out/$archive.part" "$out/$archive"
(cd "$out" && sha256sum "$archive" > "$archive.sha256")
echo "Checked handoff: $out/$archive"
