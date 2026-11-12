#!/usr/bin/env python3
"""Regenerate packaging/appimage/static-closures.json and the lock's `static` array.

The AppImage redistributes prebuilt static binaries: the uruntime runtime
header (Rust, with an embedded DwarFS `dwarfs-fuse-extract` helper in C++),
`sharun` (Rust) and `dash` (C, musl), all from pinned downloads. Every library
statically linked into them is a component with its own notices and pinned
source. This script records those components; notices.py installs them at
build time and check-appimage.sh requires them.

usage: packaging/appimage/closures.py [CACHE]   (default target/appimage-inputs)

Run it after bumping uruntime/appimagetool, Anylinux-sharun or DwarFS, then
review the diff. Needs network access and cargo (RUSTC_BOOTSTRAP=1 lets a
stable cargo read the tools' nightly manifests). How each closure is decided
is written into the output, under `hosts`.
"""
import hashlib, json, os, pathlib, re, shutil, subprocess, sys, tarfile, tempfile, tomllib, urllib.request

HERE = pathlib.Path(__file__).resolve().parent
LOCK = HERE / 'inputs.lock.json'
OUT = HERE / 'static-closures.json'
TARGET = 'x86_64-unknown-linux-musl'
LICENSE_NAME = re.compile(r'^(COPYING|COPYRIGHT|LICEN[CS]E|UNLICENSE|NOTICE)([._-].*)?$', re.I)
# The SPDX license list at a pinned revision (v3.29.0): the steward text for a crate
# whose package carries no license file of its own.
SPDX_DATA = 'https://raw.githubusercontent.com/spdx/license-list-data/31ba1a50e5397e00a304dbadc76531740e89ee48/text/'

# Tools and how their closure is determined.
HOSTS = {
    'uruntime': {
        'location': 'runtime header',
        'determined_by': (
            'uruntime v0.8.1 variant appimage-dwarfs-lite (the only helper embedded in the pinned header is '
            'dwarfs-fuse-extract, payload sha256 2c230635... = uruntime checksums.txt x86_64 dwarfs-fuse-extract). '
            'Crates: cargo metadata over the pinned Cargo.lock, --no-default-features --features appimage,dwarfs,lite, '
            'target x86_64-unknown-linux-musl, normal dependencies only, proc-macro crates and their build-time '
            'dependencies excluded. std: the header .comment names rustc 1.100.0-nightly (6eeff9a52 2026-09-23), '
            'the 2026-09-24 nightly; build-std = std, panic_abort. Linked by xtask zig-linker with Zig 0.16.0 '
            '(uruntime checksums.txt), which supplies musl and compiler_rt.')},
    'dwarfs': {
        'location': 'runtime header',
        'determined_by': (
            'dwarfs-fuse-extract 0.15.7, built by the DwarFS static release (.docker/build-linux.sh, '
            'clang, -static, -rtlib=libgcc, libressl variant, minimal feature set). Libraries from the pinned '
            'source at 3945d23 (.docker/static-libs-versions.sh, toolchain-versions.sh) confirmed by the strings of '
            'the embedded binary (LibreSSL 4.3.1, libarchive 3.8.8, liblzma, libzstd, jemalloc, FUSE3, boost '
            'filesystem/program_options, utf8, nlohmann, xxh3, GCC 16.1.0 runtime). Absent: lz4, brotli, flac, '
            'ricepp (minimal build; only DwarFS compression-type names remain), fuse2 (FUSE3 found and '
            'ONLY_UNIVERSAL_BINARIES), mimalloc, OpenSSL, libunwind, cpptrace, libdwarf, benchmark, libmagic, '
            'libucontext (other build types or architectures).')},
    'anylinux-sharun': {
        'location': None,
        'determined_by': (
            'sharun 3.8.3 (Anylinux-sharun 2748a37, resolution cutoff 2026-11-05T03:42:31Z): the pinned source has no '
            'Cargo.lock, so the crates are resolved with cargo generate-lockfile --publish-time at that cutoff '
            'and checked against the actual x86_64 compilation log (run 41963279170, job 127034383534; '
            'packaging/appimage/sharun-provenance.json), whose archive and binary SHA-256 match the pinned tar. '
            'Normal dependencies, target x86_64-unknown-linux-musl, proc-macro crates excluded. std from the pinned '
            'toolchain nightly-2026-09-11 (build-std = std, panic_abort). Linked by zig cc 0.16.0 (ci.yml, '
            '+crt-static, link-self-contained=no), which supplies musl and compiler_rt; the preload libraries are '
            'built with the same zig cc.')},
    'dash': {
        'location': None,
        'determined_by': (
            'dash 0.5.13.5 built by Anylinux-sharun tools/build-dash.sh with zig cc 0.16.0 -target x86_64-linux-musl '
            '-static: Zig supplies musl and compiler_rt.')},
}

