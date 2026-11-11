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
python3 - "$appdir" "$(dirname "${BASH_SOURCE[0]}")/../packaging/appimage/inputs.lock.json" "${artifact:-}" \
    "$(dirname "${BASH_SOURCE[0]}")/../packaging/appimage/static-closures.json" <<'PY'
import hashlib, json, os, pathlib, re, sys
root = pathlib.Path(sys.argv[1]).resolve()
def fail(message):
    raise SystemExit(message)
def require(pattern):
    found = [p for p in root.glob(pattern) if p.is_file()]
    if not found:
        fail('missing component: ' + pattern)
    return found
for pattern in ['AppRun', 'AppRun.sh', 'sharun', 'dash', 'bin/addict', 'shared/bin/addict', 'bin/tesseract', 'shared/bin/tesseract',
                'lib/libtesseract.so*', 'lib/libleptonica.so*', 'share/ocr-components.sha256',
                'share/licenses/ocr/tessdata-LICENSE', 'share/licenses/ocr/tesseract/*',
                'share/licenses/ocr/leptonica/*',
                'lib/libgtk-4.so*', 'lib/libGLESv2.so*', 'lib/libvulkan_lvp.so*',
                'share/vulkan/icd.d/lvp_icd*.json', 'lib/libc.so.6', 'lib/ld-linux-x86-64.so.2',
                'share/glib-2.0/schemas/gschemas.compiled', 'etc/fonts/fonts.conf',
                'share/fonts/**/*.ttf', 'share/icons/hicolor/*/apps/io.github.doqhu.Addict.png',
                'share/icons/Adwaita/index.theme', 'lib/gio/modules/*.so',
                'share/metainfo/io.github.doqhu.Addict.metainfo.xml', '*.desktop',
                'share/licenses/addict/LICENSE', 'share/licenses/addict/crate-licenses.txt', 'share/licenses/components.json']:
    require(pattern)
for name in ['AppRun', 'AppRun.sh', 'sharun', 'dash', 'bin/addict', 'shared/bin/addict', 'bin/tesseract', 'shared/bin/tesseract']:
    if not os.access(root/name, os.X_OK): fail('not executable: '+name)
models = {x['file']: x['sha256'] for x in json.load(open(sys.argv[2]))['ocr'] if x['file'].endswith('.traineddata')}
for name, digest in models.items():
    paths = require('share/tessdata/'+name)
    if hashlib.file_digest(paths[0].open('rb'), 'sha256').hexdigest() != digest:
        fail('corrupt OCR model: share/tessdata/'+name)
# Manifest records the exact deployed closure, including transitive libraries.
inventory = {}
for line in (root/'share/ocr-components.sha256').read_text().splitlines():
    digest, name = line.split('  ', 1)
    path = root/name
    if not path.resolve().is_relative_to(root) or name in inventory:
        fail('invalid OCR inventory path: '+name)
    if not path.is_file(): fail('missing OCR dependency: '+name)
    if hashlib.file_digest(path.open('rb'), 'sha256').hexdigest() != digest:
        fail('corrupt OCR component: '+name)
    inventory[name] = digest
for pattern in ['bin/tesseract', 'shared/bin/tesseract', 'lib/libtesseract.so*', 'lib/libleptonica.so*', 'share/licenses/ocr/tessdata-LICENSE']:
    for path in require(pattern):
        # Symlink aliases share their resolved content's manifest entry.
        if str(path.relative_to(root)) not in inventory and str(path.resolve().relative_to(root)) not in inventory:
            fail('unrecorded OCR component: '+str(path.relative_to(root)))
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
    if p.suffix.lower() == '.traineddata' and (rel != 'share/tessdata/'+p.name or p.name not in models):
        fail('unexpected OCR model: '+rel)
    if p.suffix.lower() in {'.index', '.xz', '.dict', '.ifo', '.dsl', '.dz'} or header[:8] == b'ADDICTIX':
        fail('dictionary/OCR data contamination: ' + rel)
    if header[:4] == b'\x7fELF' and (header[4:6] != b'\x02\x01' or header[18:20] != b'\x3e\x00'):
        fail('unsupported ELF architecture: ' + rel)
    print(rel + (' -> ' + str(p.readlink()) if p.is_symlink() else ''))
