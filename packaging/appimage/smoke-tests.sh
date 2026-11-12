#!/usr/bin/env bash
# Focused launcher failures; no container engine or host display is used.
set -Eeuo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
mkdir "$work/bin"
cat > "$work/bin/podman" <<'STUB'
#!/usr/bin/env bash
case $1 in
    build) exit "${BUILD_STATUS:-0}" ;;
    run) exit "${RUN_STATUS:-0}" ;;
    rm) exit 0 ;;
esac
exit 1
STUB
chmod +x "$work/bin/podman"
export PATH="$work/bin:$PATH"
refuse() {
    local expected=$1 status; shift
    if "$@" > "$work/log" 2>&1; then echo "unexpected success: $*" >&2; exit 1; else status=$?; fi
    [[ $status == "$expected" ]] || { cat "$work/log" >&2; echo "exit $status, expected $expected" >&2; exit 1; }
}
refuse 1 "$root/scripts/check-appimage-smoke.sh" "$work/missing" "$work/no-image"
[[ ! -e $work/no-image ]]
touch "$work/image"; chmod +x "$work/image"
BUILD_STATUS=23 refuse 23 "$root/scripts/check-appimage-smoke.sh" "$work/image" "$work/build-failure"
RUN_STATUS=24 refuse 24 "$root/scripts/check-appimage-smoke.sh" "$work/image" "$work/run-failure"
refuse 1 "$root/scripts/check-appimage-smoke.sh" "$work/image" "$work/missing-pass"
grep -q 'missing OCR success evidence' "$work/log"
refuse 1 "$root/scripts/check-appimage-smoke.sh" "$work/image" "$work/missing-pass"
grep -q 'diagnostics directory already exists' "$work/log"
refuse 1 "$root/scripts/archive-appimage.sh" "$work/run-failure" "$work/handoff"
[[ ! -e $work/handoff ]]
[[ ! -e $work/build-failure/ocr/PASS.json && ! -e $work/run-failure/ocr/PASS.json ]]
# A tiny synthetic handoff reaches source guards before package execution.
mkdir -p "$work/fixture/scripts" "$work/fixture/packaging/appimage"
cp "$root/scripts/archive-appimage.sh" "$work/fixture/scripts/"
python3 - "$work/fixture" <<'PY'
import hashlib, json, pathlib, sys
root = pathlib.Path(sys.argv[1]); out = root/'target/appimage'; out.mkdir(parents=True)
(root/'Cargo.toml').write_text('[workspace.package]\nversion = "1.0.0"\n')
name = 'Addict-1.0.0-x86_64.AppImage'
for suffix in ('', '.build.txt', '.rpms.txt', '.layout.txt', '.components.sha256'):
    (out/(name+suffix)).write_text('fixture\n')
digest = hashlib.sha256((out/name).read_bytes()).hexdigest()
(out/(name+'.sha256')).write_text(digest+'  '+name+'\n')
item = {'file': 'source.tar', 'sha256': hashlib.sha256(b'source').hexdigest(), 'url': 'https://invalid.test/source.tar'}
lock = {k: [] for k in ('rpms', 'tools', 'ocr', 'notices', 'srpms', 'static')}; lock['notices'] = [item]
for path in (root/'packaging/appimage/inputs.lock.json', out/(name+'.inputs.lock.json')):
    path.write_text(json.dumps(lock))
(out/(name+'.notices.json')).write_text(json.dumps({'components': {'fixture': {'source': {'files': [item]}}}}))
sources = out/(name+'.sources'); sources.mkdir(); (sources/'source.tar').write_bytes(b'source')
(sources/'SHA256SUMS').write_text(item['sha256']+'  source.tar\n')
smoke = root/'smoke/ocr'; smoke.mkdir(parents=True)
(smoke/'image.sha256').write_text(digest+'\n')
(smoke/'PASS.json').write_text(json.dumps({'eng': 'The bank is beside the river.', 'spa': 'El niño está cerca del río.', 'cat': 'El camí és a prop del riu.'}))
PY
fixture=$work/fixture
sources=$fixture/target/appimage/Addict-1.0.0-x86_64.AppImage.sources
mv "$sources/source.tar" "$work/source.saved"
refuse 1 "$fixture/scripts/archive-appimage.sh" "$fixture/smoke" "$work/incomplete-handoff"
grep -q 'incomplete corresponding-source set' "$work/log"
mv "$sources/SHA256SUMS" "$work/sums.saved"; touch "$sources/SHA256SUMS"
refuse 1 "$fixture/scripts/archive-appimage.sh" "$fixture/smoke" "$work/deleted-checksum-handoff"
grep -q 'incomplete corresponding-source set' "$work/log"
mv "$work/source.saved" "$sources/source.tar"; mv "$work/sums.saved" "$sources/SHA256SUMS"
echo corrupt > "$sources/source.tar"
refuse 1 "$fixture/scripts/archive-appimage.sh" "$fixture/smoke" "$work/corrupt-handoff"
grep -q 'corrupt corresponding source' "$work/log"
[[ ! -e $work/incomplete-handoff && ! -e $work/deleted-checksum-handoff && ! -e $work/corrupt-handoff ]]
# Reach the real checksum guard with a tiny fake extraction and package audit.
printf source > "$sources/source.tar"
artifact=$fixture/target/appimage/Addict-1.0.0-x86_64.AppImage
export FIXTURE_NOTICES=$artifact.notices.json
cat > "$artifact" <<'IMAGE'
#!/usr/bin/env bash
mkdir -p AppDir/share/licenses
cp "$FIXTURE_NOTICES" AppDir/share/licenses/components.json
printf 'icon\n' > AppDir/.DirIcon
IMAGE
printf '#!/bin/sh\nexit 0\n' > "$fixture/scripts/check-appimage.sh"
chmod +x "$artifact" "$fixture/scripts/check-appimage.sh"
(cd "$(dirname "$artifact")" && sha256sum "${artifact##*/}" > "$artifact.sha256")
cut -d' ' -f1 "$artifact.sha256" > "$fixture/smoke/ocr/image.sha256"
mkdir "$work/extracted"
(cd "$work/extracted" && "$artifact" --appimage-extract)
(cd "$work/extracted/AppDir" && find . -type f -print0 | sort -z | xargs -0 sha256sum) > "$artifact.components.sha256"
"$fixture/scripts/archive-appimage.sh" "$fixture/smoke" "$work/complete-handoff" > "$work/log" 2>&1
sed -i '\|  ./\.DirIcon$|d' "$artifact.components.sha256"
refuse 1 "$fixture/scripts/archive-appimage.sh" "$fixture/smoke" "$work/truncated-manifest-handoff"
grep -q 'incomplete or changed component checksum manifest' "$work/log"
[[ ! -e $work/truncated-manifest-handoff ]]
echo 'PASS: wrapper errors, source refusals and truncated component manifest refuse'
