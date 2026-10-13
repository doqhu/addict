#!/usr/bin/env bash
# Acceptance measurements of the product, by the protocol of acceptance.md.
# Run it in a terminal of an unlocked Wayland session and follow the prompts.
#   acceptance.sh calibrate                  ten presses -> clock certificate of this boot
#   acceptance.sh <foliate|android-studio>   one session: cold press, 60 s idle sample,
#                                            30 warm presses, 30 lookups, online requests of
#                                            a word, of a sentence and of a definition,
#                                            60 s idle sample, GPU record, online digest, summary
# A session is of the product with both dictionary indexes imported and records them; it
# sends text to DeepL and does not start without the key file. Calibration sends nothing.
# Exit 0 only when the certificate was written or the session's row is `pass`.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
action=${1:-}
app="$root/target/release/addict"
exp="$root/target/release/addict-exp4"
out="$root/_bmad-output/initiative-linux-rust-translator/acceptance-online-contextual-translation"
results="$out/results.jsonl"
calibration="$out/calibration.jsonl"
boot=$(cat /proc/sys/kernel/random/boot_id)
# A certificate speaks for one boot: each boot keeps its own, with its evidence text.
clock="$out/clock-$boot"
certificate="$clock/clock-certificate.json"
build='cargo build --release -p addict-app -p addict-exp4'
# The data directory as the application finds it: a relative XDG_DATA_HOME is ignored.
data=${XDG_DATA_HOME:-}
[[ $data == /* ]] || data=$HOME/.local/share
entry="$data/applications/dev.addict.Addict.desktop"
# Each index with the published file it is imported from.
indexes=("$data/addict/oewn.index" "$data/addict/freedict.index")
published=(english-wordnet-2025.xml.gz freedict-eng-rus-2025.11.23.src.tar.xz)
lookups=30
# The key file as the application finds it: a relative XDG_CONFIG_HOME is ignored.
config=${XDG_CONFIG_HOME:-}
[[ $config == /* ]] || config=$HOME/.config
key="$config/addict/deepl.key"

case $action in
  calibrate) origin=local name='any application' file=$calibration ;;
  foliate) origin=foliate name='Foliate' file=$results ;;
  android-studio) origin=android-studio name='Android Studio' file=$results ;;
  *) echo 'usage: acceptance.sh <calibrate|foliate|android-studio>' >&2; exit 2 ;;
esac

# A session without both indexes would measure another state of the program.
if [[ $action != calibrate ]]; then
  missing=0
  for i in "${!indexes[@]}"; do
    [[ -f ${indexes[i]} ]] && continue
    echo "Index ${indexes[i]} missing; import it: $app import <path to ${published[i]}>" >&2
    missing=1
  done
  (( missing == 0 )) || exit 1
  # Without the key every online request is refused before it is sent: nothing to measure.
  if [[ ! -f $key ]]; then
    echo "Key file $key missing; write your DeepL API key into it as its only line and run: chmod 600 $key" >&2
    exit 1
  fi
fi

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

# Prints "<session started> <cold frames> <warm frames> <frames without a time> <presses>
# <lookups> <online: of a word> <of a sentence> <of a definition>" of the process this
# script started.
progress() {
  python3 - "$file" "$pid" <<'EOF'
import json, os, sys
started = cold = warm = untimed = presses = lookups = 0
online = {'word': 0, 'sentence': 0, 'definition': 0}
mine = False
flags = {}
if os.path.exists(sys.argv[1]):
    for line in open(sys.argv[1]):
        if not line.strip():
            continue
        row = json.loads(line)
        if row['event'] == 'broker-started':
            # A lookup or an online request names no press: it is of the session it follows.
            mine = str(row['details'].get('pid')) == sys.argv[2]
            if mine:
                started = 1
                lookups = 0
                online = dict.fromkeys(online, 0)
        elif row['event'] == 'lookup' and mine:
            lookups += 1
        elif row['event'] == 'online' and mine and row['details'].get('subject') in online:
            online[row['details']['subject']] += 1
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
print(started, cold, warm, untimed, presses, lookups, *online.values())
EOF
}
alive() { kill -0 "$pid" 2>/dev/null || { echo; echo 'addict exited; see its message above' >&2; exit 1; }; }
base=0 # what the sum already was when the step began
wait_for() { # <label> <target> <field index>...: waits until the sum of the fields, less $base, reaches the target
  local values total field
  while :; do
    alive
    read -r -a values <<<"$(progress)"
    total=$((-base))
    for field in "${@:3}"; do total=$((total + values[field])); done
    printf '\r%s: %s/%s   frames without a time: %s ' "$1" "$total" "$2" "${values[3]}"
    (( total >= $2 )) && { echo; return; }
    sleep 0.3
  done
}
presses() { local values; read -r -a values <<<"$(progress)"; echo "${values[4]}"; }
looked() { local values; read -r -a values <<<"$(progress)"; echo "${values[5]}"; }
asked() { local values; read -r -a values <<<"$(progress)"; echo "${values[@]:6:3}"; }
wait_online() { # waits until, since the step began, a request of each kind has ended
  local values first word sentence definition
  # A request of the last click of the step before, still pending, would be counted here.
  read -r -p 'Wait until every online line in the window has its answer, then press Enter here. '
  read -r -a first <<<"$(asked)"
  while :; do
    alive
    read -r -a values <<<"$(asked)"
    word=$((values[0] - first[0])) sentence=$((values[1] - first[1])) definition=$((values[2] - first[2]))
    printf '\ronline requests ended: word %s, sentence %s, definition %s ' "$word" "$sentence" "$definition"
    (( word >= 1 && sentence >= 1 && definition >= 1 )) && { echo; return; }
    sleep 0.3
  done
}
sample() { # <phase>: 60 s idle, then PSS, RSS and CPU of the process over one second
  local before
  read -r -p 'Make sure the window is closed (Esc), then press Enter here. '
  before="$(presses) $(looked) $(asked)"
  echo "Hands off for 60 s: idle sample ($1)."
  "$exp" sample --backend gtk4 --mode resident --origin "$origin" --roots "$pid" \
    --phase "$1" --idle-seconds 60 --inventory-complete --results "$results" >/dev/null
  alive
  # A press, a lookup or an online request during the wait means the process was not idle:
  # the sample is not one.
  [[ "$(presses) $(looked) $(asked)" == "$before" ]] \
    || { echo 'A press, a lookup or an online request arrived during the idle wait; repeat the session' >&2; exit 1; }
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

# What this session measures with. Uncommitted sources are said: the build is of them.
commit=$(git -C "$root" rev-parse HEAD)
[[ -z $(git -C "$root" status --porcelain) ]] || commit+=-dirty
"$exp" data-sets --results "$results" --roots "$pid" --commit "$commit" --files "${indexes[@]}"

echo "1. Focus $name, press Super+Q once (cold start), check the window, close it with Esc."
echo '   If the bind dialog was dismissed or the window appeared by itself, stop with Ctrl+C and start again.'
wait_for 'cold activation' 1 1
sample before-invocation
echo "2. From $name: Super+Q, wait for the window, Esc. Exactly 30 times, no extra presses."
wait_for 'warm activations' 30 2
echo 'Stop pressing.'
echo "3. Super+Q once more (this press is measured too, as an extra attempt), paste a sentence into"
echo "   the field and click its words, one after another, waiting for each result, until $lookups lookups"
echo "   are counted. The window stays open meanwhile."
# Only a lookup after the 30 presses counts.
base=$(looked)
wait_for 'lookups' "$lookups" 5
base=0
echo 'Stop clicking.'
echo '4. Online requests, in the same window. First, as asked below, let the requests of step 3 end.'
echo '   After Enter: paste another sentence, click a word of it and wait for its online lines, of'
echo '   the word and of the sentence. Then click a sense of the article and press'
echo '   "Translate definition into Russian". A refusal counts as an answer.'
wait_online
echo 'Stop clicking and wait until every online line has its answer.'
sample after-close
"$exp" gpu --results "$results" --roots "$pid"

# A reading aid of the online records of this session; the evidence file is the record.
# Its failure is said and changes nothing: the summary below is what the session is judged by.
python3 - "$results" "$pid" <<'EOF' | tee "$out/online-$origin-$boot-$pid.txt" \
  || echo 'The digest of the online records failed; the evidence file still holds them' >&2
import json, statistics, sys
mine = False
rows = []
for line in open(sys.argv[1]):
    if not line.strip():
        continue
    row = json.loads(line)
    if row['event'] == 'broker-started':
        mine = str(row['details'].get('pid')) == sys.argv[2]
        if mine:
            rows = []
    elif row['event'] == 'online' and mine:
        rows.append(row['details'])
megabytes = lambda value: '-' if value is None else '{:.1f}'.format(value / 1e6)
layout = '{:<11} {:<21} {:>10} {:>12} {:>8}'
print('== online requests: {} =='.format(len(rows)))
print(layout.format('kind', 'outcome', 'elapsed ms', 'peak RSS MB', 'PSS MB'))
for d in rows:
    print(layout.format(d['subject'], d['outcome'], d['elapsed_ms'],
                        megabytes(d['peak_rss_bytes']), megabytes(d['pss_bytes'])))
for kind in ('word', 'sentence', 'definition'):
    of_kind = [d for d in rows if d['subject'] == kind]
    times = sorted(d['elapsed_ms'] for d in of_kind if d['outcome'] == 'translated')
    spread = 'no translation'
    if times:
        spread = 'translated in {}..{} ms, median {:g}'.format(times[0], times[-1], statistics.median(times))
    print('{}: {} ended, {} translated; {}'.format(kind, len(of_kind), len(times), spread))
for field, name in (('peak_rss_bytes', 'peak RSS'), ('pss_bytes', 'PSS')):
    values = [d[field] for d in rows if d[field] is not None]
    print('highest {}: {} MB'.format(name, megabytes(max(values, default=None))))
EOF

summary=$("$exp" summarize --product --results "$results" --clock-mapping "$certificate")
python3 -c '
import json, sys
row = next(r for r in json.load(sys.stdin) if r["origin"] == sys.argv[1])
text = json.dumps(row, indent=2, ensure_ascii=False)
open(sys.argv[2], "w").write(text + "\n")
print(text)
print("== {}: {} ==".format(sys.argv[1], row["acceptance"]))
sys.exit(0 if row["acceptance"] == "pass" else 1)
' "$origin" "$out/summary-$origin-$boot-$pid.json" <<<"$summary"
