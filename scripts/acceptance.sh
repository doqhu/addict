#!/usr/bin/env bash
# Acceptance measurements of the product, by the protocol of acceptance.md.
# Run it in a terminal of an unlocked Wayland session and follow the prompts.
#   acceptance.sh calibrate                  ten presses -> clock certificate of this boot
#   acceptance.sh <foliate|android-studio>   one session: cold press, 60 s idle sample,
#                                            30 warm presses, 60 s idle sample, GPU record, summary
# Exit 0 only when the certificate was written or the session's row is `pass`.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
action=${1:-}
app="$root/target/release/addict"
exp="$root/target/release/addict-exp4"
out="$root/_bmad-output/initiative-linux-rust-translator/acceptance-resident-hotkey-window"
results="$out/results.jsonl"
calibration="$out/calibration.jsonl"
boot=$(cat /proc/sys/kernel/random/boot_id)
# A certificate speaks for one boot: each boot keeps its own, with its evidence text.
clock="$out/clock-$boot"
certificate="$clock/clock-certificate.json"
entry="${XDG_DATA_HOME:-$HOME/.local/share}/applications/dev.addict.Addict.desktop"
build='cargo build --release -p addict-app -p addict-exp4'

case $action in
  calibrate) origin=local name='any application' file=$calibration ;;
  foliate) origin=foliate name='Foliate' file=$results ;;
  android-studio) origin=android-studio name='Android Studio' file=$results ;;
  *) echo 'usage: acceptance.sh <calibrate|foliate|android-studio>' >&2; exit 2 ;;
esac

# Release build of the current sources: a stale binary would measure another program.
(cd "$root" && $build --quiet) || { echo "Release build failed: $build" >&2; exit 1; }
[[ -x $app && -x $exp ]] || { echo "Release binaries missing after: $build" >&2; exit 1; }
[[ -f $entry ]] || { echo "Desktop entry $entry missing; the portal needs it: run '$app install'" >&2; exit 1; }
if pgrep -x addict >/dev/null; then
  echo "addict is already running (pid $(pgrep -x addict | tr '\n' ' ')); stop it first" >&2
  exit 1
fi
if [[ $action != calibrate ]]; then
  [[ -f $certificate ]] \
    || { echo 'No clock certificate of this boot; run: scripts/acceptance.sh calibrate' >&2; exit 1; }
  # The validation the final summary applies (boot, versions, evidence hash), before any press.
  "$exp" summarize --product --clock-mapping "$certificate" --results /dev/null >/dev/null \
    || { echo 'The clock certificate is not valid now; run: scripts/acceptance.sh calibrate' >&2; exit 1; }
fi
mkdir -p "$out"

pid=
cleanup() {
  if [[ -n $pid ]] && kill -0 "$pid" 2>/dev/null; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT

# Prints "<session started> <cold frames> <warm frames> <frames without a time> <presses>"
# of the process this script started.
progress() {
  python3 - "$file" "$pid" <<'EOF'
import json, os, sys
started = cold = warm = untimed = presses = 0
flags = {}
if os.path.exists(sys.argv[1]):
    for line in open(sys.argv[1]):
        if not line.strip():
            continue
        row = json.loads(line)
        if row['event'] == 'broker-started' and str(row['details'].get('pid')) == sys.argv[2]:
            started = 1
        identifier = row.get('invocation_id') or ''
        if not identifier.startswith(sys.argv[2] + '-'):
            continue
        if row['event'] == 'invocation':
            flags[identifier] = row['details']['cold']
            presses += 1
        elif row['event'] == 'presentation' and identifier in flags:
            if flags[identifier]:
                cold += 1
            else:
                warm += 1
            if row['details'].get('time_ns') is None:
                untimed += 1
print(started, cold, warm, untimed, presses)
EOF
}
alive() { kill -0 "$pid" 2>/dev/null || { echo; echo 'addict exited; see its message above' >&2; exit 1; }; }
wait_for() { # <label> <target> <field index>...: waits until the sum of the fields reaches the target
  local values total field
  while :; do
    alive
    read -r -a values <<<"$(progress)"
    total=0
    for field in "${@:3}"; do total=$((total + values[field])); done
    printf '\r%s: %s/%s   frames without a time: %s ' "$1" "$total" "$2" "${values[3]}"
    (( total >= $2 )) && { echo; return; }
    sleep 0.3
  done
}
presses() { local values; read -r -a values <<<"$(progress)"; echo "${values[4]}"; }
sample() { # <phase>: 60 s idle, then PSS, RSS and CPU of the process over one second
  local before
  read -r -p 'Make sure the window is closed (Esc), then press Enter here. '
  before=$(presses)
  echo "Hands off for 60 s: idle sample ($1)."
  "$exp" sample --backend gtk4 --mode resident --origin "$origin" --roots "$pid" \
    --phase "$1" --idle-seconds 60 --inventory-complete --results "$results" >/dev/null
  alive
  # A press during the wait means the process was not idle: the sample is not one.
  [[ $(presses) == "$before" ]] || { echo 'A press arrived during the idle wait; repeat the session' >&2; exit 1; }
}

"$app" --evidence "$file" --origin "$origin" &
pid=$!
echo "== $action (addict $pid, boot $boot) =="
echo 'If a system dialog asks for the shortcut, confirm it.'
until read -r -a state <<<"$(progress)" && (( state[0] == 1 )); do alive; sleep 0.1; done

if [[ $action == calibrate ]]; then
  echo 'Press Super+Q, wait for the window, Esc. Ten times.'
  wait_for 'presses with a frame' 10 1 2
  echo 'Stop pressing.'
  "$exp" calibrate --results "$calibration" --out "$clock"
  echo "Certificate of boot $boot written."
  exit 0
fi

echo "1. Focus $name, press Super+Q once (cold start), check the window, close it with Esc."
echo '   If the bind dialog was dismissed or the window appeared by itself, stop with Ctrl+C and start again.'
wait_for 'cold activation' 1 1
sample before-invocation
echo "2. From $name: Super+Q, wait for the window, Esc. Exactly 30 times, no extra presses."
wait_for 'warm activations' 30 2
echo 'Stop pressing.'
sample after-close
"$exp" gpu --results "$results" --roots "$pid"

summary=$("$exp" summarize --product --results "$results" --clock-mapping "$certificate")
python3 -c '
import json, sys
row = next(r for r in json.load(sys.stdin) if r["origin"] == sys.argv[1])
text = json.dumps(row, indent=2, ensure_ascii=False)
open(sys.argv[2], "w").write(text + "\n")
print(text)
print("== {}: {} ==".format(sys.argv[1], row["acceptance"]))
sys.exit(0 if row["acceptance"] == "pass" else 1)
' "$origin" "$out/summary-$origin-$boot.json" <<<"$summary"
