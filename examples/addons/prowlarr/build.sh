#!/usr/bin/env sh
# Build the wasm module and stage a packageable add-on directory in dist/,
# with your Prowlarr's address in the manifest: PROWLARR_URL from the
# environment or the repository's .env (https://host[:port], no path).
# Needs: rustup target add wasm32-unknown-unknown
set -e
cd "$(dirname "$0")"
url=${PROWLARR_URL:-$(sed -n 's/^PROWLARR_URL=//p' ../../../.env 2>/dev/null | tr -d '"'"'" | tail -n 1)}
host=$(printf '%s' "$url" | sed -n 's#^https://\([a-z0-9.-]*\)\(:[0-9]*\)\{0,1\}/\{0,1\}$#\1#p')
if [ -z "$host" ]; then
  echo "Set PROWLARR_URL to https://<host>[:port] (in the environment or the repo's .env)." >&2
  exit 1
fi
url=$(printf '%s' "$url" | sed 's#/$##')
cargo build --release --target wasm32-unknown-unknown
mkdir -p dist
cp target/wasm32-unknown-unknown/release/rvah_addon_prowlarr.wasm dist/module.wasm
sed -e "s#\"prowlarr.example.com\"#\"$host\"#" -e "s#\"https://prowlarr.example.com\"#\"$url\"#" manifest.toml > dist/manifest.toml
echo "dist/ ready for $url — next:"
echo "  rvah package dist -o prowlarr.rvah --key <keys/signing.key>"
