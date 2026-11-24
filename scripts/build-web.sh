#!/bin/bash
# Build the web app (Leptos → WebAssembly) into crates/agentum-web/dist/, which the hub embeds.
# Needs: the wasm32-unknown-unknown target, wasm-bindgen-cli (same version as the wasm-bindgen
# crate in Cargo.lock) and wasm-opt (binaryen). No Node anywhere.
set -euo pipefail
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
cd "$(dirname "$0")/.."
WEB=crates/agentum-web
DIST=$WEB/dist
TARGET_DIR=${CARGO_TARGET_DIR:-target}
cargo build -p agentum-web --target wasm32-unknown-unknown --profile wasm
rm -rf "$DIST" && mkdir -p "$DIST/assets"
wasm-bindgen --target web --no-typescript --out-dir "$DIST/tmp" --out-name agentum "$TARGET_DIR/wasm32-unknown-unknown/wasm/agentum-web.wasm"
wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals -o "$DIST/tmp/agentum_bg.opt.wasm" "$DIST/tmp/agentum_bg.wasm"
hash() { sha256sum "$1" | cut -c1-10; }
WASM=agentum-$(hash "$DIST/tmp/agentum_bg.opt.wasm").wasm
mv "$DIST/tmp/agentum_bg.opt.wasm" "$DIST/assets/$WASM"
JS=agentum-$(hash "$DIST/tmp/agentum.js").js
mv "$DIST/tmp/agentum.js" "$DIST/assets/$JS"
rm -rf "$DIST/tmp"
# One stylesheet: fonts, the base (index.css), then every other file.
CSSF=$(mktemp)
cat "$WEB/style/fonts.css" "$WEB/style/index.css" > "$CSSF"
find "$WEB/style" -name '*.css' ! -name index.css ! -name fonts.css | sort | xargs cat >> "$CSSF"
CSS=agentum-$(hash "$CSSF").css
mv "$CSSF" "$DIST/assets/$CSS" && chmod 644 "$DIST/assets/$CSS"
sed -e "s/{{JS}}/$JS/g" -e "s/{{WASM}}/$WASM/g" -e "s/{{CSS}}/$CSS/g" "$WEB/index.html" > "$DIST/index.html"
cp -r crates/agentum/public/. "$DIST/"
ls -la "$DIST/assets"
