#!/usr/bin/env sh
# Build the wasm module and stage a packageable add-on directory in dist/,
# with your Trakt app's redirect URI as the setting's default:
# TRAKT_REDIRECT_URI from the environment or the repository's .env
# (default urn:ietf:wg:oauth:2.0:oob).
# Needs: rustup target add wasm32-unknown-unknown
set -e
cd "$(dirname "$0")"
uri=${TRAKT_REDIRECT_URI:-$(sed -n 's/^TRAKT_REDIRECT_URI=//p' ../../../.env 2>/dev/null | tr -d '"'"'" | tail -n 1)}
uri=${uri:-urn:ietf:wg:oauth:2.0:oob}
case "$uri" in
  *[!A-Za-z0-9:/._~%-]*) echo "TRAKT_REDIRECT_URI has unexpected characters." >&2; exit 1 ;;
esac
cargo build --release --target wasm32-unknown-unknown
mkdir -p dist
cp target/wasm32-unknown-unknown/release/rvah_addon_discover.wasm dist/module.wasm
sed "s#\"urn:ietf:wg:oauth:2.0:oob\"#\"$uri\"#" manifest.toml > dist/manifest.toml
echo "dist/ ready (redirect URI $uri) — next:"
echo "  rvah package dist -o discover.rvah --key <keys/signing.key>"
