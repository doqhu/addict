#!/usr/bin/env bash
# A fresh source/build directory and a verified, immutable container input set.
set -Eeuo pipefail
step=arguments
trap 'echo "AppImage: failed step: $step (line $LINENO)" >&2' ERR
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
lock=$root/packaging/appimage/inputs.lock.json
verify_inputs() {
    python3 - "$lock" "$1" "${2:-verify}" <<'PY'
import hashlib, json, pathlib, sys, urllib.request
lock, directory, mode = json.load(open(sys.argv[1])), pathlib.Path(sys.argv[2]), sys.argv[3]
directory.mkdir(parents=True, exist_ok=True)
lines = []
for item in lock['rpms'] + lock['tools'] + lock['ocr'] + lock['notices'] + lock['srpms'] + lock['static']:
    name, expected = item['file'], item['sha256']
    path = directory / name
    if not path.exists() and mode == 'fetch':
        print('Fetching ' + name, flush=True)
        try:
            with urllib.request.urlopen(item['url'], timeout=120) as src, open(str(path)+'.part', 'wb') as dst:
                import shutil
                shutil.copyfileobj(src, dst)
            pathlib.Path(str(path)+'.part').replace(path)
        except Exception as e:
            raise SystemExit(f'input {name}: {e}')
    if not path.is_file():
        raise SystemExit(f'missing input: {name}')
    actual = hashlib.file_digest(open(path, 'rb'), 'sha256').hexdigest()
    if actual != expected:
        raise SystemExit(f'checksum mismatch: {name}: {actual}, expected {expected}')
    lines.append(f'{expected}  {name}\n')
(directory / 'SHA256SUMS').write_text(''.join(lines))
PY
}
if [[ ${1:-} == --verify-arch ]]; then
    [[ ${2:-} == x86_64 ]] || { echo "unsupported architecture: ${2:-missing}" >&2; exit 1; }
    exit 0
fi
if [[ ${1:-} == --verify-inputs ]]; then
    verify_inputs "${2:?input directory required}"
    exit 0
