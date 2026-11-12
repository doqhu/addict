#!/usr/bin/env bash
# Tests of the release workflow, all local:
# - the guard scripts/release-ref.sh over the tag names the release must take
#   or refuse;
# - the workflows, read from the files: the release workflow starts only on
#   the agreed tag filter, CI's push starts only on branches, so no tag starts
#   it, the upload job runs only after the guard and CI passed and is the only
#   one that may write, and dry runs share one concurrency group;
# - scripts/release-upload.sh against a stub gh that keeps its releases in a
#   temporary directory and records its calls;
# - scripts/release-appimage-assets.sh on a small fixture handoff, and the
#   upload job's own two steps, read from release.yml, run on that fixture
#   against the stub gh.
# Needs python3 with PyYAML.
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
failures=0

pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; failures=$((failures + 1)); }

accepted=(v0.2.0 v0.1.1 v1.0.0 v10.20.300)
refused=(indexes-v3 v0.2.0-rc1 v1.2 xv0.2.0 '' v0.2.0x 'v0.2.0 ' 'v0.2.0
x' v.2.0 V0.2.0 refs/tags/v0.2.0)

for name in "${accepted[@]}"; do
    if "$root/scripts/release-ref.sh" "$name" 2>/dev/null; then
        pass "the guard accepts $(printf '%q' "$name")"
    else
        fail "the guard refuses $(printf '%q' "$name")"
    fi
done
for name in "${refused[@]}"; do
    if "$root/scripts/release-ref.sh" "$name" 2>/dev/null; then
        fail "the guard accepts $(printf '%q' "$name")"
    else
        pass "the guard refuses $(printf '%q' "$name")"
    fi
done

# Each line the Python check prints is PASS or FAIL with its case.
while IFS= read -r line; do
    case $line in
        PASS:*) pass "${line#PASS: }" ;;
        *) fail "${line#FAIL: }" ;;
    esac
done < <(python3 - "$root" <<'PY' || echo "FAIL: the workflow check stopped"
import pathlib, sys
import yaml

root = pathlib.Path(sys.argv[1])

def triggers(name):
    doc = yaml.safe_load((root / ".github/workflows" / name).read_text())
    # YAML 1.1 reads the key `on` as true.
    return doc.get("on", doc.get(True)) or {}

def check(ok, case):
    print(("PASS: " if ok else "FAIL: ") + case)

release = triggers("release.yml")
push = release.get("push") or {}
check(push == {"tags": ["v[0-9]+.[0-9]+.[0-9]+"]},
      "release.yml's push has only the tag filter v[0-9]+.[0-9]+.[0-9]+ (found %r)" % (push,))
check(set(release) == {"push", "workflow_dispatch"},
      "release.yml starts only on push and workflow_dispatch (found %r)" % (sorted(release),))

ci = triggers("ci.yml")
ci_push = ci.get("push") or {}
check(bool(ci_push.get("branches")) and not ({"tags", "tags-ignore"} & set(ci_push)),
      "ci.yml's push has a branch filter and no tag filter (found %r)" % (ci_push,))
check("workflow_call" in ci, "ci.yml can be called by the release workflow")

doc = yaml.safe_load((root / ".github/workflows/release.yml").read_text())
jobs = doc.get("jobs") or {}
upload = jobs.get("upload") or {}
needs = upload.get("needs") or []
needs = [needs] if isinstance(needs, str) else needs
check({"guard", "ci"} <= set(needs),
      "the upload job needs both guard and ci (found %r)" % (needs,))
check("if" not in upload,
      "the upload job has no if:, so a failed or skipped need skips it")
def writes(perms):
    if isinstance(perms, str):
        return perms == "write-all"
    return (perms or {}).get("contents") == "write"
writers = sorted(name for name, job in jobs.items() if writes(job.get("permissions")))
check(writers == ["upload"],
      "only the upload job has contents: write (found %r)" % (writers,))
check(not writes(doc.get("permissions")),
      "the workflow's default permissions do not write")
group = (doc.get("concurrency") or {}).get("group")
check(group == "release-${{ github.event_name == 'push' && github.ref || 'dry-run' }}",
      "a tag run's concurrency group is its ref, every dry run's is one group (found %r)" % (group,))
PY
)

