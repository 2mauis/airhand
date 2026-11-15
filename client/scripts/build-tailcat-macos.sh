#!/usr/bin/env bash
set -euo pipefail
# Use an existing, fixed local development identity; never generate credentials.
[[ "$(uname -s)" == Darwin ]] || { echo 'Build on macOS with Xcode command-line tools.' >&2; exit 1; }
: "${APPLE_SIGNING_IDENTITY:?Select an existing fixed development signing identity}"
[[ "$APPLE_SIGNING_IDENTITY" != '-' ]] || { echo 'A fixed identity is required for TCC validation.' >&2; exit 1; }
repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo_root"
rust_target="${AIRHAND_MACOS_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"
case "$rust_target" in
  aarch64-apple-darwin) go_arch=arm64 ;;
  x86_64-apple-darwin) go_arch=amd64 ;;
  *) echo 'Use one native macOS target.' >&2; exit 1 ;;
esac
mkdir -p client/src-tauri/binaries
sidecar="$repo_root/client/src-tauri/binaries/airhand-p2p-$rust_target"
GOOS=darwin GOARCH="$go_arch" go -C p2p build -trimpath -o "$sidecar" ./cmd/airhand-p2p
codesign --force --options runtime --timestamp=none --sign "$APPLE_SIGNING_IDENTITY" "$sidecar"
codesign --verify --strict "$sidecar"
cd client
npm run tauri build -- --bundles app --target "$rust_target"
codesign --verify --deep --strict "src-tauri/target/$rust_target/release/bundle/macos/Airhand.app"
