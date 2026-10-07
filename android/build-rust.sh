#!/bin/sh
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_dir"

rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
if ! command -v cargo-ndk >/dev/null 2>&1; then
  cargo install cargo-ndk --locked
fi
cargo ndk \
  -t arm64-v8a \
  -t armeabi-v7a \
  -t x86_64 \
  -o android/app/src/main/jniLibs \
  build --release -p rvah-android --lib

cargo build -p rvah-android --lib
cargo run -p rvah-android --bin uniffi-bindgen -- \
  generate \
  --library target/debug/librvah_android.so \
  --language kotlin \
  --out-dir android/app/src/main/generated