# scripts/release-upload.sh with a stub gh. The stub keeps each release as
# a directory, $GH_STUB/releases/<name>, with a file `draft` (true or false)
# and its assets in `assets/`, and appends each call to $GH_STUB/calls. A file
# $GH_STUB/view-error or $GH_STUB/upload-error makes `release view` or
# `release upload` fail with its content.
tmp=$(mktemp -d)
trap 'rm -rf -- "$tmp"' EXIT
mkdir "$tmp/bin"
cat > "$tmp/bin/gh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
echo "$*" >> "$GH_STUB/calls"
[[ ${1:-} == release ]] || { echo "stub gh: unexpected call: $*" >&2; exit 99; }
command=$2 name=$3
dir=$GH_STUB/releases/$name
shift 3
case $command in
    view)
        if [[ -f $GH_STUB/view-error ]]; then cat "$GH_STUB/view-error" >&2; exit 1; fi
        [[ -d $dir ]] || { echo "release not found" >&2; exit 1; }
        case "$*" in
            '--json isDraft --jq .isDraft') cat "$dir/draft" ;;
            "--json assets --jq .assets[].name") ls -1 "$dir/assets" ;;
            *) echo "stub gh: unexpected view: $*" >&2; exit 99 ;;
        esac ;;
    create)
        [[ ! -d $dir ]] || { echo "stub gh: $name exists" >&2; exit 1; }
        mkdir -p "$dir/assets"
        if [[ " $* " == *" --draft "* ]]; then echo true > "$dir/draft"; else echo false > "$dir/draft"; fi ;;
    edit)
        [[ -d $dir ]] || { echo "release not found" >&2; exit 1; } ;;
    delete-asset)
        [[ -f $dir/assets/$1 ]] || { echo "asset not found" >&2; exit 1; }
        rm -- "$dir/assets/$1" ;;
    upload)
        if [[ -f $GH_STUB/upload-error ]]; then cat "$GH_STUB/upload-error" >&2; exit 1; fi
        [[ -d $dir ]] || { echo "release not found" >&2; exit 1; }
        clobber=false files=()
        for arg in "$@"; do
            if [[ $arg == --clobber ]]; then clobber=true; else files+=("$arg"); fi
        done
        for file in "${files[@]}"; do
            asset=$dir/assets/$(basename -- "$file")
            if [[ -e $asset && $clobber != true ]]; then echo "asset exists" >&2; exit 1; fi
            cp -- "$file" "$asset"
        done ;;
    *) echo "stub gh: unexpected call: release $command $*" >&2; exit 99 ;;
esac
STUB
chmod +x "$tmp/bin/gh"

mkdir "$tmp/new"
bundle=$tmp/new/Addict-0.2.0-x86_64.flatpak
checksum=$bundle.sha256
echo "new bundle" > "$bundle"
(cd "$tmp/new" && sha256sum -- "$(basename -- "$bundle")" > "$checksum")

# A fresh stub state; each argument NAME:DRAFT[:ASSET...] is a release.
stub() {
    export GH_STUB=$tmp/stub-$((++stubs))
    mkdir -p "$GH_STUB/releases"
    : > "$GH_STUB/calls"
    local spec name draft rest asset assets
    for spec in "$@"; do
        IFS=: read -r name draft rest <<< "$spec"
        mkdir -p "$GH_STUB/releases/$name/assets"
        echo "$draft" > "$GH_STUB/releases/$name/draft"
        IFS=: read -ra assets <<< "${rest:-}"
        for asset in "${assets[@]}"; do
            echo old > "$GH_STUB/releases/$name/assets/$asset"
        done
    done
}
stubs=0

# Runs the upload; its status in $code, its output in $out.
upload() {
    code=0
    out=$(PATH="$tmp/bin:$PATH" GITHUB_SHA=0123abc GITHUB_REF_NAME=topic \
        "$root/scripts/release-upload.sh" "$1" "$bundle" "$checksum" 2>&1) || code=$?
}
assets_of() { ls -1 "$GH_STUB/releases/$1/assets" | tr '\n' ' '; }
called() { grep -qE "$1" "$GH_STUB/calls"; }
expect() { if eval "$2"; then pass "$1"; else fail "$1"; echo "  output: $out"; echo "  calls:"; sed 's/^/    /' "$GH_STUB/calls"; fi; }
new_assets="Addict-0.2.0-x86_64.flatpak Addict-0.2.0-x86_64.flatpak.sha256 "
same() { cmp -s "$bundle" "$GH_STUB/releases/$1/assets/$(basename -- "$bundle")" && cmp -s "$checksum" "$GH_STUB/releases/$1/assets/$(basename -- "$checksum")"; }