# C/C++ code linked into dwarfs-fuse-extract, from the DwarFS static build's own pins.
DWARFS_LIBS = [
    # name, version, SPDX, url, input name, license members
    ('boost', '1.91.0', 'BSL-1.0', 'https://github.com/boostorg/boost/releases/download/boost-1.91.0-1/boost-1.91.0-1-cmake.tar.xz',
     'boost-1.91.0-1-cmake.tar.xz', ['LICENSE_1_0.txt']),
    ('libarchive', '3.8.8', 'BSD-2-Clause AND BSD-3-Clause AND LicenseRef-Public-Domain AND (CC0-1.0 OR OpenSSL OR Apache-2.0)',
     'https://github.com/libarchive/libarchive/releases/download/v3.8.8/libarchive-3.8.8.tar.xz', 'libarchive-3.8.8.tar.xz', ['COPYING']),
    ('libressl', '4.3.1', 'OpenSSL AND ISC AND LicenseRef-Public-Domain',
     'https://github.com/libressl/portable/releases/download/v4.3.1/libressl-4.3.1.tar.gz', 'libressl-4.3.1.tar.gz', ['COPYING']),
    ('libfuse3', '3.18.2', 'LGPL-2.1-only', 'https://github.com/libfuse/libfuse/releases/download/fuse-3.18.2/fuse-3.18.2.tar.gz',
     'fuse-3.18.2.tar.gz', ['LICENSE', 'LGPL2.txt', 'GPL2.txt']),
    ('liblzma', '5.8.3', '0BSD', 'https://github.com/tukaani-project/xz/releases/download/v5.8.3/xz-5.8.3.tar.xz',
     'xz-5.8.3.tar.xz', ['COPYING', 'COPYING.0BSD']),
    ('zstd', '1.5.7', 'BSD-3-Clause OR GPL-2.0-only', 'https://github.com/facebook/zstd/releases/download/v1.5.7/zstd-1.5.7.tar.gz',
     'zstd-1.5.7.tar.gz', ['LICENSE', 'COPYING']),
    ('xxhash', '0.8.3', 'BSD-2-Clause', 'https://github.com/Cyan4973/xxHash/archive/refs/tags/v0.8.3.tar.gz',
     'xxHash-0.8.3.tar.gz', ['LICENSE']),
    ('jemalloc', '5.3.1', 'BSD-2-Clause', 'https://github.com/jemalloc/jemalloc/releases/download/5.3.1/jemalloc-5.3.1.tar.bz2',
     'jemalloc-5.3.1.tar.bz2', ['COPYING']),
    ('fmt', '12.2.0', 'MIT', 'https://github.com/fmtlib/fmt/archive/refs/tags/12.2.0.tar.gz', 'fmt-12.2.0.tar.gz', ['LICENSE']),
    ('utfcpp', '4.1.1', 'BSL-1.0', 'https://github.com/nemtrif/utfcpp/archive/refs/tags/v4.1.1.tar.gz', 'utfcpp-4.1.1.tar.gz', ['LICENSE']),
    ('range-v3', '0.12.0', 'BSL-1.0 AND (NCSA OR MIT) AND HPND', 'https://github.com/ericniebler/range-v3/archive/refs/tags/0.12.0.tar.gz',
     'range-v3-0.12.0.tar.gz', ['LICENSE.txt']),
    ('parallel-hashmap', '2.0.0', 'Apache-2.0', 'https://github.com/greg7mdp/parallel-hashmap/archive/refs/tags/v2.0.0.tar.gz',
     'parallel-hashmap-2.0.0.tar.gz', ['LICENSE']),
    ('libstdc++', '16.1.0', 'GPL-3.0-or-later WITH GCC-exception-3.1', 'https://ftp.gnu.org/gnu/gcc/gcc-16.1.0/gcc-16.1.0.tar.xz',
     'gcc-16.1.0.tar.xz', ['COPYING3', 'COPYING.RUNTIME']),
    ('libgcc', '16.1.0', 'GPL-3.0-or-later WITH GCC-exception-3.1', 'https://ftp.gnu.org/gnu/gcc/gcc-16.1.0/gcc-16.1.0.tar.xz',
     'gcc-16.1.0.tar.xz', ['COPYING3', 'COPYING.RUNTIME']),
    ('musl', '1.2.6', 'MIT', 'https://musl.libc.org/releases/musl-1.2.6.tar.gz', 'musl-1.2.6.tar.gz', ['COPYRIGHT']),
]
# nlohmann/json is used as its single released header; the license is beside it in the repository.
NLOHMANN = [('nlohmann-json-3.12.0.hpp', 'https://github.com/nlohmann/json/releases/download/v3.12.0/json.hpp'),
            ('nlohmann-json-3.12.0-LICENSE.MIT', 'https://raw.githubusercontent.com/nlohmann/json/v3.12.0/LICENSE.MIT')]