# Notices: the inventory is trusted only after each path it names is checked in the image.
lock = json.load(open(sys.argv[2]))
pins = {i['file']: i for k in ('rpms', 'tools', 'ocr', 'notices', 'srpms', 'static') for i in lock.get(k, [])}
srpm_pins = {i['file'] for i in lock.get('srpms', [])}
closures = json.load(open(sys.argv[4]))
inventory = json.loads((root/'share/licenses/components.json').read_text())
components, attributed = inventory['components'], inventory['files']
present = []
for directory, dirs, names in os.walk(root):
    for name in names + [d for d in dirs if os.path.islink(os.path.join(directory, d))]:
        present.append(os.path.relpath(os.path.join(directory, name), root))
for name, component in sorted(components.items()):
    if not component.get('license') or not component.get('license_files'):
        fail('component without license: '+name)
    for rel in component['license_files']:
        path = root/rel
        if not path.resolve().is_relative_to(root) or not path.is_file() or not path.stat().st_size or attributed.get(rel) is None:
            fail('missing notice: '+name+': '+rel)
    source = component.get('source') or {}
    files = source.get('files', [])
    if any(pins.get(item.get('file')) != item for item in files):
        fail('unpinned source record: '+name)
    if re.search(r'\b(AGPL|LGPL|GPL|MPL|EPL|EUPL|CDDL)-', component['license']) and not files \
            and not (component.get('origin') == 'project' and source.get('revision')):
        fail('copyleft component without source: '+name)
    # A Fedora package's corresponding source is its pinned source RPM, not a binary RPM or a tool.
    if component.get('origin') == 'fedora-rpm' and not any(item.get('file') in srpm_pins for item in files):
        fail('fedora component without source RPM: '+name)
    if not files and not (component.get('origin') == 'project' and source.get('revision')):
        fail('component without source record: '+name)
for rel in sorted(present):
    if rel not in attributed: fail('file not in notices inventory: '+rel)
for rel, name in sorted(attributed.items()):
    if rel not in present: fail('notices inventory names a missing file: '+rel)
    if name not in components: fail('file of unknown component: '+rel+' ('+name+')')
# The pinned tools' binaries belong to their tools, whose static closures are checked below.
for rel, name in sorted(attributed.items()):
    owner = 'dash' if rel == 'dash' else 'anylinux-sharun' if rel == 'sharun' else \
        ('cross-libc-dlopen' if rel.endswith('/cross-libc-dlopen.so') else 'anylinux-sharun') if rel.startswith('lib/sharun-preload/') else None
    if owner and name != owner:
        fail('tool binary not attributed to '+owner+': '+rel+' ('+name+')')
runtime = sorted(n for n, c in components.items() if c.get('location') == 'runtime header')
# The runtime header is uruntime with an embedded DwarFS helper; every library
# statically linked into either is a component of its own.
for name in ['uruntime', 'dwarfs'] + sorted(n for n, c in closures['components'].items() if set(c['in']) & {'uruntime', 'dwarfs'}):
    if name not in runtime:
        fail('missing notice: runtime header ('+name+')')
# The static closures of the tools in the AppDir (sharun, dash) travel with them.
for name, entry in sorted(closures['components'].items()):
    hosts = sorted(set(entry['in']) & set(components) - {'uruntime', 'dwarfs'})
    if hosts and name not in components:
        fail('missing notice: static closure of '+hosts[0]+' ('+name+')')
    if name in components and components[name].get('license') != entry['license']:
        fail('static component license differs from static-closures.json: '+name)
if sys.argv[3]:
    with open(sys.argv[3], 'rb') as f:
        if b'github.com/VHSgunzo/uruntime' not in f.read(4 << 20): fail('unrecognized runtime header: '+sys.argv[3])
print('Notices: %d components (%d in the runtime header) cover %d files' % (len(components), len(runtime), len(attributed)))
print('Validated: x86_64, GTK, libc, loader, schemas, fonts, icons, modules, pinned EN/ES/CA OCR and closure; notices; no dictionary data')
PY
