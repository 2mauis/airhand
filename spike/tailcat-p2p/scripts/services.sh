#!/usr/bin/env bash
# Starts the spike services inside the namespaces from netns.sh, as the
# invoking (non-root) user:
#   relay ns: localderp (DERP+STUN) and the real airhand relay server
#   host ns:  p2p-bench host (spawns one helper per session/epoch)
#
#   services.sh start [poll_ms] | stop | ctrl <p2p-bench ctrl args...>
set -euo pipefail
HERE=$(cd "$(dirname "$0")/.." && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
BIN=${BIN:-/tmp/spikebin}
OUT=${OUT:-/tmp/p2p}
BENCH=$HERE/bench/target/release/p2p-bench
SERVER=$ROOT/target/release/airhand-server
ME=$(id -un)

as_user() { local ns=$1; shift; sudo ip netns exec "$ns" sudo -u "$ME" env P2P_HELPER="$BIN/airhand-p2p-spike" ${P2P_HELPER_STDERR_DIR:+P2P_HELPER_STDERR_DIR=$P2P_HELPER_STDERR_DIR} "$@"; }

stop() {
  pkill -f "p2p-bench host" 2>/dev/null || true
  pkill -f "$BIN/localderp" 2>/dev/null || true
  pkill -f "airhand-server" 2>/dev/null || true
  pkill -f "airhand-p2p-spike" 2>/dev/null || true
  sleep 0.5
}

start() {
  local poll=${1:-100}
  stop
  mkdir -p "$OUT/relay-data"
  as_user ah-relay "$BIN/localderp" --ip 192.0.2.1 --derp-port 8443 --stun-port 3478 --every 100ms \
    > "$OUT/region.json" 2> "$OUT/derp-counters.jsonl" &
  as_user ah-relay env AIRHAND_LISTEN=192.0.2.1:4433 AIRHAND_CERT="$OUT/relay-data/cert.pem" \
    AIRHAND_KEY="$OUT/relay-data/key.pem" AIRHAND_DB="$OUT/relay-data/airhand.db" RUST_LOG=warn \
    "$SERVER" > "$OUT/relay-server.log" 2>&1 &
  for _ in $(seq 50); do [ -s "$OUT/region.json" ] && break; sleep 0.1; done
  sleep 1
  as_user ah-host "$BENCH" host --sig "$OUT/sig.sock" --region "$OUT/region.json" \
    --listen 192.0.2.10:0 --relay 192.0.2.1:4433 --poll-ms "$poll" --log "$OUT/host.jsonl" \
    > "$OUT/host.out" 2>&1 &
  for _ in $(seq 100); do grep -q "host ready" "$OUT/host.out" 2>/dev/null && break; sleep 0.1; done
  grep -q "host ready" "$OUT/host.out" || { cat "$OUT/host.out"; exit 1; }
}

ctrl() { as_user ah-ctrl "$BENCH" ctrl --sig "$OUT/sig.sock" --relay 192.0.2.1:4433 "$@"; }

# Second pair (EXTRA=1 topology), sharing the relay node's services.
start2() {
  local poll=${1:-100}
  as_user ah-host2 "$BENCH" host --sig "$OUT/sig2.sock" --region "$OUT/region.json" \
    --listen 192.0.2.11:0 --relay 192.0.2.1:4433 --poll-ms "$poll" --log "$OUT/host2.jsonl" \
    > "$OUT/host2.out" 2>&1 &
  for _ in $(seq 100); do grep -q "host ready" "$OUT/host2.out" 2>/dev/null && break; sleep 0.1; done
}
ctrl2() { as_user ah-ctrl2 "$BENCH" ctrl --sig "$OUT/sig2.sock" --relay 192.0.2.1:4433 "$@"; }

"$@"