DWARFS_BUILD = ('dwarfs-source.tar.gz', 'https://github.com/mhx/dwarfs/archive/3945d234619504bc149da18cfb208218779c477d.tar.gz')
ZIG = ('zig-0.16.0.tar.xz', 'https://ziglang.org/download/0.16.0/zig-0.16.0.tar.xz')
GPL2 = ('gnu-gpl-2.0.txt', 'https://www.gnu.org/licenses/old-licenses/gpl-2.0.txt')
RUST_SRC = {'uruntime': '2026-09-24', 'anylinux-sharun': '2026-09-11'}
SHARUN_RESOLVED_AT = '2026-11-05T03:42:31Z'
# std's dependencies on Linux: the non-optional ones and those of the backtrace
# feature, which may or may not survive panic = immediate-abort.
STD_DEPS = ['cfg-if', 'hashbrown', 'libc', 'rustc-demangle', 'addr2line', 'gimli', 'object', 'miniz_oxide', 'adler2', 'memchr']


def fetch(cache, name, url):
    path = cache / name
    if not path.exists():
        print('Fetching ' + url, flush=True)
        with urllib.request.urlopen(url, timeout=300) as src, open(str(path) + '.part', 'wb') as dst:
            shutil.copyfileobj(src, dst)
        pathlib.Path(str(path) + '.part').replace(path)
    return {'file': name, 'url': url, 'sha256': hashlib.file_digest(open(path, 'rb'), 'sha256').hexdigest()}


def top_members(path):
    """Names below the archive's top directory -> full member name."""
    with tarfile.open(path) as t:
        return {m.name.split('/', 1)[1]: m.name for m in t if m.isfile() and '/' in m.name}


def license_members(members):
    return sorted(k for k in members if '/' not in k and LICENSE_NAME.match(k))


def spdx_ids(expression):
    return sorted(set(re.findall(r'[A-Za-z0-9.+-]+', expression)) - {'AND', 'OR', 'WITH'})


