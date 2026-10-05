#!/usr/bin/env bash
# One live EXP-4 session: cold activation, 30 warm hotkey activations, optional
# memory cycle and manual checks. Run it in a terminal of an unlocked session.
#   run-trial.sh <gtk4|gpui> <resident|on-demand> <foliate|intelli-j> [--memory] [--vulkan-drivers=GLOB]
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
backend=${1:-} mode=${2:-} origin=${3:-}
memory= vulkan=()
for option in "${@:4}"; do
  case $option in
    --memory) memory=--memory ;;
    # Hides other Vulkan drivers from the frontend, e.g. --vulkan-drivers='intel_icd*'.
    --vulkan-drivers=*) vulkan=(--vulkan-drivers "${option#*=}") ;;
    *) echo "unknown option $option" >&2; exit 2 ;;
  esac
done
exp="$root/target/release/addict-exp4"
out="$root/_bmad-output/initiative-linux-rust-translator/experiment-exp-4"
results="$out/results.jsonl"
certificate="$out/clock-certificate.json"
socket=/tmp/addict-exp4-session/broker.sock
case $backend in
  gtk4) frontend="$root/target/release/addict-ui-gtk" ;;
  gpui) frontend="$root/crates/addict-ui-gpui/target/release/addict-ui-gpui" ;;
  *) echo 'usage: run-trial.sh <gtk4|gpui> <resident|on-demand> <foliate|intelli-j> [--memory]' >&2; exit 2 ;;
esac
[[ $mode == resident || $mode == on-demand ]] || { echo 'mode: resident or on-demand' >&2; exit 2; }
[[ $origin == foliate || $origin == intelli-j ]] || { echo 'origin: foliate or intelli-j' >&2; exit 2; }
[[ -x $exp && -x $frontend ]] || { echo 'Release binaries missing; build them first' >&2; exit 1; }
[[ -f $certificate ]] || { echo 'Clock certificate missing' >&2; exit 1; }
[[ ! -e $socket ]] || { echo "Another session owns $socket" >&2; exit 1; }
# The certificate is bound to the boot it was measured on.
grep -q "\"$(cat /proc/sys/kernel/random/boot_id)\"" "$certificate" || { echo 'Clock certificate is from another boot; recalibrate' >&2; exit 1; }

broker=
cleanup() {
  if [[ -n $broker ]] && kill -0 "$broker" 2>/dev/null; then
    "$exp" quit --socket "$socket" >/dev/null 2>&1 || kill "$broker" 2>/dev/null || true
    wait "$broker" 2>/dev/null || true
  fi
  "$exp" uninstall-desktop --backend "$backend" || true
}
trap cleanup EXIT

# Prints "<cold presented> <warm presented> <busy> <latest acceptance ms or -> <certificate rejected>".
progress() {
  python3 - "$results" "$broker" <<'EOF'
import json, sys
cold = warm = busy = 0
latest = '-'
flags = {}
rejected = 0
for line in open(sys.argv[1]):
    row = json.loads(line)
    if row['event'] == 'clock-certificate-invalid':
        rejected += 1
    identifier = row.get('invocation_id') or ''
    if not identifier.startswith(sys.argv[2] + '-'):
        continue
    if row['event'] == 'invocation':
        flags[identifier] = (row['details']['cold'], row['origin'])
    elif row['event'] == 'invocation-busy':
        busy += 1
    elif row['event'] == 'presentation' and flags.get(identifier, (None, 'local'))[1] != 'local':
        if flags[identifier][0]:
            cold += 1
        else:
            warm += 1
        value = row['details'].get('acceptance_latency_ns')
        latest = '-' if value is None else f'{value / 1e6:.1f}'
print(cold, warm, busy, latest, rejected)
EOF
}
wait_for() { # <field index> <target> <label>
  local values
  while :; do
    kill -0 "$broker" 2>/dev/null || { echo; echo 'Broker exited; see the log above' >&2; exit 1; }
    read -r -a values <<<"$(progress)"
    printf '\r%s: %s/%s   last latency: %s ms   presses ignored while open: %s ' "$3" "${values[$1]}" "$2" "${values[3]}" "${values[2]}"
    (( values[$1] >= $2 )) && { echo; return; }
    sleep 0.3
  done
}
sample() { # <phase> <idle seconds>
  "$exp" sample --backend "$backend" --mode "$mode" --origin "$origin" --roots "$broker" \
    --phase "$1" --idle-seconds "$2" --inventory-complete --results "$results" >/dev/null
}

