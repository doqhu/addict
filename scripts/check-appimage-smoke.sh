#!/usr/bin/env bash
# Disposable X11 production OCR, with no host desktop or network at run time.
set -Eeuo pipefail
if [[ ${1:-} == --inside ]]; then
    export HOME=/tmp/home XDG_RUNTIME_DIR=/tmp/runtime
    mkdir -m700 "$HOME" "$XDG_RUNTIME_DIR"
    # Fail explicitly if the runner's ptrace policy defeats the evidence harness.
    strace -o /tmp/ptrace.log /bin/true
    # Keep xvfb-run below PID 1: Xvfb signals its parent when ready, and a
    # namespace's PID 1 handles that readiness signal differently.
    xvfb-run -a --server-args='-screen 0 1280x800x24 -nolisten tcp' \
        python3 /checkout/packaging/appimage/ocr.py /artifact/image.AppImage /diagnostics/ocr
    exit 0
fi
[[ $# == 2 ]] || { echo "usage: $0 IMAGE NEW_DIAGNOSTICS_DIR" >&2; exit 1; }
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
image=$(realpath -- "$1")
[[ -f $image && -x $image ]] || { echo "missing executable artifact: $image" >&2; exit 1; }
out=$(realpath -m -- "$2")
[[ ! -e $out ]] || { echo "diagnostics directory already exists: $out" >&2; exit 1; }
mkdir -p -- "$out"
container=addict-smoke-$$
trap 'podman rm -f "$container" >/dev/null 2>&1 || true' EXIT
smoke=localhost/addict-appimage-smoke:$(sha256sum "$root/packaging/appimage/smoke.Containerfile" | cut -c1-16)
podman build --pull=missing -t "$smoke" -f "$root/packaging/appimage/smoke.Containerfile" "$root/packaging/appimage" \
    > "$out/container-build.log" 2>&1
# Private namespaces, no host sockets/devices, and only diagnostics are persistent
# writes. SYS_PTRACE permits strace and /proc evidence within this private PID ns;
# the default seccomp policy remains enabled. Never use --privileged.
timeout --signal=TERM --kill-after=15 900 podman run --rm --name "$container" \
    --network=none --read-only --pid=private --ipc=private --pids-limit=512 \
    --userns=keep-id --user "$(id -u):$(id -g)" --cap-drop=all --cap-add=SYS_PTRACE \
    --security-opt label=disable --tmpfs /tmp:rw,exec,size=2g --shm-size=256m \
    -v "$root:/checkout:ro" -v "$image:/artifact/image.AppImage:ro" -v "$out:/diagnostics:rw" \
    "$smoke" > "$out/container.log" 2>&1
[[ -s $out/ocr/PASS.json ]] || { echo "missing OCR success evidence: $out" >&2; exit 1; }
echo "PASS: isolated Ubuntu 24.04 no-FUSE OCR; diagnostics: $out"
