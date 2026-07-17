# Packaging and Release Builds

## Binaries

| Binary | Crate | Purpose |
|---|---|---|
| `rvah` | `rvah-cli` | Developer tooling + simulator |
| `rvah-app` | `rvah-app` | Desktop application |

## Release build

```sh
cargo build --release --workspace
# artifacts: target/release/rvah, target/release/rvah-app
```

The workspace release profile uses thin LTO, a single codegen unit, and
symbol stripping for small distributable binaries.

## Runtime requirements

- **rvah-app playback:** `mpv` on `PATH` (Unix). Without it the app runs with
  the mock player and says so in the toolbar. Windows always uses the mock
  player for now (mpv IPC is Unix-socket based).
- **Data directory:** `$XDG_DATA_HOME/rvah` or `~/.local/share/rvah` by
  default; override with `--data-dir`.

## Feature flags

- `rvah-addon-host/wasm` — Wasmtime sandbox runtime (heavier build).

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
