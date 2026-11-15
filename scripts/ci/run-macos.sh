#!/usr/bin/env bash
set -euo pipefail
expected_sha=${1:?expected full commit SHA required}
output=${2:?absolute output directory required}
[[ $expected_sha =~ ^[0-9a-f]{40}$ && $output == /* ]] || { echo 'invalid input' >&2; exit 2; }
[[ $(git rev-parse HEAD) == "$expected_sha" ]] || { echo 'checkout SHA mismatch' >&2; exit 2; }
[[ $(uname -s) == Darwin ]] || { echo 'macOS required' >&2; exit 2; }
script_dir=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$output"
sw_vers > "$output/platform.txt"
uname -s -m >> "$output/platform.txt"
xcodebuild -version >> "$output/platform.txt"
cargo --version > "$output/toolchain.txt"
rustc --version >> "$output/toolchain.txt"
git rev-parse HEAD > "$output/sha.txt"
cd client/src-tauri
# No bundle, signing, TCC changes, capture, audio, clipboard or input injection.
cargo build --workspace --locked > "$output/build.log" 2>&1
for name in host::input::macos::tests::coordinates host::input::macos::tests::modifiers_and_clicks host::encoder::vt::tests::avcc_to_annexb; do
  log="$output/${name//::/_}.log"
  set +e
  cargo test --lib --locked "$name" -- --exact --nocapture --test-threads=1 > "$log" 2>&1
  status=$?
  set -e
  python3 "$script_dir/assert-test.py" "$log" "$name" "$status"
done