# Real release: the tag's Release exists.
stub v0.2.0:false indexes-v3:false:manifest.json
upload v0.2.0
expect "real release: exit 0" '[[ $code == 0 ]]'
expect "real release: both assets on the tag's Release" '[[ $(assets_of v0.2.0) == "$new_assets" ]] && same v0.2.0'
expect "real release: the upload uses --clobber" 'called "^release upload v0\.2\.0 --clobber "'
expect "real release: nothing is created, edited or deleted" '! called "^release (create|edit|delete-asset)"'
expect "real release: indexes-v3 is untouched" '[[ $(assets_of indexes-v3) == "manifest.json " ]] && ! called " indexes-v3"'

# Second run: the assets are already there.
stub v0.2.0:false:Addict-0.2.0-x86_64.flatpak:Addict-0.2.0-x86_64.flatpak.sha256
upload v0.2.0
expect "second run: exit 0" '[[ $code == 0 ]]'
expect "second run: the assets are replaced" '[[ $(assets_of v0.2.0) == "$new_assets" ]] && same v0.2.0'

# Dry run with no draft yet.
stub v0.2.0:false:Addict-0.2.0-x86_64.flatpak indexes-v3:false:manifest.json
upload dry-run
expect "dry run, no draft: exit 0" '[[ $code == 0 ]]'
expect "dry run, no draft: a draft is created" 'called "^release create dry-run --draft " && [[ $(<"$GH_STUB/releases/dry-run/draft") == true ]]'
expect "dry run, no draft: the assets are on the draft" '[[ $(assets_of dry-run) == "$new_assets" ]] && same dry-run'
expect "dry run, no draft: no other release is touched" '! grep -v "^release [a-z-]* dry-run " "$GH_STUB/calls" | grep -q . && [[ $(<"$GH_STUB/releases/v0.2.0/assets/Addict-0.2.0-x86_64.flatpak") == old ]]'

# Second dry run, with a draft that holds an older bundle.
stub dry-run:true:Addict-0.1.1-x86_64.flatpak:Addict-0.1.1-x86_64.flatpak.sha256
upload dry-run
expect "dry run, older draft: exit 0" '[[ $code == 0 ]]'
expect "dry run, older draft: no second draft is created" '! called "^release create"'
expect "dry run, older draft: the older assets are deleted" 'called "^release delete-asset dry-run Addict-0\.1\.1-x86_64\.flatpak --yes" && called "^release delete-asset dry-run Addict-0\.1\.1-x86_64\.flatpak\.sha256 --yes"'
expect "dry run, older draft: only the new assets are on the draft" '[[ $(assets_of dry-run) == "$new_assets" ]] && same dry-run && [[ $(<"$GH_STUB/releases/dry-run/draft") == true ]]'

# A failed upload on a dry run leaves the draft as it was.
stub dry-run:true:Addict-0.1.1-x86_64.flatpak:Addict-0.1.1-x86_64.flatpak.sha256
echo "HTTP 502: Bad Gateway" > "$GH_STUB/upload-error"
upload dry-run
expect "dry run, failed upload: non-zero exit" '[[ $code != 0 ]]'
expect "dry run, failed upload: nothing deleted or edited" '! called "^release (delete-asset|edit)" && [[ $(assets_of dry-run) == "Addict-0.1.1-x86_64.flatpak Addict-0.1.1-x86_64.flatpak.sha256 " ]]'

# Release missing: a tag with no Release.
stub indexes-v3:false
upload v0.2.0
expect "release missing: non-zero exit" '[[ $code != 0 ]]'
expect "release missing: the message names the tag" '[[ $out == *"no GitHub Release for the tag v0.2.0"* ]]'
expect "release missing: no create or upload call" '! called "^release (create|upload|edit|delete-asset)"'

# A published dry-run release is refused.
stub dry-run:false:other.txt
upload dry-run
expect "published dry-run: non-zero exit and no upload" '[[ $code != 0 ]] && [[ $out == *"dry-run is published"* ]] && ! called "^release (create|upload|edit|delete-asset)"'

# An unexpected view error stops the script.
stub v0.2.0:false
echo "HTTP 502: Bad Gateway" > "$GH_STUB/view-error"
upload v0.2.0
expect "view error: non-zero exit, the error shown, no upload" '[[ $code != 0 ]] && [[ $out == *"HTTP 502"* ]] && ! called "^release (create|upload|edit|delete-asset)"'

# A name that is neither a tag nor dry-run calls no gh at all.
stub indexes-v3:false
upload indexes-v3
expect "other name: refused before any gh call" '[[ $code != 0 ]] && [[ ! -s $GH_STUB/calls ]]'

