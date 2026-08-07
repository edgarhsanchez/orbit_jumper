#!/usr/bin/env bash
# Build orbit_jumper for the web into web/dist/.
# WebGL2 (bevy default on wasm): runs on iOS Safari and Android Chrome.
# Requires: rustup target add wasm32-unknown-unknown
#           wasm-bindgen-cli 0.2.126 (match Cargo.lock)
set -euo pipefail
cd "$(dirname "$0")/.."

export RUSTFLAGS='--cfg getrandom_backend="wasm_js"'
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target-wasm}"

cargo build -p oj_game --target wasm32-unknown-unknown --profile wasm-release

rm -rf web/dist
mkdir -p web/dist
wasm-bindgen --target web --no-typescript \
  --out-dir web/dist --out-name oj_game \
  "$CARGO_TARGET_DIR/wasm32-unknown-unknown/wasm-release/oj_game.wasm"
cp web/index.html web/dist/
if command -v wasm-opt >/dev/null; then
  wasm-opt -Oz --strip-debug --strip-producers -all \
    -o web/dist/oj_game_bg.wasm web/dist/oj_game_bg.wasm
fi
ls -lah web/dist/
