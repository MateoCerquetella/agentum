#!/bin/bash
# Release builds, all static (musl): the web app, the hub and agentd for x86_64 and aarch64.
#   deploy/build.sh            → dist/agentum-<arch>, dist/agentd-<arch>
# Needs cargo-zigbuild + zig, wasm-bindgen-cli, wasm-opt, brotli. No Node.
set -euo pipefail
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
cd "$(dirname "$0")/.."
scripts/build-web.sh > /dev/null
mkdir -p dist
TD=${CARGO_TARGET_DIR:-target}
for arch in x86_64 aarch64; do
  t=$arch-unknown-linux-musl
  cargo zigbuild --release -p agentum -p agentd --target "$t"
  cp "$TD/$t/release/agentum" "dist/agentum-$arch"
  cp "$TD/$t/release/agentd" "dist/agentd-$arch"
done
ls -la dist/
