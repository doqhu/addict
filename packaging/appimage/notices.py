#!/usr/bin/env python3
"""Attribute every AppDir entry to a component and install its notices.

Runs inside the pinned build container after quick-sharun has deployed the
AppDir. Component identity comes from the installed RPM headers (file
ownership, license flags, source RPM) and the lock; the rules below cover only
project files, relocated notices, files the pinned tools generate, and caches.

Code statically linked into the prebuilt tools (the runtime header's uruntime
and DwarFS helper, sharun, dash) is recorded in static-closures.json, which
closures.py generates; each library there becomes a component of its own.

usage: notices.py APPDIR LOCK INPUTS_DIR SRPM_DIR STATIC_DIR SOURCE_DIR
       notices.py --attribute-only APPDIR [OWNERS]
           attribution only, for the refusal tests; OWNERS is a JSON map of
           system path -> package that replaces the RPM database
"""
import collections, fnmatch, hashlib, io, json, os, pathlib, re, shutil, subprocess, sys, tarfile, tempfile, zipfile

INVENTORY = 'share/licenses/components.json'
CLOSURES = pathlib.Path(__file__).resolve().parent / 'static-closures.json'
# Hosts whose static closure is outside the AppDir, in the AppImage runtime header.
RUNTIME_HOSTS = ('uruntime', 'dwarfs')
# AppDir prefix -> system prefixes; the first owned candidate wins.
PREFIXES = [
    ('shared/bin/', ['/usr/bin/', '/usr/libexec/', '/usr/libexec/gstreamer-1.0/']),
    ('bin/', ['/usr/bin/', '/usr/libexec/']),
    ('lib/gstreamer-1.0/', ['/usr/libexec/gstreamer-1.0/']),
    ('lib/', ['/usr/lib64/', '/usr/lib/']),
    ('share/', ['/usr/share/']),
    ('etc/', ['/etc/']),
]
PROJECT = ['shared/bin/addict', 'io.github.doqhu.Addict.desktop', 'share/applications/io.github.doqhu.Addict.desktop',
           'share/metainfo/io.github.doqhu.Addict.metainfo.xml', 'share/icons/hicolor/*/apps/io.github.doqhu.Addict.png',
           '.DirIcon', '256.png', 'share/licenses/addict/*', 'share/ocr-components.sha256', INVENTORY]
# Files of the pinned tools (sharun.tar, cross-libc-dlopen.tar) and quick-sharun's generated layout.
TOOL_RULES = [
    ('sharun', 'anylinux-sharun'), ('lib/sharun-preload/anylinux.so', 'anylinux-sharun'),
    ('lib/sharun-preload/glycin-fix.so', 'anylinux-sharun'),
    ('lib/sharun-preload/cross-libc-dlopen.so', 'cross-libc-dlopen'),
    ('dash', 'dash'), ('share/licenses/dash-COPYING', 'dash'),
    ('AppRun.sh', 'anylinux-appimages'), ('AppRun.lib', 'anylinux-appimages'), ('.env', 'anylinux-appimages'),
    ('bin/*.hook', 'anylinux-appimages'), ('lib/lib.path', 'anylinux-appimages'), ('shared/lib', 'anylinux-appimages'),
    ('etc/ssl/openssl.cnf', 'anylinux-appimages'),  # written by quick-sharun, not Fedora's file
    ('share/tessdata/eng.traineddata', 'tessdata_fast'), ('share/tessdata/spa.traineddata', 'tessdata_fast'),
    ('share/tessdata/cat.traineddata', 'tessdata_fast'), ('share/licenses/ocr/tessdata-LICENSE', 'tessdata_fast'),
]
# Caches built at deploy time belong to the package whose data they index.
CACHE_RULES = [('share/glib-2.0/schemas/gschemas.compiled', 'glib2'),
               ('share/icons/Adwaita/icon-theme.cache', 'adwaita-icon-theme')]
# Notices the build already relocates: share/licenses/ocr/<rpm>/... belongs to <rpm>.
RELOCATED = re.compile(r'^share/licenses/ocr/([^/]+)/')
LICENSE_NAME = re.compile(r'^(COPYING|COPYRIGHT|LICEN[CS]E|NOTICE|OFL)([._-].*)?$', re.I)
# Upstreams without a license file state it elsewhere in their own source:
# source RPM name -> {archive path below the top directory: whole file or its leading comments}.
IN_SOURCE_NOTICES = {'libdrm': {'xf86drm.c': 'header'}, 'libglvnd': {'README.md': 'whole'}}
COPYLEFT = re.compile(r'\b(AGPL|LGPL|GPL|MPL|EPL|EUPL|CDDL)-')