# Many files: one upload call per file, in the order given; a dry run keeps
# exactly the given names.
mkdir "$tmp/many"
many=()
for f in c.txt a.txt b.txt; do echo "$f" > "$tmp/many/$f"; many+=("$tmp/many/$f"); done
stub dry-run:true:old.txt:a.txt
code=0
out=$(PATH="$tmp/bin:$PATH" GITHUB_SHA=0123abc "$root/scripts/release-upload.sh" dry-run "${many[@]}" 2>&1) || code=$?
expect "many files: exit 0" '[[ $code == 0 ]]'
expect "many files: one upload call per file, in the order given" '[[ $(grep "^release upload" "$GH_STUB/calls" | sed "s|.*/||" | tr "\n" " ") == "c.txt a.txt b.txt " ]]'
expect "many files: the draft holds exactly the given names" '[[ $(assets_of dry-run) == "a.txt b.txt c.txt " ]] && [[ $(<"$GH_STUB/releases/dry-run/assets/a.txt") == a.txt ]]'
stub dry-run:true
code=0
out=$(PATH="$tmp/bin:$PATH" GITHUB_SHA=0123abc "$root/scripts/release-upload.sh" dry-run "$tmp/many/a.txt" "$tmp/many/a.txt" 2>&1) || code=$?
expect "many files: two files of one name are refused before any gh call" '[[ $code != 0 ]] && [[ $out == *"Two files named a.txt"* ]] && [[ ! -s $GH_STUB/calls ]]'

# scripts/release-appimage-assets.sh on a fixture handoff: the layout of
# scripts/archive-appimage.sh, with a tiny image, three sources and an
# inventory in which two components share one source.
version=0.2.0
image=Addict-$version-x86_64.AppImage
handoff_tar=Addict-$version-x86_64-handoff.tar
metadata=(build.txt inputs.lock.json rpms.txt layout.txt notices.json components.sha256)

# fixture DIR [MUTATION]: builds the handoff in DIR; MUTATION, a shell
# command run in the unpacked tree before the tar is made, changes it.
fixture() {
    local dir=$1 mutation=${2:-} tree
    tree=$(mktemp -d "$tmp/tree.XXXXXX")
    mkdir -p "$dir" "$tree/$image.sources"
    printf '#!/bin/sh\necho image\n' > "$tree/$image"
    chmod +x "$tree/$image"
    (cd "$tree" && sha256sum -- "$image" > "$image.sha256")
    local m
    for m in "${metadata[@]}"; do echo "$m of $image" > "$tree/$image.$m"; done
    echo "source a" > "$tree/$image.sources/a-1.0.src.rpm"
    echo "source b" > "$tree/$image.sources/b-2.0.tar.gz"
    echo "source c" > "$tree/$image.sources/c-3.0.src.rpm"
    (cd "$tree/$image.sources" && sha256sum -- * > SHA256SUMS)
    python3 - "$tree/$image.sources" "$tree/$image.notices.json" <<'PY2'
import hashlib, json, pathlib, sys
d = pathlib.Path(sys.argv[1])
item = lambda n: {'file': n, 'sha256': hashlib.sha256((d / n).read_bytes()).hexdigest(), 'url': 'https://example.org/' + n}
inventory = {'schema': 1, 'files': {}, 'components': {
    'a': {'source': {'files': [item('a-1.0.src.rpm')]}},
    'a-libs': {'source': {'files': [item('a-1.0.src.rpm')]}},
    'b': {'source': {'files': [item('b-2.0.tar.gz'), item('c-3.0.src.rpm')]}},
    'own': {'source': {}},
}}
pathlib.Path(sys.argv[2]).write_text(json.dumps(inventory, indent=1))
PY2
    if [[ -n $mutation ]]; then (cd "$tree" && eval "$mutation"); fi
    (cd "$tree" && tar -cf "$dir/$handoff_tar" -- *)
    (cd "$dir" && sha256sum -- "$handoff_tar" > "$handoff_tar.sha256")
    rm -rf -- "$tree"
}

# The upload job's steps, from release.yml, so the test runs the job's own
# order of assets.
python3 - "$root/.github/workflows/release.yml" "$tmp" <<'PY2'
import pathlib, sys, yaml
steps = yaml.safe_load(pathlib.Path(sys.argv[1]).read_text())['jobs']['upload']['steps']
named = {s.get('name'): s.get('run') for s in steps}
pathlib.Path(sys.argv[2], 'assets-step.sh').write_text(named['Check the handoff and write the AppImage assets'])
pathlib.Path(sys.argv[2], 'upload-step.sh').write_text(named['Upload to the release'])
PY2
mkdir "$tmp/bundle"
cp -- "$bundle" "$checksum" "$tmp/bundle/"

