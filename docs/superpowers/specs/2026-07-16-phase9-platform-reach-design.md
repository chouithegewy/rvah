# Phase 9: Platform Reach — Design

The last backlog items (README "Deferred", security-review keychain gap,
packaging.md installers follow-up). Verification strategy differs per item:
A is compile-checked locally against a Windows target and runs its
cross-platform protocol tests in CI's existing windows-latest job; B and C
are fully verifiable on this machine.

## A. Windows mpv transport (named pipes)

mpv on Windows serves `--input-ipc-server` on a named pipe (`\\.\pipe\…`),
which a client opens like a file — no new dependencies:

- `MpvIpc` becomes stream-agnostic: `from_stream` takes any
  `Read + Write + Send` stream pair (writer + cloned reader half);
  `connect(path)` keeps the platform split internally — `UnixStream` on
  unix, `std::fs::OpenOptions::new().read(true).write(true)` on the pipe
  path on Windows.
- `MpvBackend` drops its `#[cfg(unix)]` gates; the IPC endpoint path
  becomes platform-aware (`/tmp`-style socket vs `\\.\pipe\rvah-mpv-<pid>`),
  and mpv is launched identically on both.
- Tests: existing unix fake-server tests stay; new protocol tests use a
  loopback `TcpStream` fake server via `from_stream`, so they run on every
  OS — including CI's windows job, which currently skips this crate
  entirely. Local proof: `cargo check -p rvah-player-mpv --target
  x86_64-pc-windows-gnu` (and the app crates that gate on unix).

## B. OS-keychain vault (security-review gap)

This machine runs gnome-keyring (`org.freedesktop.secrets`), so the
alternative store the SDD lists is implementable and testable here:

- `rvah-auth` gains an optional `keychain` feature: `KeychainVault`
  implements `SecretVault` over the Secret Service D-Bus API, storing one
  item per `(addon_id, name)` with searchable attributes
  (`application=rvah`, `addon`, `name`) so `list` enumerates properly.
- Tests run under the feature and skip gracefully (with a note) when no
  Secret Service daemon is reachable, so plain CI stays green; verified
  for real against gnome-keyring locally.
- security-review.md's "no OS-keychain integration" gap updated.

## C. Debian package (packaging.md follow-up)

- `[package.metadata.deb]` on `rvah-app` (binary, desktop entry-less CLI+GUI
  hybrid; docs + license included), built with `cargo deb`; `rvah-cli` ships
  in the same package as a second asset.
- Verified locally: build the `.deb`, inspect with `dpkg-deb -I`/`-c`.
- packaging.md updated with the build command; CI job deliberately not
  added (release-time task, not per-push).
