# RVAH Android TV

The Android app is a native Kotlin/Compose TV shell. Rust owns add-on trust,
Wasmi sandboxing, permissions, networking, settings, secrets policy, source
selection, and watch history. Media3 owns the activity-scoped player, surface,
media session, lifecycle, and remote controls.

Requirements: JDK 17, Android SDK/API 36, NDK, Gradle 8.11+, and `cargo-ndk`.

```sh
sh android/build-rust.sh
cd android
./gradlew :app:assembleDebug
```

`build-rust.sh` packages `arm64-v8a`, `armeabi-v7a`, and `x86_64` native
libraries and generates the UniFFI Kotlin sources. All exported Rust calls are
blocking; the app invokes catalog, resolve, event, diagnostic, and setting work
on `Dispatchers.IO`.

Release builds accept only packages verified by configured trusted Ed25519
keys. A signed deterministic fixture package should be copied into the debug
assets by the fixture-media CI job; release assets must never contain it.

The emulator gate covers progressive, HLS, and DASH resolution plus pause,
seek, resume, Back, and recreation. First-frame rendering, D-pad focus,
hardware decode, and audio sync still require a physical Android TV smoke test.
