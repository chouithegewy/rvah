# Packaging and Release Builds

## Binaries

| Binary | Crate | Purpose |
|---|---|---|
| `rvah` | `rvah-cli` | Developer tooling + simulator |
| `rvah-app` | `rvah-app` | Desktop application |
| `librvah_android.so` | `rvah-android` | UniFFI core for the Android TV APK |

## Release build

```sh
cargo build --release --workspace
# artifacts: target/release/rvah, target/release/rvah-app
```

The workspace release profile uses thin LTO, a single codegen unit, and
symbol stripping for small distributable binaries.

## Runtime requirements

- **rvah-app playback:** `mpv` on `PATH` and X11/Xwayland on Linux. Video is
  embedded in the app's player view. Without an embeddable native surface the
  app stays usable with the mock player and never opens a separate mpv window.
- **Data directory:** `$XDG_DATA_HOME/rvah` or `~/.local/share/rvah` by
  default; override with `--data-dir`.

## Android TV APK

The Android build targets API 36, supports API 23+, and packages `arm64-v8a`,
`armeabi-v7a`, and `x86_64` Rust libraries. Build it with JDK 17, Android SDK
36, the NDK, and:

```sh
sh android/build-rust.sh
cd android
./gradlew :app:assembleDebug
```

The first command builds the Wasmi/UniFFI core and generates Kotlin bindings.
The APK uses Media3 rather than the desktop egui/mpv adapter.

## Feature flags

- `rvah-addon-host/wasm` — Wasmtime sandbox runtime (heavier build).
- `rvah-addon-host/wasm-wasmi` — portable Wasmi runtime used on Android.

## CI

`.github/workflows/ci.yml` runs fmt, clippy `-D warnings`, and the full test
suite on Linux/macOS/Windows, plus an `--all-features` job (wasm) on Linux.
License/advisory auditing is `cargo deny` (see `deny.toml`); the `sbom` job
generates per-crate CycloneDX SBOMs with `cargo cyclonedx` and uploads them
as the `sbom-cyclonedx` build artifact.

## Debian package

```sh
cargo build --release -p rvah-app -p rvah-cli
cargo deb -p rvah-app --no-build   # -> target/debian/rvah_<version>_amd64.deb
```

One `rvah` package ships both binaries (`rvah-app`, `rvah`) plus the README
and an auto-generated copyright file; `mpv` is a Recommends. Metadata lives
in `[package.metadata.deb]` in `crates/rvah-app/Cargo.toml`. Built at
release time, not per-push in CI.

## Follow-ups (not yet wired)

- rpm/dmg/msi installers — deb + plain binaries only for now.
