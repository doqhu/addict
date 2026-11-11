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
for item in lock['rpms']+lock['tools']+lock['ocr']:
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
python3 - "$work/AppDir" "$root/target/appimage-inputs" <<'PY'
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
echo 'PASS: input, architecture, missing-component, contamination and symlink refusals'
echo 'Desktop runtime acceptance is a separate manual check: see docs/appimage-build.md.'
