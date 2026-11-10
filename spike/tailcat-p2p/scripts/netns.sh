#!/usr/bin/env bash
# Phase 0 spike topologies (documentation addresses only, RFC 5737 TEST-NET-1):
#   relay 192.0.2.1  (local DERP + STUN + airhand relay server)
#   host  192.0.2.10 (被控端)
#   ctrl  192.0.2.20 (控制端)
#
#   netns.sh bridge   kernel veth + bridge (LAN, no emulation overhead)
#   netns.sh wan      userspace TUN emulator (delay / jitter / loss / cut)
#   netns.sh down
set -euo pipefail
BIN=${BIN:-/tmp/spikebin}
NS=(ah-relay ah-host ah-ctrl)
IPS=(192.0.2.1 192.0.2.10 192.0.2.20)
NAMES=(relay host ctrl)
# EXTRA=1 adds a second pair (host2 192.0.2.11, ctrl2 192.0.2.21) for parallel soak runs.
if [ "${EXTRA:-0}" = 1 ] || [ "${1:-}" = down ]; then
  NS+=(ah-host2 ah-ctrl2); IPS+=(192.0.2.11 192.0.2.21); NAMES+=(host2 ctrl2)
fi

down() {
  for p in "p2p-bench" "$BIN/localderp" "airhand-server" "airhand-p2p-spike" "wanemu serve"; do
    sudo pkill -f "$p" 2>/dev/null || true
  done
  sleep 0.5
  for n in "${NS[@]}"; do sudo ip netns del "$n" 2>/dev/null || true; done
  for n in "${NAMES[@]}"; do sudo ip link del "v-$n" 2>/dev/null || true; done
  sudo ip link del ahbr0 2>/dev/null || true
}

mkns() {
  for n in "${NS[@]}"; do
    sudo ip netns add "$n"
    sudo ip netns exec "$n" ip link set lo up
  done
}

bridge() {
  down; mkns
  sudo ip link add ahbr0 type bridge
  sudo ip link set ahbr0 up
  for i in 0 1 2; do
    sudo ip link add "v-${NAMES[$i]}" type veth peer name eth0 netns "${NS[$i]}"
    sudo ip link set "v-${NAMES[$i]}" master ahbr0 up
    sudo ip netns exec "${NS[$i]}" ip addr add "${IPS[$i]}/24" dev eth0
    sudo ip netns exec "${NS[$i]}" ip link set eth0 up
  done
  # Bridged traffic must not traverse the host's iptables.
  sudo sysctl -qw net.bridge.bridge-nf-call-iptables=0 2>/dev/null || true
}

wan() {
  down; mkns
  args=()
  for i in "${!NS[@]}"; do args+=(--node "${NAMES[$i]}=${NS[$i]}:${IPS[$i]}"); done
  sudo bash -c "nohup $BIN/wanemu serve --sock /tmp/wanemu.sock ${args[*]} > /tmp/wanemu.log 2>&1 &"
  for _ in $(seq 50); do grep -q ready /tmp/wanemu.log 2>/dev/null && break; sleep 0.1; done
  for i in "${!NS[@]}"; do
    sudo ip netns exec "${NS[$i]}" ip addr add "${IPS[$i]}/24" dev wan0
    sudo ip netns exec "${NS[$i]}" ip link set wan0 up mtu 1500
  done
}

"$@"
