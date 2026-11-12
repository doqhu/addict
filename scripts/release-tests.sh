#!/usr/bin/env bash
# Tests of the release workflow, all local:
# - the guard scripts/release-ref.sh over the tag names the release must take
#   or refuse;
# - the workflows, read from the files: the release workflow starts only on
#   the agreed tag filter, CI's push starts only on branches, so no tag starts
#   it, the upload job runs only after the guard and CI passed and is the only
#   one that may write, and dry runs share one concurrency group;
# - scripts/release-upload.sh against a stub gh that keeps its releases in a
#   temporary directory and records its calls.
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

if [[ $failures != 0 ]]; then
    echo "$failures release test(s) failed" >&2
    exit 1
fi
echo "All release tests passed."