def fail(message):
    raise SystemExit(message)


def installed_rpms():
    fmt = '[%{=NAME}\t%{=NAME}-%{=EPOCHNUM}:%{=VERSION}-%{=RELEASE}.%{=ARCH}\t%{=SOURCERPM}\t%{=LICENSE}\t%{FILEFLAGS}\t%{FILENAMES}\n]'
    out = subprocess.run(['rpm', '-qa', '--qf', fmt], check=True, capture_output=True, text=True).stdout
    packages, owner = {}, collections.defaultdict(set)
    for line in out.splitlines():
        name, nevra, srpm, spdx, flags, filename = line.split('\t', 5)
        package = packages.setdefault(name, {'nevra': nevra, 'srpm': srpm, 'license': spdx, 'license_paths': []})
        if int(flags) & 128 and os.path.isfile(filename):  # RPMFILE_LICENSE
            package['license_paths'].append(filename)
        owner[filename].add(name)
        # Fedora's usrmerge: /lib64/x is /usr/lib64/x, and directories may be symlinked.
        real = os.path.realpath(filename)
        if real != filename:
            owner[real].add(name)
    return packages, owner


def fixture_owners(path):
    """Owner map of a refusal fixture: system path -> package, no RPM database."""
    owner = collections.defaultdict(set)
    for filename, name in json.loads(pathlib.Path(path).read_text()).items():
        owner[filename].add(name)
    packages = {n: {'nevra': n, 'srpm': n + '.src.rpm', 'license': 'MIT', 'license_paths': []} for names in owner.values() for n in names}
    return packages, owner


def addict_license(source):
    """MIT for addict itself and every license heading of its crate list."""
    text = (source / 'packaging/licenses/crate-licenses.txt').read_text()
    ids = set(re.findall(r'^-{20,}\n.*\(([^()\n]+)\)$', text, re.M))
    if not ids:
        fail('no license headings in packaging/licenses/crate-licenses.txt')
    return ' AND '.join(['MIT'] + sorted(ids - {'MIT'}))


def read_members(path, wanted):
    """{member: bytes} of the wanted members of a tar archive, in one pass."""
    found = {}
    with tarfile.open(path) as t:
        for info in t:
            if info.name in wanted and info.isfile():
                found[info.name] = t.extractfile(info).read()
    missing = set(wanted) - set(found)
    if missing:
        fail('license text not in ' + path.name + ': ' + ', '.join(sorted(missing)))
    return found


def entries(root):
    """Every regular file and symlink, without following directory symlinks."""
    found = []
    for directory, dirs, files in os.walk(root):
        for name in files + [d for d in dirs if os.path.islink(os.path.join(directory, d))]:
            found.append(os.path.relpath(os.path.join(directory, name), root))
    return sorted(found)


