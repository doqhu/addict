#!/usr/bin/env bash
# Refusal tests use disposable layout fixtures; these are never dictionary fixtures.
set -Eeuo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
if [[ ${1:-} == --ocr ]]; then
    [[ $# == 3 ]] || { echo "usage: $0 --ocr IMAGE NEW_EVIDENCE_DIR" >&2; exit 1; }
    exec python3 "$root/packaging/appimage/ocr.py" "$2" "$3"
fi
if [[ ${1:-} == --relaunch ]]; then
    [[ $# == 3 ]] || { echo "usage: $0 --relaunch IMAGE NEW_EVIDENCE_DIR" >&2; exit 1; }
    exec python3 "$root/packaging/appimage/relaunch.py" "$2" "$3"
fi
if [[ ${1:-} == --runtime ]]; then
    [[ $# == 3 ]] || { echo "usage: $0 --runtime IMAGE EVIDENCE_DIR" >&2; exit 1; }
    # This launches on the caller's display; target-distribution acceptance is manual.
    python3 - "$root" "$2" "$3" <<'RUNTIME'
import os, pathlib, subprocess, sys, time
root, image, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]).resolve(), pathlib.Path(sys.argv[3]).resolve()
if not os.environ.get('DISPLAY') and not os.environ.get('WAYLAND_DISPLAY'):
    raise SystemExit('missing desktop: DISPLAY or WAYLAND_DISPLAY required')
out.mkdir(parents=True, exist_ok=False)
env = os.environ.copy()
for k in ['APPIMAGE_EXTRACT_AND_RUN', 'APPIMAGE_TARGET_DIR', 'TARGET_APPIMAGE', 'DBUS_SESSION_BUS_ADDRESS', 'GTK_PATH', 'GTK_MODULES', 'LD_LIBRARY_PATH']:
    env.pop(k, None)
for key, directory in [('HOME','home'), ('XDG_DATA_HOME','data'), ('XDG_CONFIG_HOME','config'), ('XDG_CACHE_HOME','cache'), ('XDG_RUNTIME_DIR','runtime')]:
    p = out/directory; p.mkdir(mode=0o700); env[key] = str(p)
wayland = os.environ.get('WAYLAND_DISPLAY')
if wayland and not pathlib.Path(wayland).is_absolute():
    (out/'runtime'/wayland).symlink_to(pathlib.Path(os.environ['XDG_RUNTIME_DIR'])/wayland)
env['XAUTHORITY'] = os.environ.get('XAUTHORITY', str(pathlib.Path.home()/'.Xauthority'))
(out/'host.txt').write_text(pathlib.Path('/etc/os-release').read_text()+'\n/dev/fuse exists: '+str(pathlib.Path('/dev/fuse').exists())+'\n')
pid = bundle = None
with (out/'launch.log').open('w') as log:
    launch = subprocess.Popen(['dbus-run-session', '--', str(image)], env=env, stdout=log, stderr=log)
    try:
        for _ in range(30):
            for proc in pathlib.Path('/proc').glob('[0-9]*'):
                try:
                    if pathlib.Path(os.readlink(proc/'exe')).name == 'addict' and ('XDG_DATA_HOME='+env['XDG_DATA_HOME']).encode() in (proc/'environ').read_bytes().split(b'\0'):
                        pid = proc.name; break
                except (OSError, PermissionError):
                    pass
            if pid: break
            if launch.poll() is not None: raise SystemExit('runtime exited before resident launch; see '+str(out/'launch.log'))
            time.sleep(1)
        if not pid: raise SystemExit('runtime resident not found within 30 seconds')
        time.sleep(5)
        subprocess.run([str(root/'scripts/check-appimage.sh'), '--trace', pid, str(out/'process')], check=True)
        observed = dict(line.split('=', 1) for line in (out/'process/environment.txt').read_text().splitlines() if '=' in line)
        for key in ['HOME', 'XDG_DATA_HOME', 'XDG_CONFIG_HOME', 'XDG_CACHE_HOME']:
            if observed.get(key) != env[key]: raise SystemExit('application path changed: '+key+'='+str(observed.get(key)))
        maps = (out/'process/maps.txt').read_text()
        appdir = next((line.split('=', 1)[1] for line in (out/'process/environment.txt').read_text().splitlines() if line.startswith('APPDIR=')), None)
        if not appdir: raise SystemExit('missing APPDIR in the resident environment')
        bundle = pathlib.Path(appdir)
        for library in ['libgtk-4', 'libc.so.6', 'ld-linux-x86-64.so.2']:
            paths = [line.split()[-1] for line in maps.splitlines() if library in line]
            if not paths or any(not pathlib.Path(p).is_relative_to(bundle) for p in paths):
                raise SystemExit('unexpected mapped '+library+': '+repr(paths))
        print('PASS: isolated resident launch and mapped bundled GTK/libc; evidence: '+str(out))
        print('Visible-window, HTTPS download and typed-translation acceptance still require human observation.')
    finally:
        if pid:
            try: os.kill(int(pid), 15)
            except ProcessLookupError: pass
        # The runtime removes its extraction tree before it exits; that took 34 s on a desktop VM.
        try: launch.wait(timeout=90)
        except subprocess.TimeoutExpired: launch.terminate(); launch.wait(timeout=5)
        if bundle and bundle.exists(): print('WARNING: extraction tree left behind: '+str(bundle))
RUNTIME
    exit 0
fi
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
refuse() {
    local expected=$1; shift
    if "$@" > "$work/log" 2>&1; then echo "unexpected success: $*" >&2; exit 1; fi
    grep -q -- "$expected" "$work/log" || { cat "$work/log" >&2; exit 1; }
}
"$root/scripts/build-appimage.sh" --verify-arch x86_64
python3 - "$root" <<'PROVENANCE'
import copy, json, pathlib, runpy, sys
root = pathlib.Path(sys.argv[1]); package = root/'packaging/appimage'
check = runpy.run_path(str(package/'closures.py'))['check_sharun_provenance']
lock = json.loads((package/'inputs.lock.json').read_text())
components = json.loads((package/'static-closures.json').read_text())['components']
cache = root/'target/appimage-inputs'
check(lock, components, cache)
def refuse(lock, components, message):
    try: check(lock, components, cache)
    except SystemExit as error:
        assert message in str(error), str(error)
    else: raise AssertionError('unexpected sharun provenance success')
crate = next(k for k,c in components.items() if k.startswith('rust-crate-') and 'anylinux-sharun' in c['in'])
changed = copy.deepcopy(components); changed[crate]['version'] = '0.0.0'
refuse(lock, changed, 'crate versions mismatch')
missing = copy.deepcopy(components); missing[crate]['in'].remove('anylinux-sharun')
refuse(lock, missing, 'crate versions mismatch')
for name in ('rust-std-nightly-2026-09-11', 'rust-compiler-builtins-nightly-2026-09-11'):
    changed = copy.deepcopy(components); changed[name]['version'] = 'wrong-version'
    refuse(lock, changed, 'toolchain components mismatch')
    missing = copy.deepcopy(components); del missing[name]
    refuse(lock, missing, 'toolchain components mismatch')
changed = copy.deepcopy(components)
changed['rust-std-nightly-2026-09-11']['sources'] = ['wrong-source.tar.xz']
refuse(lock, changed, 'toolchain components mismatch')
changed = copy.deepcopy(lock)
next(i for i in changed['tools'] if i['file'] == 'sharun.tar')['sha256'] = '0'*64
refuse(changed, components, 'input mismatch: sharun.tar')
changed = copy.deepcopy(lock)
next(i for i in changed['notices'] if i['file'] == 'anylinux-sharun-source.tar.gz')['url'] = 'https://invalid.test/source'
refuse(changed, components, 'source revision mismatch')
print('PASS: sharun provenance; changed/missing crate/toolchain components, archive and source identity refuse')
PROVENANCE
refuse 'unsupported architecture: aarch64' "$root/scripts/build-appimage.sh" --verify-arch aarch64
mkdir "$work/inputs"
refuse 'missing input:' "$root/scripts/build-appimage.sh" --verify-inputs "$work/inputs"
first=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["rpms"][0]["file"])' "$root/packaging/appimage/inputs.lock.json")
echo corrupt > "$work/inputs/$first"
refuse 'checksum mismatch:' "$root/scripts/build-appimage.sh" --verify-inputs "$work/inputs"
# Every locked OCR download has named missing/corrupt refusals, after the
# preceding pinned inputs have been satisfied. Symlinks avoid copying RPMs.
python3 - "$root" "$work/ocr-inputs" <<'INPUTS'
import json, pathlib, sys
root, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]); out.mkdir()
lock=json.load(open(root/'packaging/appimage/inputs.lock.json'))
for item in lock['rpms']+lock['tools']+lock['ocr']+lock['notices']+lock['srpms']+lock['static']:
    (out/item['file']).symlink_to(root/'target/appimage-inputs'/item['file'])
INPUTS
for name in tessdata-LICENSE eng.traineddata spa.traineddata cat.traineddata; do
    mv "$work/ocr-inputs/$name" "$work/saved-link"
    refuse "missing input: $name" "$root/scripts/build-appimage.sh" --verify-inputs "$work/ocr-inputs"
    echo corrupt > "$work/ocr-inputs/$name"
    refuse "checksum mismatch: $name" "$root/scripts/build-appimage.sh" --verify-inputs "$work/ocr-inputs"
    rm "$work/ocr-inputs/$name"
    mv "$work/saved-link" "$work/ocr-inputs/$name"
done
python3 - "$work/AppDir" "$root/target/appimage-inputs" "$root/packaging/appimage/inputs.lock.json" "$root/packaging/appimage/static-closures.json" <<'PY'
import hashlib, pathlib, shutil, sys
r = pathlib.Path(sys.argv[1])
files = ['AppRun', 'AppRun.sh', 'sharun', 'dash', 'bin/addict', 'shared/bin/addict', 'bin/tesseract', 'shared/bin/tesseract',
         'lib/libtesseract.so.5', 'lib/libleptonica.so.6',
         'share/licenses/ocr/tesseract/LICENSE', 'share/licenses/ocr/leptonica/LICENSE',
         'share/licenses/ocr/tessdata-LICENSE',
         'lib/libgtk-4.so.1', 'lib/libGLESv2.so.2', 'lib/libvulkan_lvp.so',
         'share/vulkan/icd.d/lvp_icd.x86_64.json', 'lib/libc.so.6', 'lib/ld-linux-x86-64.so.2',
         'share/glib-2.0/schemas/gschemas.compiled', 'etc/fonts/fonts.conf',
         'share/fonts/test.ttf', 'share/icons/hicolor/256x256/apps/io.github.doqhu.Addict.png',
         'share/icons/Adwaita/index.theme', 'lib/gio/modules/module.so',
         'share/metainfo/io.github.doqhu.Addict.metainfo.xml', 'addict.desktop']
for name in files:
    p = r/name; p.parent.mkdir(parents=True, exist_ok=True); p.write_bytes(b'fixture')
for name in ['AppRun', 'AppRun.sh', 'sharun', 'dash', 'bin/addict', 'shared/bin/addict', 'bin/tesseract', 'shared/bin/tesseract']:
    (r/name).chmod(0o755)
for lang in ['eng','spa','cat']:
    target=r/'share/tessdata'/(lang+'.traineddata'); target.parent.mkdir(parents=True,exist_ok=True)
    shutil.copyfile(pathlib.Path(sys.argv[2])/(lang+'.traineddata'),target)
# Keep another library payload present when the required SONAME alias goes away.
(r/'lib/libtesseract.so.5.5.0').write_bytes(b'fixture')
(r/'lib/libtesseract.so.5.5').symlink_to('libtesseract.so.5.5.0')
entries=['lib/libtesseract.so.5.5','lib/libtesseract.so.5.5.0','bin/tesseract','shared/bin/tesseract','lib/libtesseract.so.5','lib/libleptonica.so.6','share/licenses/ocr/tessdata-LICENSE']
(r/'share/ocr-components.sha256').write_text(''.join(hashlib.sha256((r/name).read_bytes()).hexdigest()+'  '+name+'\n' for name in entries))
(r/'shared/bin/addict').write_bytes(b'\x7fELF\x02\x01'+b'\0'*12+b'\x3e\0')
# A notices inventory over every fixture entry: a project, a copyleft RPM and the
# runtime header with every library statically linked into it.
import json, os
lock = json.load(open(pathlib.Path(sys.argv[3])))
pins = {i['file']: i for k in ('notices', 'srpms', 'static') for i in lock[k]}
glibc = next(i for i in lock['srpms'] if i['file'].startswith('glibc-'))
components = {
    'addict': {'origin': 'project', 'license': 'MIT', 'license_files': ['share/licenses/addict/LICENSE', 'share/licenses/addict/crate-licenses.txt'], 'source': {'revision': 'fixture'}},
    'glibc': {'origin': 'fedora-rpm', 'license': 'LGPL-2.1-or-later', 'license_files': ['share/licenses/glibc/COPYING.LIB'], 'source': {'files': [glibc]}},
    'uruntime': {'origin': 'pinned-download', 'location': 'runtime header', 'license': 'MIT', 'license_files': ['share/licenses/uruntime/LICENSE'], 'source': {'files': [pins['uruntime-source.tar.gz']]}},
    'dwarfs': {'origin': 'pinned-download', 'location': 'runtime header', 'license': 'MIT', 'license_files': ['share/licenses/dwarfs/LICENSE'], 'source': {'files': [pins['dwarfs-0.15.7.tar.xz']]}},
    'anylinux-sharun': {'origin': 'pinned-download', 'license': 'MIT', 'license_files': ['share/licenses/anylinux-sharun/LICENSE'], 'source': {'files': [pins['anylinux-sharun-source.tar.gz']]}},
    'dash': {'origin': 'pinned-download', 'license': 'BSD-3-Clause AND GPL-2.0-or-later', 'license_files': ['share/licenses/dash/LICENSE'], 'source': {'files': [pins['dash-0.5.13.5.tar.gz']]}}}
# Every static closure: the runtime header's and those of sharun and dash.
for name, entry in json.load(open(sys.argv[4]))['components'].items():
    components[name] = {'origin': 'static', 'embedded_in': entry['in'], 'license': entry['license'],
                        'license_files': ['share/licenses/'+name+'/LICENSE'], 'source': {'files': [pins[f] for f in entry['sources']]}}
    if set(entry['in']) & {'uruntime', 'dwarfs'}:
        components[name]['location'] = 'runtime header'
for component in components.values():
    for name in component['license_files']:
        p = r/name; p.parent.mkdir(parents=True, exist_ok=True); p.write_bytes(b'fixture notice')
inventory = r/'share/licenses/components.json'
inventory.write_text('{}')
files = {}
for directory, dirs, names in os.walk(r):
    for name in names + [d for d in dirs if os.path.islink(os.path.join(directory, d))]:
        rel = os.path.relpath(os.path.join(directory, name), r)
        files[rel] = 'glibc' if rel.startswith('lib/libc.so') else 'anylinux-sharun' if rel == 'sharun' else 'dash' if rel == 'dash' else rel.split('/')[2] if rel.startswith('share/licenses/') and rel.count('/') >= 3 and rel.split('/')[2] in components else 'addict'
inventory.write_text(json.dumps({'schema': 1, 'components': components, 'files': files}))
PY
check=("$root/scripts/check-appimage.sh" --appdir "$work/AppDir")
"${check[@]}" > /dev/null
for path in bin/tesseract shared/bin/tesseract lib/libtesseract.so.5 lib/libleptonica.so.6 share/tessdata/eng.traineddata share/tessdata/spa.traineddata share/tessdata/cat.traineddata lib/libgtk-4.so.1 lib/libGLESv2.so.2 lib/libvulkan_lvp.so lib/libc.so.6 lib/ld-linux-x86-64.so.2 share/glib-2.0/schemas/gschemas.compiled etc/fonts/fonts.conf; do
    mv "$work/AppDir/$path" "$work/removed"
    refuse 'missing \(component\|OCR dependency\):' "${check[@]}"
    mv "$work/removed" "$work/AppDir/$path"
done
# A remaining payload (and other matching aliases) must not mask a missing
# SONAME link that the OCR engine's DT_NEEDED actually requires.
mv "$work/AppDir/lib/libtesseract.so.5.5" "$work/removed-soname"
refuse 'missing OCR dependency: lib/libtesseract.so.5.5' "${check[@]}"
mv "$work/removed-soname" "$work/AppDir/lib/libtesseract.so.5.5"
for path in shared/bin/tesseract lib/libtesseract.so.5 lib/libleptonica.so.6 share/tessdata/eng.traineddata share/tessdata/spa.traineddata share/tessdata/cat.traineddata; do
    cp "$work/AppDir/$path" "$work/saved"
    echo corrupt >> "$work/AppDir/$path"
    refuse 'corrupt OCR' "${check[@]}"
    cp "$work/saved" "$work/AppDir/$path"
done
chmod -x "$work/AppDir/bin/tesseract"
refuse 'not executable: bin/tesseract' "${check[@]}"
chmod +x "$work/AppDir/bin/tesseract"
for path in share/tessdata/fra.traineddata eng.traineddata share/eng.traineddata; do
    touch "$work/AppDir/$path"
    refuse 'unexpected OCR model:' "${check[@]}"
    rm "$work/AppDir/$path"
done
printf ADDICTIX > "$work/AppDir/hidden-data"
refuse 'contamination: hidden-data' "${check[@]}"
rm "$work/AppDir/hidden-data"
touch "$work/AppDir/accidental.index"
refuse 'contamination: accidental.index' "${check[@]}"
rm "$work/AppDir/accidental.index"
ln -s /etc/passwd "$work/AppDir/escape"
refuse 'escaping or broken symlink: escape' "${check[@]}"
rm "$work/AppDir/escape"
printf '\267' | dd of="$work/AppDir/shared/bin/addict" bs=1 seek=18 conv=notrunc status=none
refuse 'unsupported ELF architecture:' "${check[@]}"
printf '\076' | dd of="$work/AppDir/shared/bin/addict" bs=1 seek=18 conv=notrunc status=none
# Notices: every row of the 5.4 matrix has a named refusal.
inventory=$work/AppDir/share/licenses/components.json
cp "$inventory" "$work/inventory"
edit_inventory() { python3 -c 'import json,sys; p=sys.argv[1]; d=json.load(open(p)); exec(sys.argv[2]); open(p,"w").write(json.dumps(d))' "$inventory" "$1"; }
touch "$work/AppDir/share/stray"
refuse 'file not in notices inventory: share/stray' "${check[@]}"
rm "$work/AppDir/share/stray"
mv "$work/AppDir/share/licenses/glibc/COPYING.LIB" "$work/saved-notice"
refuse 'missing notice: glibc: share/licenses/glibc/COPYING.LIB' "${check[@]}"
: > "$work/AppDir/share/licenses/glibc/COPYING.LIB"
refuse 'missing notice: glibc: share/licenses/glibc/COPYING.LIB' "${check[@]}"
mv "$work/saved-notice" "$work/AppDir/share/licenses/glibc/COPYING.LIB"
edit_inventory 'd["components"]["glibc"]["source"]={}'
refuse 'copyleft component without source: glibc' "${check[@]}"
cp "$work/inventory" "$inventory"
edit_inventory 'd["components"]["glibc"]["source"]["files"][0]["sha256"]="0"*64'
refuse 'unpinned source record: glibc' "${check[@]}"
cp "$work/inventory" "$inventory"
edit_inventory 'd["components"]["glibc"]["source"]["files"]=[next(i for i in json.load(open("'"$root"'/packaging/appimage/inputs.lock.json"))["rpms"] if i["file"].startswith("glibc-"))]'
refuse 'fedora component without source RPM: glibc' "${check[@]}"
cp "$work/inventory" "$inventory"
# The runtime header: uruntime, the DwarFS helper and each library linked into them.
for name in uruntime dwarfs static-libfuse3-3.18.2 rust-crate-procfs-core-0.18.0; do
    edit_inventory 'n="'"$name"'"; del d["components"][n]; d["files"]={k:("addict" if v==n else v) for k,v in d["files"].items()}'
    refuse "missing notice: runtime header ($name)" "${check[@]}"
    cp "$work/inventory" "$inventory"
done
# A tool in the AppDir brings its static closure: sharun without its crates is refused.
edit_inventory 'n="rust-crate-goblin-0.8.2"; del d["components"][n]; d["files"]={k:("addict" if v==n else v) for k,v in d["files"].items()}'
refuse 'missing notice: static closure of anylinux-sharun (rust-crate-goblin-0.8.2)' "${check[@]}"
cp "$work/inventory" "$inventory"
# A tool binary attributed elsewhere would skip its closure: refused, naming the path.
edit_inventory 'd["files"]["sharun"]="addict"'
refuse 'tool binary not attributed to anylinux-sharun: sharun (addict)' "${check[@]}"
cp "$work/inventory" "$inventory"
edit_inventory 'd["files"]["share/gone"]="addict"'
refuse 'notices inventory names a missing file: share/gone' "${check[@]}"
cp "$work/inventory" "$inventory"
"${check[@]}" > /dev/null
# Build-time attribution names a file that no RPM, tool or rule accounts for.
# A fixture owner map stands in for the RPM database: no host rpm is needed.
mkdir -p "$work/attribution/share/licenses/addict" "$work/attribution/lib"
cp "$work/AppDir/sharun" "$work/attribution/"; cp "$work/AppDir/sharun" "$work/attribution/AppRun"; touch "$work/attribution/.DirIcon"
touch "$work/attribution/share/licenses/addict/LICENSE" "$work/attribution/lib/libfixture.so.1"
echo '{"/usr/lib64/libfixture.so.1": "fixture-libs"}' > "$work/owners.json"
attribute=(python3 "$root/packaging/appimage/notices.py" --attribute-only "$work/attribution" "$work/owners.json")
"${attribute[@]}" > /dev/null
touch "$work/attribution/unowned-file"
refuse 'unattributable file: unowned-file' "${attribute[@]}"
echo 'PASS: input, architecture, missing-component, contamination, symlink and notices refusals'
echo 'Desktop runtime acceptance is a separate manual check: see docs/appimage-build.md.'