# publish RELEASE [MUTATION [TAR_MUTATION]]: a fresh fixture handoff, then
# the two steps of the upload job as Actions runs them, from the checkout,
# with RUNNER_TEMP. TAR_MUTATION, a shell command run in the handoff
# directory, changes the built tar or its checksum.
# Status in $code, output in $out.
publish() {
    runner=$(mktemp -d "$tmp/runner.XXXXXX")
    cp -r -- "$tmp/bundle" "$runner/bundle"
    fixture "$runner/handoff" "${2:-}"
    if [[ -n ${3:-} ]]; then (cd "$runner/handoff" && eval "$3"); fi
    code=0
    out=$( (cd "$root" && export PATH="$tmp/bin:$PATH" RUNNER_TEMP=$runner RELEASE=$1 \
        BUNDLE=$(basename -- "$bundle") IMAGE=$image GITHUB_SHA=0123abc ${PUBLISH_ENV:-} \
        && bash --noprofile --norc -eo pipefail "$tmp/assets-step.sh" \
        && bash --noprofile --norc -eo pipefail "$tmp/upload-step.sh") 2>&1) || code=$?
}
no_upload() { ! called "^release upload"; }

all_assets="Addict-0.2.0-x86_64.AppImage Addict-0.2.0-x86_64.AppImage.build.txt Addict-0.2.0-x86_64.AppImage.components.sha256 Addict-0.2.0-x86_64.AppImage.inputs.lock.json Addict-0.2.0-x86_64.AppImage.layout.txt Addict-0.2.0-x86_64.AppImage.notices.json Addict-0.2.0-x86_64.AppImage.rpms.txt Addict-0.2.0-x86_64.AppImage.sha256 Addict-0.2.0-x86_64.AppImage.sources.tar.00 Addict-0.2.0-x86_64.AppImage.sources.tar.01 Addict-0.2.0-x86_64.AppImage.sources.tar.02 Addict-0.2.0-x86_64.AppImage.sources.tar.sha256 Addict-0.2.0-x86_64.flatpak Addict-0.2.0-x86_64.flatpak.sha256 "

# Complete handoff. A part size of 4 KiB makes the fixture's source tar
# (10 KiB) three parts.
stub v0.2.0:false
PUBLISH_ENV="PART_SIZE=4096" publish v0.2.0
expect "complete handoff: exit 0" '[[ $code == 0 ]]'
expect "complete handoff: every asset on the Release" '[[ $(assets_of v0.2.0) == "$all_assets" ]]'
expect "complete handoff: the image and its checksum are uploaded last" '[[ $(grep "^release upload" "$GH_STUB/calls" | tail -2 | sed "s|.*/||" | tr "\n" " ") == "$image $image.sha256 " ]]'
expect "complete handoff: the Flatpak and the sources come before the image" '[[ $(grep "^release upload" "$GH_STUB/calls" | head -6 | sed "s|.*/||" | tr "\n" " ") == "Addict-0.2.0-x86_64.flatpak Addict-0.2.0-x86_64.flatpak.sha256 $image.sources.tar.00 $image.sources.tar.01 $image.sources.tar.02 $image.sources.tar.sha256 " ]]'
expect "complete handoff: every asset name is only A-Za-z0-9._- and under 2 GiB" '! ls -1 "$GH_STUB/releases/v0.2.0/assets" | grep -qv "^[A-Za-z0-9._-]*$" && [[ -z $(find "$GH_STUB/releases/v0.2.0/assets" -type f -size +2097151k) ]]'
reassembled=$tmp/reassembled
rm -rf -- "$reassembled"; mkdir "$reassembled"
expect "complete handoff: the parts pass their .sha256, and the image its own" '(cd "$GH_STUB/releases/v0.2.0/assets" && sha256sum --check --strict --quiet "$image.sources.tar.sha256" && sha256sum --check --strict --quiet "$image.sha256")'
expect "complete handoff: the reassembled source set passes SHA256SUMS with 3 files" '(cd "$GH_STUB/releases/v0.2.0/assets" && cat "$image".sources.tar.[0-9][0-9] | tar -x -C "$reassembled") && (cd "$reassembled/$image.sources" && sha256sum --check --strict --quiet SHA256SUMS && [[ $(ls | wc -l) == 4 ]])'
expect "complete handoff: the handoff tar and the extracted tree are removed" '[[ ! -e $runner/handoff/$handoff_tar ]] && [[ -z $(find "$runner" -maxdepth 1 -name ".appimage-assets.*") ]]'

