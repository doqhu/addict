#!/usr/bin/env bash
# Inspect without a FUSE mount. --appdir is also used for disposable fixtures.
set -Eeuo pipefail
if [[ ${1:-} == --trace ]]; then
    [[ $# == 3 ]] || { echo "usage: $0 --trace PID EVIDENCE_DIR" >&2; exit 1; }
    python3 - "$2" "$3" <<'TRACE'
import os, pathlib, sys
proc, out = pathlib.Path('/proc')/str(int(sys.argv[1])), pathlib.Path(sys.argv[2])
out.mkdir(parents=True, exist_ok=True)
exe = os.readlink(proc/'exe')
if pathlib.Path(exe).name != 'addict':
    raise SystemExit('not an addict executable: ' + exe)
(out/'exe.txt').write_text(exe+'\n')
parents, current = [], int(sys.argv[1])
while current and len(parents) < 64:
    status = (pathlib.Path('/proc')/str(current)/'status').read_text()
    name = next(line for line in status.splitlines() if line.startswith('Name:'))
    parent = int(next(line.split()[1] for line in status.splitlines() if line.startswith('PPid:')))
    parents.append(f'{current} {name} parent={parent}\n'); current = parent
(out/'parents.txt').write_text(''.join(parents))
(out/'maps.txt').write_bytes((proc/'maps').read_bytes())
(out/'cmdline.txt').write_bytes((proc/'cmdline').read_bytes().replace(b'\0', b'\n'))
env = (proc/'environ').read_bytes().split(b'\0')
keys = (b'USE_HOST_XDG_CACHE_HOME=', b'APPIMAGE', b'APPDIR', b'ARG0', b'ARGV0', b'SHARUN', b'LD_', b'XDG_', b'GSETTINGS_', b'FONTCONFIG_', b'GIO_', b'GI_', b'GTK_', b'GDK_', b'LIBGL_', b'VK_', b'HOME=', b'PATH=', b'DISPLAY=', b'WAYLAND_DISPLAY=')
(out/'environment.txt').write_bytes(b'\n'.join(e for e in env if e.startswith(keys))+b'\n')
print('Recorded process paths and packaging environment in ' + str(out))
TRACE
    exit 0
fi
[[ $# == 1 || ( $# == 2 && $1 == --appdir ) ]] || { echo "usage: $0 IMAGE | --appdir DIR" >&2; exit 1; }
if [[ $1 == --appdir ]]; then
    appdir=$(realpath -- "$2")
else
    artifact=$(realpath -- "$1")
    [[ -x $artifact ]] || { echo "missing executable artifact: $artifact" >&2; exit 1; }
    python3 - "$artifact" <<'PY'
import sys
b = open(sys.argv[1], 'rb').read(20)
if b[:5] != b'\x7fELF\x02' or b[5] != 1 or b[18:20] != b'\x3e\x00':
    raise SystemExit('unsupported artifact architecture: ' + sys.argv[1])
PY
    work=$(mktemp -d)
    trap 'rm -rf -- "$work"' EXIT
    (cd "$work" && "$artifact" --appimage-extract >/dev/null)
    appdir=$work/AppDir
    [[ -d $appdir ]] || { echo "missing extracted AppDir: $artifact" >&2; exit 1; }
fi
python3 - "$appdir" <<'PY'
import pathlib, sys
root = pathlib.Path(sys.argv[1]).resolve()
def fail(message):
    raise SystemExit(message)
def require(pattern):
    found = [p for p in root.glob(pattern) if p.is_file()]
    if not found:
        fail('missing component: ' + pattern)
    return found
for pattern in ['AppRun', 'AppRun.sh', 'sharun', 'dash', 'bin/addict', 'shared/bin/addict',
                'lib/libgtk-4.so*', 'lib/libGLESv2.so*', 'lib/libvulkan_lvp.so*',
                'share/vulkan/icd.d/lvp_icd*.json', 'lib/libc.so.6', 'lib/ld-linux-x86-64.so.2',
                'share/glib-2.0/schemas/gschemas.compiled', 'etc/fonts/fonts.conf',
                'share/fonts/**/*.ttf', 'share/icons/hicolor/*/apps/io.github.doqhu.Addict.png',
                'share/icons/Adwaita/index.theme', 'lib/gio/modules/*.so',
                'share/metainfo/io.github.doqhu.Addict.metainfo.xml', '*.desktop']:
    require(pattern)
for p in root.rglob('*'):
    if p.is_symlink():
        target = p.resolve()
        if not target.is_relative_to(root) or not target.exists():
            fail('escaping or broken symlink: ' + str(p.relative_to(root)))
    if p.is_dir():
        print(str(p.relative_to(root)) + (' -> ' + str(p.readlink()) if p.is_symlink() else '/'))
        continue
    if not p.is_file():
        continue
    rel = str(p.relative_to(root))
    with p.open('rb') as f:
        header = f.read(20)
    if p.suffix.lower() in {'.index', '.xz', '.dict', '.ifo', '.dsl', '.dz', '.traineddata'} or header[:8] == b'ADDICTIX':
        fail('dictionary/OCR data contamination: ' + rel)
    if header[:4] == b'\x7fELF' and (header[4:6] != b'\x02\x01' or header[18:20] != b'\x3e\x00'):
        fail('unsupported ELF architecture: ' + rel)
    print(rel + (' -> ' + str(p.readlink()) if p.is_symlink() else ''))
print('Validated: x86_64, GTK, libc, loader, schemas, fonts, icons, modules; no dictionary data')
PY
