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

## Follow-ups (not yet wired)

- SBOM / license audit (`cargo about`, `cargo deny`) in CI.
- Platform installers (deb/rpm/dmg/msi) — plain binaries only for now.