fi
if [[ ${1:-} != --inside ]]; then
    [[ $# == 0 ]] || { echo "usage: $0 [--verify-inputs DIR | --verify-arch ARCH]" >&2; exit 1; }
    out=$root/target/appimage
    mkdir -p -- "$out"
    # No old artifact or success metadata survives a failed attempt, from the first check on.
    find "$out" -maxdepth 1 \( -type f -o -type l \) -delete
    find "$out" -mindepth 1 -maxdepth 1 -type d -name '*.sources*' -exec rm -rf -- {} +
    step=architecture
    "$0" --verify-arch "$(uname -m)"
    step=prerequisites
    command -v podman >/dev/null
    work=$(mktemp -d)
    trap 'if [[ ${ADDICT_APPIMAGE_KEEP_WORK:-0} == 1 ]]; then echo "Build workspace retained: $work"; else rm -rf -- "$work"; fi' EXIT
    step=inputs
    cache=$root/target/appimage-inputs
    verify_inputs "$cache" fetch
    mkdir -p "$work/context/inputs" "$work/srpms" "$work/static" "$work/source" "$work/result"
    # Source RPMs and the static closures' sources are mounted at run time, not
    # baked into the image: it installs every /inputs/*.rpm, and they are large.
    declare -A static_input=()
    while read -r file; do static_input[$file]=1; done < <(python3 -c 'import json, sys; print("\n".join(i["file"] for i in json.load(open(sys.argv[1]))["static"]))' "$lock")
    while read -r checksum file; do
        if [[ $file == *.src.rpm ]]; then
            cp -- "$cache/$file" "$work/srpms/"
            echo "$checksum  $file" >> "$work/srpms/SHA256SUMS"
        elif [[ -n ${static_input[$file]:-} ]]; then
            cp -- "$cache/$file" "$work/static/"
        else
            cp -- "$cache/$file" "$work/context/inputs/"
            echo "$checksum  $file" >> "$work/context/inputs/SHA256SUMS"
        fi
    done < "$cache/SHA256SUMS"
    cp -- "$root/packaging/appimage/Containerfile" "$work/context/Containerfile"
    step=source-snapshot
    # Includes new source files for local development; never copies target or indexes.
    # Tracked files deleted from the working tree are left out, as they are of the build.
    git -C "$root" ls-files --cached --others --exclude-standard -z \
        | (cd "$root" && while IFS= read -r -d '' f; do if [[ -e $f || -L $f ]]; then printf '%s\0' "$f"; fi; done) \
        > "$work/files"
    tar -C "$root" --null -T "$work/files" -cf "$work/source.tar"
    tar -C "$work/source" -xf "$work/source.tar"
    git -C "$root" rev-parse HEAD > "$work/source/source-revision.txt"
    sha256sum "$work/source.tar" | cut -d' ' -f1 > "$work/source/source-snapshot.sha256"
    git -C "$root" status --porcelain > "$work/source/source-status.txt"
    step=container-build
    image=localhost/addict-appimage:$(sha256sum "$lock" | cut -c1-16)
    podman build --pull=missing --no-cache -t "$image" "$work/context"
    step=release-and-package
    podman run --rm -v "$work/source:/build:Z" -v "$work/result:/out:Z" -v "$work/srpms:/srpms:ro,Z" -v "$work/static:/static:ro,Z" "$image"
    step=artifact-validation
    artifact=$(find "$work/result" -maxdepth 1 -name '*.AppImage' -print -quit)
    "$root/scripts/check-appimage.sh" "${artifact:?no artifact produced}"
    step=corresponding-source
    # Every pinned source the inventory names, beside (not inside) the AppImage.
    # Assembled under a temporary name and published last, after the artifact.
    sources=$out/${artifact##*/}.sources
    python3 - "$lock" "$artifact.notices.json" "$cache" "$sources.part" <<'PY'
import json, pathlib, shutil, sys
lock, inventory = json.load(open(sys.argv[1])), json.load(open(sys.argv[2]))
cache, out = pathlib.Path(sys.argv[3]), pathlib.Path(sys.argv[4])
pins = {i['file']: i for k in ('rpms', 'tools', 'ocr', 'notices', 'srpms', 'static') for i in lock[k]}
srpms = {i['file'] for i in lock['srpms']}
out.mkdir()
named = {}
for name, component in inventory['components'].items():
    for item in component['source'].get('files', []):
        if item['file'] not in pins or pins[item['file']] != item:
            raise SystemExit('unpinned source: ' + item['file'] + ' (component ' + name + ')')
        named[item['file']] = item
    if component['origin'] == 'fedora-rpm' and not any(i['file'] in srpms for i in component['source']['files']):
        raise SystemExit('unpinned source package for component ' + name)
for name in sorted(named):
    shutil.copyfile(cache / name, out / name)
(out / 'SHA256SUMS').write_text(''.join(named[n]['sha256'] + '  ' + n + '\n' for n in sorted(named)))
print('Corresponding source: ' + str(len(named)) + ' files')
PY
    (cd "$sources.part" && sha256sum --check --strict --quiet SHA256SUMS)
    step=publish
    cp -- "$work/result/"* "$out/"
    mv -- "$sources.part" "$sources"
    echo "AppImage ready: $out/${artifact##*/}"
    exit 0
fi
step=release-build
cd "$root"
export CARGO_HOME=/build/.cargo CARGO_TARGET_DIR=/build/target
cargo build --release --locked -p addict-app --bin addict
version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml","rb"))["workspace"]["package"]["version"])')
name=Addict-$version-x86_64.AppImage
step=staging
export APPDIR=/build/AppDir TMPDIR=/build/tool-tmp LIB_DIR=/usr/lib64 GTK_DIR=gtk-4.0
mkdir -p "$TMPDIR" "$APPDIR/share/applications" "$APPDIR/share/metainfo" "$APPDIR/share/licenses/addict"
cp /inputs/appimagetool "$TMPDIR/appimagetool"
chmod +x "$TMPDIR/appimagetool"
install -Dm644 data/io.github.doqhu.Addict.desktop "$APPDIR/share/applications/io.github.doqhu.Addict.desktop"
install -Dm644 data/io.github.doqhu.Addict.metainfo.xml "$APPDIR/share/metainfo/io.github.doqhu.Addict.metainfo.xml"
cp LICENSE CREDITS packaging/licenses/crate-licenses.txt "$APPDIR/share/licenses/addict/"
for size in 64 128 256 512; do
    install -Dm644 "crates/addict-app/icons/$size.png" "$APPDIR/share/icons/hicolor/${size}x${size}/apps/io.github.doqhu.Addict.png"
done
# Make quick-sharun use the preverified local downloads, not a second URL lookup.
export SHARUN_LINK=file:///inputs/sharun.tar CROSS_LIBC_DLOPEN_LINK=file:///inputs/cross-libc-dlopen.tar
export DESKTOP=$root/data/io.github.doqhu.Addict.desktop ICON=$root/crates/addict-app/icons/256.png
# Keep Fedora's checksummed Mesa/LLVM closure, including software rendering.
export DEPLOY_GTK=1 DEPLOY_GLIBC=1 DEPLOY_GDK=1 STRACE_MODE=0 DEPLOY_OPENGL=1 DEPLOY_VULKAN=1 DEPLOY_VULKAN_ALL=1 I_WANT_FAT_APPIMAGE=1
step=deploy-gtk-libc
bash /inputs/quick-sharun.sh "$CARGO_TARGET_DIR/release/addict" /usr/bin/tesseract
# quick-sharun may copy host tessdata: replace it with the exact shared pins.
rm -rf "$APPDIR/share/tessdata" "$APPDIR/share/tesseract"
mkdir -p "$APPDIR/share/tessdata" "$APPDIR/share/licenses/ocr"
install -m644 /inputs/{eng,spa,cat}.traineddata "$APPDIR/share/tessdata/"
install -m644 /inputs/tessdata-LICENSE "$APPDIR/share/licenses/ocr/"
# Preserve RPM-provided upstream engine and Leptonica notices.
cp -a /usr/share/licenses/{tesseract,leptonica,giflib} "$APPDIR/share/licenses/ocr/"
cp -a /usr/share/licenses/tesseract-tessdata* "$APPDIR/share/licenses/ocr/"
# Preserve the application's native/Flatpak shared cache path as well as data/config.
sed -i '/^set -e$/a export USE_HOST_XDG_CACHE_HOME=1' "$APPDIR/AppRun.sh"
# Explicit fonts and icon resources; ELF dependencies/modules belong to quick-sharun.
cp -a /usr/share/fonts "$APPDIR/share/"
cp -aL /etc/fonts "$APPDIR/etc/"
cp -a /usr/share/icons/Adwaita "$APPDIR/share/icons/"
# Retain a content manifest of the deployed OCR binary/library closure. This
# detects corruption before success metadata, including missing transitive libs.
python3 - "$APPDIR" <<'PY_OCR'
import hashlib, pathlib, subprocess, sys
root = pathlib.Path(sys.argv[1])
seen, required_paths = set(), set()
def visit(path):
    # Keep the SONAME pathname the loader opens as well as its final contents.
    required_paths.add(path)
    path = path.resolve()
    if path in seen: return
    seen.add(path)
    result = subprocess.run(['readelf', '-d', str(path)], check=True, capture_output=True, text=True)
    import re
    for name in re.findall(r'\(NEEDED\).*?\[(.*?)\]', result.stdout):
        lib = root/'lib'/name
        if not lib.is_file(): raise SystemExit('missing OCR dependency: '+name)
        visit(lib)
visit(root/'shared/bin/tesseract')
seen.update((root/'bin/tesseract', root/'share/licenses/ocr/tessdata-LICENSE'))
manifest = root/'share/ocr-components.sha256'
manifest.write_text(''.join(hashlib.file_digest(p.open('rb'), 'sha256').hexdigest()+'  '+str(p.relative_to(root))+'\n' for p in sorted(seen | required_paths)))
PY_OCR
step=notices
# Every AppDir entry and the runtime header belong to a component with notices and a source record.
python3 packaging/appimage/notices.py "$APPDIR" packaging/appimage/inputs.lock.json /inputs /srpms /static /build
step=validate-appdir
scripts/check-appimage.sh --appdir "$APPDIR" > /build/layout.txt
step=make-appimage
export OUTPATH=/build/package OUTNAME=$name APPIMAGE_ARCH=x86_64
mkdir -p "$OUTPATH"
"$TMPDIR/appimagetool" "$APPDIR"
# uruntime 0.8.1: 2 means automatic extraction fallback without the 350 MiB cap.
python3 - "$OUTPATH/$name" <<'PY'
import pathlib, sys
p = pathlib.Path(sys.argv[1]); b = p.read_bytes()
assert b.count(b'URUNTIME_EXTRACT=3') == 1, 'unexpected pinned runtime extraction setting'
p.write_bytes(b.replace(b'URUNTIME_EXTRACT=3', b'URUNTIME_EXTRACT=2'))
PY
chmod +x "$OUTPATH/$name"
step=inventory
cp packaging/appimage/inputs.lock.json "$OUTPATH/$name.inputs.lock.json"
cp /inputs/installed-rpms.txt "$OUTPATH/$name.rpms.txt"
cp /build/layout.txt "$OUTPATH/$name.layout.txt"
cp "$APPDIR/share/licenses/components.json" "$OUTPATH/$name.notices.json"
{ cat source-revision.txt source-snapshot.sha256 source-status.txt; rustc -Vv; cargo -V; sha256sum Cargo.lock packaging/appimage/Containerfile; } > "$OUTPATH/$name.build.txt"
# Full per-file content inventory; share/licenses/components.json attributes each file.
(cd "$APPDIR" && find . -type f -print0 | sort -z | xargs -0 sha256sum) > "$OUTPATH/$name.components.sha256"
(cd "$OUTPATH" && sha256sum "$name" > "$name.sha256")
step=publish-local-outputs
cp "$OUTPATH/"* /out/
