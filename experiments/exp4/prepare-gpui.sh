#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
checkout="$root/experiments/exp4/upstream"
pin=a84689073d296dfd39987bc7dd478e43ef76d83a
patch="$root/experiments/exp4/gpui-exp4.patch"
if [[ ! -e "$checkout" ]]; then
  git clone --filter=blob:none --no-checkout https://github.com/zed-industries/zed.git "$checkout"
  git -C "$checkout" checkout --detach "$pin"
fi
[[ -d "$checkout/.git" ]] || { echo 'Refusing an unrelated upstream directory' >&2; exit 1; }
[[ $(git -C "$checkout" remote get-url origin) == https://github.com/zed-industries/zed.git ]] || { echo 'Unexpected upstream remote' >&2; exit 1; }
[[ $(git -C "$checkout" rev-parse HEAD) == "$pin" ]] || { echo 'Unexpected upstream revision; refusing to reset local work' >&2; exit 1; }
if git -C "$checkout" diff --quiet && [[ -z $(git -C "$checkout" ls-files --others --exclude-standard) ]]; then
  git -C "$checkout" apply --check "$patch"
  git -C "$checkout" apply "$patch"
else
  # Idempotence requires exactly this patch, not merely a reverse-applicable subset.
  existing=$(mktemp)
  trap 'rm -f "$existing"' EXIT
  git -C "$checkout" diff --binary > "$existing"
  cmp -s "$patch" "$existing" || { echo 'Upstream contains unrelated changes; refusing to alter it' >&2; exit 1; }
  [[ -z $(git -C "$checkout" ls-files --others --exclude-standard) ]] || { echo 'Upstream has untracked files; refusing to alter it' >&2; exit 1; }
fi
printf 'GPUI revision: %s\n' "$pin"
sha256sum "$patch"
