#!/usr/bin/env sh
set -eu
cd "$(dirname "$0")"
env RUSTFLAGS= cargo build --release --target wasm32-unknown-unknown
mkdir -p dist
cp target/wasm32-unknown-unknown/release/rvah_addon_veilid.wasm dist/module.wasm
cp manifest.toml dist/manifest.toml
echo 'Package with: rvah package dist -o veilid.rvah --key <private-signing-key>'