def digest(path):
    with open(path, 'rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def attribute(root, packages, owner):
    sharun = root / 'sharun'
    sharun_digest = digest(sharun) if sharun.is_file() else None
    files, unattributed = {}, []
    for rel in entries(root):
        path = root / rel
        component = next((c for p, c in TOOL_RULES if fnmatch.fnmatchcase(rel, p)), None)
        if component is None and any(fnmatch.fnmatchcase(rel, p) for p in PROJECT):
            component = 'addict'
        if component is None and rel.startswith('share/licenses/') and rel.count('/') >= 3:
            component = rel.split('/')[2]  # notices this script installs
            match = RELOCATED.match(rel)
            if match:
                component = match.group(1)
            if component == 'ocr':
                component = None
        # sharun installs itself as AppRun and as every bin/ launcher.
        if component is None and sharun_digest and not path.is_symlink() and path.is_file() \
                and (rel == 'AppRun' or rel.startswith('bin/')) and digest(path) == sharun_digest:
            component = 'anylinux-sharun'
        if component is None:
            for prefix, targets in PREFIXES:
                if not rel.startswith(prefix):
                    continue
                for target in targets:
                    candidate = target + rel[len(prefix):]
                    names = owner.get(candidate) or owner.get(os.path.realpath(candidate))
                    if names:
                        component = sorted(names)[0]
                        break
                if component:
                    break
        if component is None:
            component = next((c for p, c in CACHE_RULES if rel == p), None)
        if component is None:
            unattributed.append(rel)
        else:
            files[rel] = component
    if unattributed:
        fail('unattributable file: ' + ', '.join(unattributed))
    static = set(json.loads(CLOSURES.read_text())['components'])
    for rel, component in files.items():
        known = component in packages or component in static or component in (
            'addict', 'anylinux-sharun', 'cross-libc-dlopen', 'dash', 'anylinux-appimages', 'tessdata_fast', 'uruntime', 'dwarfs')
        if not known:
            fail('unknown component ' + component + ' for ' + rel)
    return files


def cpio_members(stream):
    """Yield (name, bytes) from an uncompressed newc cpio stream."""
    while True:
        header = stream.read(110)
        if len(header) < 110 or header[:6] not in (b'070701', b'070702'):
            fail('unsupported source package payload')
        fields = [int(header[6 + 8 * i:14 + 8 * i], 16) for i in range(13)]
        size, namesize = fields[6], fields[11]
        name = stream.read(namesize)[:-1].decode()
        stream.read((4 - (110 + namesize) % 4) % 4)
        data = stream.read(size)
        stream.read((4 - size % 4) % 4)
        if name == 'TRAILER!!!':
            return
        yield name.lstrip('./'), data


def archive_members(name, data):
    """Yield (path, bytes) of regular files in a source archive."""
    if name.endswith(('.zst', '.tzst')):
        data = subprocess.run(['zstd', '-dc'], input=data, check=True, capture_output=True).stdout
    if zipfile.is_zipfile(io.BytesIO(data)):
        with zipfile.ZipFile(io.BytesIO(data)) as z:
            for info in z.infolist():
                if not info.is_dir():
                    yield info.filename, z.read(info)
        return
    try:
        with tarfile.open(fileobj=io.BytesIO(data)) as t:
            for info in t:
                if info.isfile():
                    yield info.name, t.extractfile(info).read()
    except tarfile.ReadError:
        return


def srpm_license_texts(srpm, named_files):
    """The upstream license texts in a source RPM: its spec's %license names
    and conventional license files, at the top of each source archive or as
    separate sources."""
    with tempfile.TemporaryFile() as payload:
        subprocess.run(['rpm2cpio', str(srpm)], stdout=payload, check=True)
        payload.seek(0)
        members = dict(cpio_members(payload))
    spec = next((data.decode(errors='replace') for name, data in members.items() if name.endswith('.spec')), '')
    named = set()
    for line in re.findall(r'^%license\s+(.+)$', spec, re.M):
        named.update(os.path.basename(token) for token in line.split() if '%' not in token and '*' not in token)
    texts = {}
    for name, data in members.items():
        if LICENSE_NAME.match(name) or name in named:
            texts[name] = data
            continue
        for member, content in archive_members(name, data):
            parts = member.lstrip('./').split('/')
            inner = '/'.join(parts[1:])
            if inner in named_files:
                text = content if named_files[inner] == 'whole' else content.split(b'#include', 1)[0]
                texts[inner.replace('/', '-') + ('' if named_files[inner] == 'whole' else '.license-header')] = text
            elif len(parts) == 2 and (LICENSE_NAME.match(parts[1]) or parts[1] in named) \
                    or len(parts) == 3 and parts[1] == 'LICENSES':
                if content.strip():
                    texts.setdefault('/'.join(parts[1:]), content)
    return texts


def install(root, component, name, data, installed):
    rel = 'share/licenses/' + component + '/' + name
    path = root / rel
    if path.exists() and path.read_bytes() != data:
        fail('conflicting notice: ' + rel)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    installed.append(rel)


def main(argv):
    if argv[:1] == ['--attribute-only']:
        packages, owner = fixture_owners(argv[2]) if len(argv) > 2 else installed_rpms()
        files = attribute(pathlib.Path(argv[1]).resolve(), packages, owner)
        print('Attributed ' + str(len(files)) + ' files')
        return
    root, lock_path, inputs, srpms, static, source = (pathlib.Path(a).resolve() for a in argv)
    lock = json.loads(lock_path.read_text())
    pinned = {item['file']: item for key in ('rpms', 'tools', 'ocr', 'srpms', 'notices', 'static') for item in lock[key]}
    srpm_pins = {item['file']: item for item in lock['srpms']}
    notice = lambda name: (inputs / name).read_bytes()
    packages, owner = installed_rpms()
    # A first pass names every unattributable path before any notice is written.
    files = attribute(root, packages, owner)
    components = {}
    revision = (source / 'source-revision.txt').read_text().strip()
    snapshot = (source / 'source-snapshot.sha256').read_text().strip()
    components['addict'] = {
        'origin': 'project', 'version': revision,
        'license': addict_license(source),
        'license_files': ['share/licenses/addict/LICENSE', 'share/licenses/addict/CREDITS', 'share/licenses/addict/crate-licenses.txt'],
        'source': {'url': 'https://github.com/doqhu/addict', 'revision': revision, 'snapshot_sha256': snapshot}}
    components['tessdata_fast'] = {
        'origin': 'pinned-download', 'inputs': [pinned[n] for n in ('eng.traineddata', 'spa.traineddata', 'cat.traineddata')],
        'license': 'Apache-2.0', 'license_files': ['share/licenses/ocr/tessdata-LICENSE'],
        'source': {'files': [pinned[n] for n in ('eng.traineddata', 'spa.traineddata', 'cat.traineddata', 'tessdata-LICENSE')]}}
    tools = {
        'anylinux-sharun': ('MIT', ['sharun.tar'], [('LICENSE', notice('anylinux-sharun-LICENSE'))], ['anylinux-sharun-source.tar.gz'], None),
        'dash': ('BSD-3-Clause AND GPL-2.0-or-later', ['sharun.tar'], [], ['dash-0.5.13.5.tar.gz', 'anylinux-sharun-source.tar.gz'], None),
        'cross-libc-dlopen': ('MIT', ['cross-libc-dlopen.tar'], [('LICENSE', tar_member(inputs / 'cross-libc-dlopen.tar', 'LICENSE'))],
                              ['cross-libc-dlopen-source.tar.gz'], None),
        'anylinux-appimages': ('MIT', ['quick-sharun.sh'], [('LICENSE', notice('anylinux-appimages-LICENSE'))], ['quick-sharun.sh'], None),
        # The AppImage runtime header is outside the AppDir; its notices are inside.
        'uruntime': ('MIT', ['appimagetool'], [('LICENSE', notice('uruntime-LICENSE'))], ['uruntime-source.tar.gz'], 'runtime header'),
        'dwarfs': ('MIT AND GPL-3.0-or-later AND GPL-2.0-only AND Apache-2.0 AND BSD-3-Clause AND BSL-1.0 AND 0BSD AND CC0-1.0 AND Unlicense',
                   ['appimagetool'], [('LICENSE', notice('dwarfs-LICENSE'))] + [
                       ('LICENSES/' + item['file'][len('dwarfs-LICENSES-'):], notice(item['file']))
                       for item in lock['notices'] if item['file'].startswith('dwarfs-LICENSES-')],
                   ['dwarfs-0.15.7.tar.xz', 'dwarfs-source.tar.gz'], 'runtime header'),
    }
    closures = json.loads(CLOSURES.read_text())
    static_pins = {item['file']: item for item in lock['static']}
    verified = set()

    def input_path(name):
        if name in static_pins:
            path = static / name
            if name not in verified:
                if digest(path) != static_pins[name]['sha256']:
                    fail('checksum mismatch: ' + name)
                verified.add(name)
            return path
        if name in pinned:
            return inputs / name
        fail('unpinned static-closure input: ' + name)

    def closure_texts(texts):
        """[(installed name, bytes)] of [{input, member, as}] records."""
        wanted = collections.defaultdict(set)
        for text in texts:
            if text['member']:
                wanted[text['input']].add(text['member'])
        data = {name: read_members(input_path(name), members) for name, members in wanted.items()}
        return [(t['as'], data[t['input']][t['member']] if t['member'] else input_path(t['input']).read_bytes()) for t in texts]

    for name, texts in closures['tool_texts'].items():
        tools[name][2].extend(closure_texts(texts))
    for name, (spdx, used, texts, sources, location) in tools.items():
        installed = ['share/licenses/dash-COPYING'] if name == 'dash' else []
        for text_name, data in texts:
            install(root, name, text_name, data, installed)
        components[name] = {'origin': 'pinned-download', 'inputs': [pinned[n] for n in used], 'license': spdx,
                            'license_files': installed, 'source': {'files': [pinned[n] for n in sources]}}
        if location:
            components[name]['location'] = location
        if name in closures['hosts']:
            components[name]['static_closure'] = closures['hosts'][name]['determined_by']
    # Libraries linked into those tools: one component each.
    for name, entry in closures['components'].items():
        installed = []
        for text_name, data in closure_texts(entry['license_texts']):
            install(root, name, text_name, data, installed)
        components[name] = {'origin': 'static', 'version': entry['version'], 'license': entry['license'],
                            'embedded_in': entry['in'], 'license_files': installed,
                            'source': {'files': [pinned[n] if n in pinned else fail('unpinned source: ' + n) for n in entry['sources']]}}
        if any(host in RUNTIME_HOSTS for host in entry['in']):
            components[name]['location'] = 'runtime header'
        if entry.get('note'):
            components[name]['note'] = entry['note']
    srpm_texts = {}
    for name in sorted({c for c in files.values() if c in packages}):
        package = packages[name]
        srpm = package['srpm']
        if srpm not in srpm_pins:
            fail('unpinned source package: ' + srpm + ' (component ' + name + ')')
        installed, origin = [], 'rpm'
        paths = package['license_paths']
        if not paths:
            sibling = next((m for m in sorted(packages) if packages[m]['srpm'] == srpm and packages[m]['license_paths']), None)
            if sibling:
                paths, origin = packages[sibling]['license_paths'], 'sibling ' + sibling
        for p in paths:
            parts = pathlib.PurePath(p).parts
            text_name = '/'.join(parts[5:]) if parts[1:4] == ('usr', 'share', 'licenses') and len(parts) > 5 else parts[-1]
            install(root, name, text_name, pathlib.Path(p).read_bytes(), installed)
        if not paths:
            if srpm not in srpm_texts:
                path = srpms / srpm
                if digest(path) != srpm_pins[srpm]['sha256']:
                    fail('checksum mismatch: ' + srpm)
                srpm_texts[srpm] = srpm_license_texts(path, IN_SOURCE_NOTICES.get(srpm.rsplit('-', 2)[0], {}))
            if not srpm_texts[srpm]:
                fail('no license text for ' + name + ' in ' + srpm)
            origin = 'srpm ' + srpm
            for text_name, data in sorted(srpm_texts[srpm].items()):
                install(root, name, text_name, data, installed)
        components[name] = {'origin': 'fedora-rpm', 'nevra': package['nevra'], 'license': package['license'],
                            'license_files': installed, 'license_texts_from': origin, 'source': {'files': [srpm_pins[srpm]]}}
    for name, component in components.items():
        for rel in component['license_files']:
            if not (root / rel).is_file() or not (root / rel).stat().st_size:
                fail('missing notice: ' + name + ': ' + rel)
        if COPYLEFT.search(component['license']) and not component['source'].get('files') \
                and not (component['origin'] == 'project' and component['source'].get('revision')):
            fail('copyleft component without source: ' + name)
    # The notices installed above are attributed by the same rules.
    (root / INVENTORY).write_text('{}')
    files = attribute(root, packages, owner)
    used = set(files.values()) | {n for n, c in components.items() if c.get('location')}
    # A static library is redistributed wherever one of its host binaries is.
    used |= {n for n, c in components.items() if set(c.get('embedded_in', [])) & used}
    components = {n: c for n, c in components.items() if n in used}
    inventory = {'schema': 1, 'components': components, 'files': files}
    (root / INVENTORY).write_text(json.dumps(inventory, indent=1, sort_keys=True) + '\n')
    print('Notices: ' + str(len(components)) + ' components (' + str(sum(c['origin'] == 'static' for c in components.values()))
          + ' statically linked), ' + str(len(files)) + ' files')


def tar_member(archive, member):
    with tarfile.open(archive) as t:
        return t.extractfile(member).read()


if __name__ == '__main__':
    main(sys.argv[1:])