"$exp" install-desktop --backend "$backend"
rejected_before=$(grep -c '"clock-certificate-invalid"' "$results" || true)
"$exp" broker --backend "$backend" --mode "$mode" --origin "$origin" --frontend "$frontend" \
  --socket "$socket" --results "$results" --fixture "$root/experiments/exp4/ocr-fixture.png" \
  --clock-mapping "$certificate" --binding-timeout-seconds 60 --timeout-seconds 1800 "${vulkan[@]}" &
broker=$!
echo "== $backend / $mode / $origin ${vulkan[*]} (broker $broker) =="
echo 'Confirm the shortcut in the system dialog.'
until [[ -S $socket ]]; do kill -0 "$broker" 2>/dev/null || exit 1; sleep 0.1; done
[[ $(grep -c '"clock-certificate-invalid"' "$results" || true) == "$rejected_before" ]] || { echo 'Broker rejected the clock certificate' >&2; exit 1; }

echo "1. Focus $origin, press the hotkey once (cold start), check the window, close it with Esc."
wait_for 0 1 'cold activation'
if [[ -n $memory ]]; then
  echo 'Hands off for 60 s: idle memory sample before the warm trials.'
  sample before-invocation 60
fi
echo "2. From $origin: press the hotkey, wait for the window, Esc. Exactly 30 times, no extra presses."
wait_for 1 30 'warm activations'
echo 'Stop pressing. Close the window if it is open.'
if [[ -n $memory ]]; then
  echo 'Hands off for 60 s: idle memory sample after close.'
  sample after-close 60
  "$exp" invoke --socket "$socket" >/dev/null
  read -r -p '3. A window opened by local invoke: press "Run OCR", wait for the result, close with Esc, then Enter here. '
  echo 'Hands off for 60 s: idle memory sample after OCR.'
  sample after-ocr 60
  pgrep -x tesseract >/dev/null && echo 'WARNING: a tesseract process is still alive' >&2
fi

echo "4. Manual checks for $backend / $mode from $origin. Reopen the window with the hotkey-free command if needed:"
echo "     $exp invoke --socket $socket"
declare -A hints=(
  [focus]='The window takes keyboard focus as soon as it appears; typing goes into it.'
  [esc_close_reopen]='Esc closes the window and the hotkey reopens it, several times in a row.'
  [english_russian_editing_selection]='Type English and Russian text; caret movement, selection and deletion behave.'
  [paste_copy]='Ctrl+V pastes into the input; text copied from the result pastes elsewhere.'
  [scaling]='At the current display scale the text is sharp and nothing is clipped. Not about resizing the window.'
  [delayed_work]='Press the delayed-work button: the input stays editable for the 5 s, then the status changes.'
  [ocr_cleanup]='Press "Run OCR", wait for the text; then "pgrep -x tesseract" in another terminal prints nothing.'
)
checks=$(mktemp)
{
  printf '{'
  for check in focus esc_close_reopen english_russian_editing_selection paste_copy scaling delayed_work ocr_cleanup; do
    echo "   ${hints[$check]}" >/dev/tty
    read -r -p "   $check ok? [y/n/s=skip] " answer </dev/tty
    case $answer in y) printf '"%s":true,' "$check" ;; n) printf '"%s":false,' "$check" ;; esac
  done
  read -r -p '   notes: ' notes </dev/tty
  python3 -c 'import json,sys; print("\"notes\":"+json.dumps(sys.argv[1])+"}")' "$notes"
} >"$checks"
"$exp" human-scenarios --backend "$backend" --mode "$mode" --origin "$origin" --checks "$checks" --results "$results" \
  || echo 'Manual checks were not recorded (skipped answers leave them pending).' >&2
rm -f "$checks"
echo 'Session finished; quitting the broker and removing the experiment desktop entry.'
