#!/usr/bin/env bash
# Phase 0 LAN measurements on the kernel bridge topology (items 1, 3, 4).
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
OUT=${OUT:-/tmp/p2p}
R=$OUT/results
mkdir -p "$R"
S=$HERE/services.sh

"$HERE/netns.sh" bridge
"$S" start 100

run() { local name=$1; shift; echo "== $name"; date +%s%6N > "$R/$name.start"; "$S" ctrl "$@" --log "$R/$name.jsonl" > "$R/$name.json" || echo "FAILED $name"; date +%s%6N > "$R/$name.end"; }

for via in plain udp pipe relay; do
  run "lan-rtt-$via" --via "$via" --test rtt --n 2000 --interval-ms 5
done
for via in plain udp pipe; do
  run "lan-tput20-$via" --via "$via" --test tput --mbps 20 --secs 60
done
for via in udp pipe; do
  run "lan-tput60x300-$via" --via "$via" --test tput --mbps 60 --secs 300
done
run "lan-tput60-relay" --via relay --test tput --mbps 60 --secs 60
for via in udp pipe; do
  run "lan-upgrade-$via" --via "$via" --test upgrade --n 30 --upgrade-timeout 30
done
for via in plain udp pipe relay; do
  run "lan-tputmax-$via" --via "$via" --test tput --mbps 2000 --secs 20
done
cp "$OUT/derp-counters.jsonl" "$R/lan-derp-counters.jsonl"
echo done