def cargo_closure(manifest_dir, args):
    meta = json.loads(subprocess.run(
        ['cargo', 'metadata', '--format-version', '1', '--filter-platform', TARGET] + args,
        cwd=manifest_dir, check=True, capture_output=True, text=True, env={**os.environ, 'RUSTC_BOOTSTRAP': '1'}).stdout)
    packages = {p['id']: p for p in meta['packages']}
    nodes = {n['id']: n for n in meta['resolve']['nodes']}
    proc_macro = lambda p: all('proc-macro' in t['kind'] for t in p['targets'])
    seen, stack = set(), [meta['resolve']['root']]
    while stack:
        node = nodes[stack.pop()]
        for dep in node['deps']:
            if any(k['kind'] is None for k in dep['dep_kinds']) and dep['pkg'] not in seen \
                    and not proc_macro(packages[dep['pkg']]):
                seen.add(dep['pkg'])
                stack.append(dep['pkg'])
    lock = tomllib.loads((manifest_dir / 'Cargo.lock').read_text())
    sums = {(p['name'], p['version']): p.get('checksum') for p in lock['package']}
    return [(packages[i], sums.get((packages[i]['name'], packages[i]['version']))) for i in sorted(seen)]


def check_sharun_provenance(lock, components, cache):
    evidence = json.loads((HERE / 'sharun-provenance.json').read_text())
    pins = {i['file']: i for group in ('tools', 'notices', 'static') for i in lock[group]}
    for name, expected in evidence['inputs'].items():
        if pins[name]['sha256'] != expected or digest(cache / name) != expected:
            raise SystemExit('sharun provenance input mismatch: ' + name)
    if evidence['source_revision'] not in pins['anylinux-sharun-source.tar.gz']['url']:
        raise SystemExit('sharun provenance source revision mismatch')
    with tarfile.open(cache / 'sharun.tar') as archive:
        actual = hashlib.file_digest(archive.extractfile('sharun'), 'sha256').hexdigest()
    if actual != evidence['binary_sha256']:
        raise SystemExit('sharun provenance binary mismatch')
    actual = {name.removeprefix('rust-crate-'): c['version'] for name, c in components.items()
              if name.startswith('rust-crate-') and 'anylinux-sharun' in c['in']}
    if actual != evidence['runtime_crates']:
        raise SystemExit('sharun provenance crate versions mismatch')
    date = evidence['toolchain_date']
    expected = {'rust-std-nightly-' + date: evidence['rustc'],
                'rust-compiler-builtins-nightly-' + date: '0.1 (in-tree)'}
    actual = {name: c['version'] for name, c in components.items()
              if name.startswith(('rust-std-', 'rust-compiler-builtins-')) and 'anylinux-sharun' in c['in']}
    if actual != expected or any(components[name]['sources'] != ['rust-src-nightly-' + date + '.tar.xz']
                                 for name in actual):
        raise SystemExit('sharun provenance toolchain components mismatch')


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def main(argv):
    if argv and argv[0] == '--check-sharun-provenance':
        check_sharun_provenance(json.loads(LOCK.read_text()), json.loads(OUT.read_text())['components'],
                                pathlib.Path(argv[1] if len(argv) > 1 else 'target/appimage-inputs'))
        print('PASS: sharun release provenance, 22 runtime crate versions and Rust toolchain components')
        return
    cache = pathlib.Path(argv[0] if argv else 'target/appimage-inputs').resolve()
    cache.mkdir(parents=True, exist_ok=True)
    lock = json.loads(LOCK.read_text())
    notices = {i['file']: i for i in lock['notices']}
    pins, components = {}, {}

    def pin(name, url):
        pins[name] = fetch(cache, name, url)
        return name

    def add(name, version, spdx, hosts, texts, sources, note=None):
        """texts: [(input, member or None, installed name)]"""
        c = components.setdefault(name, {'version': version, 'license': spdx, 'in': [], 'license_texts': [], 'sources': []})
        if c['license'] != spdx:
            raise SystemExit('conflicting license for ' + name)
        c['in'] = sorted(set(c['in']) | set(hosts))
        if not c['license_texts']:
            c['license_texts'] = [{'input': i, 'member': m, 'as': a} for i, m, a in texts]
        c['sources'] = sorted(set(c['sources']) | set(sources))
        if note:
            c['note'] = note

    def steward_texts(spdx):
        texts = []
        for license_id in spdx_ids(spdx):
            texts.append((pin('spdx-' + license_id + '.txt', SPDX_DATA + license_id + '.txt'), None, license_id + '.txt'))
        return texts

    def add_crate(package, checksum, host):
        name, version = package['name'], package['version']
        spdx = (package.get('license') or '').replace('/', ' OR ')
        if not spdx:
            raise SystemExit('crate without license: ' + name)
        source = package['source'] or ''
        if source.startswith('registry+'):
            if not checksum:
                raise SystemExit('crate without checksum: ' + name)
            file = 'crate-' + name + '-' + version + '.crate'
            url = 'https://static.crates.io/crates/' + name + '/' + name + '-' + version + '.crate'
            pin(file, url)
            if pins[file]['sha256'] != checksum:
                raise SystemExit('crate checksum differs from Cargo.lock: ' + name)
        elif source.startswith('git+'):
            repo, rev = re.match(r'git\+(https://github\.com/[^?#]+?)(?:\.git)?(?:\?[^#]*)?#([0-9a-f]{40})$', source).groups()
            file = 'git-' + name + '-' + rev[:12] + '.tar.gz'
            pin(file, repo + '/archive/' + rev + '.tar.gz')
        else:
            raise SystemExit('unsupported crate source: ' + name + ' ' + source)
        members = top_members(cache / file)
        texts = [(file, members[k], k) for k in license_members(members)] or steward_texts(spdx)
        add('rust-crate-' + name + '-' + version, version, spdx, [host], texts, [file])

    with tempfile.TemporaryDirectory() as tmp:
        tmp = pathlib.Path(tmp)
        # uruntime's crates, from its own Cargo.lock.
        with tarfile.open(cache / 'uruntime-source.tar.gz') as t:
            t.extractall(tmp / 'uruntime', filter='data')
        uruntime = next((tmp / 'uruntime').iterdir())
        (uruntime / 'rust-toolchain.toml').unlink(missing_ok=True)
        for package, checksum in cargo_closure(uruntime, ['--locked', '--no-default-features', '--features', 'appimage,dwarfs,lite']):
            add_crate(package, checksum, 'uruntime')
        # sharun's crates, resolved as of its release run.
        with tarfile.open(cache / 'anylinux-sharun-source.tar.gz') as t:
            t.extractall(tmp / 'sharun', filter='data')
        sharun = next((tmp / 'sharun').iterdir())
        (sharun / 'rust-toolchain.toml').unlink(missing_ok=True)
        subprocess.run(['cargo', 'generate-lockfile', '-Zunstable-options', '--publish-time', SHARUN_RESOLVED_AT],
                       cwd=sharun, check=True, env={**os.environ, 'RUSTC_BOOTSTRAP': '1'})
        for package, checksum in cargo_closure(sharun, ['--locked']):
            add_crate(package, checksum, 'anylinux-sharun')

    # The Rust standard library, as built by -Zbuild-std from the toolchain's rust-src.
    for host, date in sorted(RUST_SRC.items()):
        url = 'https://static.rust-lang.org/dist/' + date + '/rust-src-nightly.tar.xz'
        file = pin('rust-src-nightly-' + date + '.tar.xz', url)
        with urllib.request.urlopen('https://static.rust-lang.org/dist/' + date + '/channel-rust-nightly.toml', timeout=120) as f:
            manifest = tomllib.loads(f.read().decode())
        if manifest['pkg']['rust-src']['target']['*']['xz_hash'] != pins[file]['sha256']:
            raise SystemExit('rust-src differs from the channel manifest: ' + date)
        members = top_members(cache / file)
        library = 'rust-src/lib/rustlib/src/rust/library/'
        add('rust-std-nightly-' + date, manifest['pkg']['rustc']['version'], 'MIT OR Apache-2.0', [host],
            [(file, members[k], k) for k in ('LICENSE-MIT', 'LICENSE-APACHE', 'COPYRIGHT')], [file],
            'core, alloc, std, std_detect, panic_abort, panic_unwind and unwind')
        add('rust-compiler-builtins-nightly-' + date, '0.1 (in-tree)', 'MIT AND Apache-2.0 WITH LLVM-exception AND (MIT OR Apache-2.0)',
            [host], [(file, members[library + 'compiler-builtins/LICENSE.txt'], 'LICENSE.txt')], [file])
        std_lock = tomllib.loads(tarfile.open(cache / file).extractfile(members[library + 'Cargo.lock']).read().decode())
        for p in std_lock['package']:
            if p['name'] not in STD_DEPS:
                continue
            crate = library + 'vendor/' + p['name'] + '-' + p['version'] + '/'
            manifest_toml = tomllib.loads(tarfile.open(cache / file).extractfile(members[crate + 'Cargo.toml']).read().decode())
            spdx = manifest_toml['package']['license'].replace('/', ' OR ')
            texts = [(file, members[k], k[len(crate):]) for k in sorted(members)
                     if k.startswith(crate) and '/' not in k[len(crate):] and LICENSE_NAME.match(k[len(crate):])] or steward_texts(spdx)
            add('rust-crate-' + p['name'] + '-' + p['version'], p['version'], spdx, [host], texts, [file])

    # Zig 0.16.0 links uruntime, sharun (and its preload libraries) and dash.
    zig = pin(*ZIG)
    zig_hosts = ['uruntime', 'anylinux-sharun', 'dash']
    add('zig-musl-1.2.5', '1.2.5 (Zig 0.16.0)', 'MIT', zig_hosts,
        [(zig, 'zig-0.16.0/lib/libc/musl/COPYRIGHT', 'COPYRIGHT')], [zig], 'the musl libc Zig 0.16.0 builds for -target x86_64-linux-musl')
    add('zig-compiler-rt-0.16.0', '0.16.0', 'MIT', zig_hosts, [(zig, 'zig-0.16.0/LICENSE', 'LICENSE')], [zig],
        "Zig's compiler_rt, linked into every Zig-linked binary")

    # dwarfs-fuse-extract's static libraries.
    build = pin(*DWARFS_BUILD)
    for name, version, spdx, url, file, licenses in DWARFS_LIBS:
        pin(file, url)
        members = top_members(cache / file)
        add('static-' + name + '-' + version, version, spdx, ['dwarfs'], [(file, members[m], m) for m in licenses], [file, build])
    header, text = (pin(*item) for item in NLOHMANN)
    add('static-nlohmann-json-3.12.0', '3.12.0', 'MIT', ['dwarfs'], [(text, None, 'LICENSE.MIT')], [header, text, build])

    # dash names GPL-2.0-or-later (mksignames.c's output) without its text.
    gpl2 = pin(*GPL2)
    for item in pins.values():
        if item['file'] in notices and notices[item['file']] != item:
            raise SystemExit('pin differs from the notices array: ' + item['file'])
    check_sharun_provenance(lock, components, cache)
    lock['static'] = sorted((i for i in pins.values() if i['file'] not in notices), key=lambda i: i['file'])
    LOCK.write_text(json.dumps(lock, indent=2) + '\n')
    OUT.write_text(json.dumps({'schema': 1, 'generated_by': 'packaging/appimage/closures.py', 'hosts': HOSTS,
                               'tool_texts': {'dash': [{'input': gpl2, 'member': None, 'as': 'GPL-2.0.txt'}]},
                               'components': dict(sorted(components.items()))}, indent=1) + '\n')
    print('Static closures: ' + str(len(components)) + ' components, ' + str(len(lock['static'])) + ' pinned inputs')


if __name__ == '__main__':
    main(sys.argv[1:])
