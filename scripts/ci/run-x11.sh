#!/usr/bin/env bash
# This script owns a new Xvfb. It never attaches to an inherited DISPLAY.
set -euo pipefail
expected_sha=${1:?expected full commit SHA required}
output=${2:?absolute output directory required}
[[ $expected_sha =~ ^[0-9a-f]{40}$ ]] || { echo 'invalid SHA' >&2; exit 2; }
[[ $output == /* ]] || { echo 'output must be absolute' >&2; exit 2; }
[[ $(git rev-parse HEAD) == "$expected_sha" ]] || { echo 'checkout SHA mismatch' >&2; exit 2; }
[[ $(uname -s) == Linux ]] || { echo 'Linux required' >&2; exit 2; }
for tool in Xvfb xauth xdpyinfo python3 cargo; do
  command -v "$tool" >/dev/null || { echo "missing required tool: $tool" >&2; exit 3; }
done
script_dir=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$output"
scratch=$(mktemp -d)
xvfb_pid=
cleanup() {
  if [[ -n $xvfb_pid ]]; then kill "$xvfb_pid" 2>/dev/null || true; wait "$xvfb_pid" 2>/dev/null || true; fi
  rm -rf "$scratch"
}
trap cleanup EXIT
n=100
while [[ -e /tmp/.X11-unix/X$n || -e /tmp/.X$n-lock ]]; do n=$((n + 1)); done
export DISPLAY=:$n WAYLAND_DISPLAY= XAUTHORITY=$scratch/Xauthority
touch "$XAUTHORITY"
cookie=$(python3 -c 'import secrets; print(secrets.token_hex(16))')
xauth -f "$XAUTHORITY" add "$DISPLAY" . "$cookie"
Xvfb "$DISPLAY" -screen 0 1920x1080x24 -nolisten tcp -auth "$XAUTHORITY" > "$output/xvfb.log" 2>&1 &
xvfb_pid=$!
ready=false
for _ in {1..100}; do
  kill -0 "$xvfb_pid" 2>/dev/null || { echo 'owned Xvfb exited' >&2; exit 3; }
  if xdpyinfo > "$output/display.txt" 2>&1; then ready=true; break; fi
  sleep 0.1
done
[[ $ready == true ]] || { echo 'owned Xvfb not ready' >&2; exit 3; }
for extension in DAMAGE MIT-SHM XFIXES RANDR XTEST; do
  grep -F "$extension" "$output/display.txt" >/dev/null || { echo "missing X extension: $extension" >&2; exit 3; }
done
cargo --version > "$output/toolchain.txt"
rustc --version >> "$output/toolchain.txt"
uname -s -m >> "$output/toolchain.txt"
git rev-parse HEAD > "$output/sha.txt"
cd client/src-tauri
for name in host::capture::x11::tests::x11_capture host::input::x11_tests::x11_input; do
  log="$output/${name//::/_}.log"
  set +e
  cargo test --lib --locked "$name" -- --exact --ignored --nocapture --test-threads=1 > "$log" 2>&1
  status=$?
  set -e
  python3 "$script_dir/assert-test.py" "$log" "$name" "$status"
done
