#!/usr/bin/env bash
# r4 consent step (被控端确认) on the emulated WAN: relay 20 ms one way per
# peer, direct 1 ms (same as run-wan.sh upgrade).
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
OUT=${OUT:-/tmp/p2p}
BIN=${BIN:-/tmp/spikebin}
R=$OUT/results
N=${N:-30}
mkdir -p "$R"
S=$HERE/services.sh
wan() { "$BIN/wanemu" ctl --sock /tmp/wanemu.sock "$@" > /dev/null; }
run() {
  local name=$1; shift; echo "== $name"
  "$S" ctrl --test consent --derp-counters "$OUT/derp-counters.jsonl" --host-log "$OUT/host.jsonl" \
    "$@" --log "$R/$name.jsonl" > "$R/$name.json" || echo "FAILED $name"
}

EXTRA=0 "$HERE/netns.sh" wan; "$S" start 100
wan link relay host delay=20
wan link relay ctrl delay=20
wan link host ctrl delay=1

for via in udp pipe; do
  run "consent-ask-cold-$via"    --via $via --n "$N" --delay-ms 2000 --end stop
  run "consent-ask-prewarm-$via" --via $via --n "$N" --delay-ms 2000 --end stop --prewarm 1
  run "consent-never-$via"       --via $via --n "$N" --approve never --end kick
done
run consent-decline      --via udp --n 3 --decision decline --delay-ms 2000
run consent-nodirect     --via udp --n 3 --grant-direct 0 --delay-ms 1000 --end stop
run consent-ctrlbye      --via udp --n 3 --delay-ms 1000 --end bye
run consent-unanswered   --via udp --n 1 --decision none --run-timeout 120
cp "$OUT/derp-counters.jsonl" "$R/consent-derp-counters.jsonl"
echo done
