# Contributing to RVAH

Thanks for helping improve RVAH. This project accepts focused bug fixes,
tests, documentation, and features that preserve the host's security model.

## Before opening a change

- Use the stable Rust toolchain declared in `rust-toolchain.toml`.
- Do not commit API keys, signing keys, `.env`, built add-on packages, user
  data, diagnostic exports, or generated Android/Rust build output.
- Do not submit add-ons, code, or assets that facilitate unauthorized access,
  DRM circumvention, hidden tracking, or copyright infringement.
- Report suspected vulnerabilities through the private process in
  [SECURITY.md](SECURITY.md), not a public issue.

## Development checks

Run the same Rust checks as CI before submitting a pull request:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check
```

For Android changes, also run:

```sh
sh android/build-rust.sh
cd android
./gradlew :app:testDebugUnitTest :app:assembleDebug
```

Android builds require JDK 17, Android SDK/API 36, the NDK, and `cargo-ndk`.
See [android/README.md](android/README.md) for emulator and device checks.

## Pull requests

Keep changes scoped and include tests for behavior changes. Describe the
security and permission impact of add-on, networking, storage, authentication,
package, or playback changes. A passing build is not a substitute for a real
desktop or Android smoke test when the user-visible behavior changed.

Unless stated otherwise, contributions are accepted under the repository's
dual MIT or Apache-2.0 license.
