#!/usr/bin/env bash
# Phase 0 measurements on the emulated WAN (items 3, 4, 9 and the soak runs).
#   run-wan.sh fence|upgrade|cut|soak
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
OUT=${OUT:-/tmp/p2p}
BIN=${BIN:-/tmp/spikebin}
R=$OUT/results
mkdir -p "$R"
S=$HERE/services.sh
wan() { "$BIN/wanemu" ctl --sock /tmp/wanemu.sock "$@" > /dev/null; }
run() { local name=$1; shift; echo "== $name"; date +%s%6N > "$R/$name.start"; "$S" ctrl "$@" --log "$R/$name.jsonl" > "$R/$name.json" || echo "FAILED $name"; date +%s%6N > "$R/$name.end"; }

# relay_ms: one-way delay between each peer and the relay node (DERP + airhand relay)
# direct: wanemu link args for host<->ctrl
setup() {
  local relay_ms=$1; shift
  wan link relay host delay="$relay_ms"
  wan link relay ctrl delay="$relay_ms"
  wan link host ctrl "$@"
}

phase=${1:?fence|upgrade|cut|soak}
case $phase in
fence)
  EXTRA=0 "$HERE/netns.sh" wan; "$S" start 100
  for ms in 5 20 50 100 200; do
    setup "$ms" delay=1
    run "wan-relayrtt-$ms" --via relay --test rtt --n 200 --interval-ms 10
    run "wan-fence-$ms" --via udp --test fence --n 200 --interval-ms 20
  done
  ;;
upgrade)
  EXTRA=0 "$HERE/netns.sh" wan; "$S" start 100
  setup 20 delay=1
  for via in udp pipe; do
    run "wan-upgrade-$via" --via "$via" --test upgrade --n 30 --upgrade-timeout 30
  done
  ;;
cut)
  EXTRA=0 "$HERE/netns.sh" wan; "$S" start 100
  setup 20 delay=1
  for i in 1 2 3; do
    for via in udp pipe; do
      name="${CUT:-wan-cut}-$via-$i"
      ( sleep 15; wan cut host ctrl; sleep 20; wan restore host ctrl ) &
      run "$name" --via "$via" --test hold --secs 50 --mbps 5 --ping-ms 50
      wait
    done
  done
  cp /tmp/wanemu.log "$R/${CUT:-wan-cut}-wanemu.log"
  cp "$OUT/derp-counters.jsonl" "$R/${CUT:-wan-cut}-derp-counters.jsonl"
  cp "$OUT/host.jsonl" "$R/${CUT:-wan-cut}-host.jsonl"
  ;;
soak)
  EXTRA=1 "$HERE/netns.sh" wan; "$S" start 100; "$S" start2 100
  SECS=${SECS:-1800}
  # Pair 1: stable direct path at 400 ms RTT (path-misjudgement threshold).
  setup 20 delay=200
  # Pair 2: Wi-Fi-like access links: jitter + 1% loss on every link.
  wan link relay host2 delay=20 jitter=15 loss=1
  wan link relay ctrl2 delay=20 jitter=15 loss=1
  wan link host2 ctrl2 delay=3 jitter=15 loss=1
  date +%s%6N > "$R/wan-soak.start"
  "$S" ctrl --via udp --test soak --secs "$SECS" --ping-ms "${PING_MS:-250}" --k-ms "${K_MS:-2000}" --log "$R/wan-soak-rtt400.jsonl" > "$R/wan-soak-rtt400.json" &
  "$S" ctrl2 --via udp --test soak --secs "$SECS" --ping-ms "${PING_MS:-250}" --k-ms "${K_MS:-2000}" --log "$R/wan-soak-wifi.jsonl" > "$R/wan-soak-wifi.json" &
  wait
  date +%s%6N > "$R/wan-soak.end"
  ;;
esac
echo done