# Second run: the assets are already on the Release.
PUBLISH_ENV="PART_SIZE=4096" publish v0.2.0
expect "second run with the AppImage: exit 0, the same assets once each" '[[ $code == 0 ]] && [[ $(assets_of v0.2.0) == "$all_assets" ]]'

# Dry run after one of another version: only this run's assets remain.
stub "dry-run:true:Addict-0.1.1-x86_64.AppImage:Addict-0.1.1-x86_64.AppImage.sources.tar.00:Addict-0.1.1-x86_64.flatpak"
PUBLISH_ENV="PART_SIZE=4096" publish dry-run
expect "dry run after an older one: only this run's assets remain" '[[ $code == 0 ]] && [[ $(assets_of dry-run) == "$all_assets" ]]'
PUBLISH_ENV="PART_SIZE=4096" publish dry-run
expect "second dry run: the same asset names once each" '[[ $code == 0 ]] && [[ $(assets_of dry-run) == "$all_assets" ]]'

# The refusals: each uploads nothing, and the asset directory is removed.
refuse() {
    expect "$1: non-zero exit" '[[ $code != 0 ]]'
    needle=$2
    expect "$1: the message names $2" '[[ $out == *"$needle"* ]]'
    expect "$1: the stub gh records no upload" 'no_upload'
    expect "$1: no asset directory is left" '[[ ! -e $runner/appimage ]]'
}
stub v0.2.0:false
publish v0.2.0 'rm "$image.sources/b-2.0.tar.gz"'
refuse "source removed" "missing corresponding source: b-2.0.tar.gz"
stub v0.2.0:false
publish v0.2.0 'rm "$image.sources/b-2.0.tar.gz"; sed -i "/b-2.0.tar.gz/d" "$image.sources/SHA256SUMS"'
refuse "source and its line removed" "differs from the inventory in $image.notices.json: missing b-2.0.tar.gz"
stub v0.2.0:false
publish v0.2.0 'echo extra > "$image.sources/d-4.0.src.rpm"; (cd "$image.sources" && sha256sum -- * | grep -v SHA256SUMS > SHA256SUMS.new && mv SHA256SUMS.new SHA256SUMS)'
refuse "source not in the inventory" "not in the inventory: d-4.0.src.rpm"
stub v0.2.0:false
publish v0.2.0 'printf X | dd of="$image.sources/c-3.0.src.rpm" bs=1 seek=2 conv=notrunc status=none'
refuse "corrupt source" "corrupt corresponding source: c-3.0.src.rpm"
stub v0.2.0:false
publish v0.2.0 'printf X | dd of="$image" bs=1 seek=3 conv=notrunc status=none'
refuse "corrupt image" "the image does not match its checksum: $image"
stub v0.2.0:false
publish v0.2.0 'rm "$image.layout.txt"'
refuse "metadata file missing" "missing from the handoff: $image.layout.txt"
stub v0.2.0:false
publish v0.2.0 '' 'printf X | dd of="$handoff_tar" bs=1 seek=600 conv=notrunc status=none'
refuse "corrupt handoff tar" "$handoff_tar does not match $handoff_tar.sha256"
expect "corrupt handoff tar: the tar is kept" '[[ -f $runner/handoff/$handoff_tar ]]'
# Asset too large, with the limit lowered below the image's size (the parts
# are 4 KiB, under it).
stub v0.2.0:false
PUBLISH_ENV="ASSET_LIMIT=4097 PART_SIZE=4096" publish v0.2.0 'head -c 5000 /dev/zero >> "$image"; sha256sum -- "$image" > "$image.sha256"'
refuse "asset too large" "not under 4097: $image"
stub v0.2.0:false
PUBLISH_ENV="ASSET_LIMIT=4097 PART_SIZE=4096" publish v0.2.0 'head -c 4096 /dev/zero > "$image"; sha256sum -- "$image" > "$image.sha256"'
expect "asset just under the limit: exit 0" '[[ $code == 0 ]]'

if [[ $failures != 0 ]]; then
    echo "$failures release test(s) failed" >&2
    exit 1
fi
echo "All release tests passed."
