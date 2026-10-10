#!/usr/bin/env sh
# Build the wasm module and stage a packageable add-on directory in dist/.
# Needs: rustup target add wasm32-unknown-unknown
set -e
cd "$(dirname "$0")"
cargo build --release --target wasm32-unknown-unknown
mkdir -p dist
cp target/wasm32-unknown-unknown/release/rvah_addon_tmdb.wasm dist/module.wasm
cp manifest.toml dist/
echo "dist/ ready — next:"
echo "  rvah package dist -o tmdb.rvah --key <keys/signing.key>"
