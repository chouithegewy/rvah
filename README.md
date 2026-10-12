# RVAH — Rust Video Add-on Host

RVAH is a media-center *host* for third-party video add-ons, written in Rust.
Add-ons implement a narrow, typed provider interface (`list` / `search` /
`resolve` / `account_status`) and reach the outside world only through
capability-checked host services. The host owns everything security-relevant:
networking, storage, secrets, playback, and add-on lifecycle.

**Scope decision:** RVAH is *not* Kodi-compatible. It shares no code, add-on
format, or assets with Kodi; the add-on model is a clean design with the
manifest as the security boundary.

## Project status

RVAH is pre-1.0 software under active development. The native Linux desktop
application and CLI build on x86_64, and the Android TV shell targets API 23+
with API 36 build tooling. There is no browser application target; WebAssembly
is used to sandbox add-ons inside the native hosts. Interfaces and package
formats may change before a stable release.

RVAH ships no media or provider credentials. Third-party service names and
example integrations belong to their respective owners and do not imply
endorsement. Users and add-on authors are responsible for complying with
applicable law and service terms. Add-ons that bypass access controls, enable
unauthorized access, or hide data collection are outside project policy.

## Crate map

| Crate | Role |
|---|---|
| `rvah-addon-sdk` | Typed data model (`Page`, `MediaItem`, `PlayableSource`, …), `AddonProvider` and `HostApi` traits |
| `rvah-addon-host` | Manifest parsing/validation, signed `.rvah` packaging, install/update/uninstall lifecycle, runtime boundary (`AddonRuntime`) with in-process, Wasmtime desktop, and Wasmi Android implementations, capability-checked `HostApi` bridge |
| `rvah-net` | Allowlisted, quota'd HTTP client — add-ons never get raw sockets |
| `rvah-data` | SQLite persistence: transactional migrations, add-on registry, per-add-on quota'd storage, favorites, watch state, diagnostics |
| `rvah-auth` | Encrypted secret vault (AES-256-GCM), log redaction, device-code auth flow |
| `rvah-profile` | Portable AES-256-GCM profile bundles for add-on settings and linked accounts |
| `rvah-player` | Player state machine, source validation, watch-progress tracking, mock backend |
| `rvah-core` | Use-case orchestration: browse/search/resolve→play, quarantine, navigation, resume |
| `rvah-testkit` | Fixture providers and package builders for tests and the simulator |
| `rvah-cli` | `rvah` binary: developer tooling, simulator, and repository publish/consume commands |
| `rvah-player-mpv` | mpv `PlayerBackend` over JSON IPC with app-embedded video |
| `rvah-ui` | egui presentation adapter: core-service thread, pure view-model reducers, thin views |
| `rvah-app` | `rvah-app` binary: desktop application bootstrap and backend selection |
| `rvah-android` | Typed UniFFI boundary for native Android catalog, playback, settings, diagnostics, and Keystore callbacks |
| `rvah-repo` | Signed repository indexes, update checks with channels, deprecation notices (FR-008) |

## Quickstart (fixture add-on)

```sh
cargo build --workspace

# 1. Author an add-on directory with a manifest
mkdir -p demo-addon && cat > demo-addon/manifest.toml <<'EOF'
[addon]
id = "com.example.demo"
name = "Demo"
version = "1.0.0"
sdk = ">=0.1, <0.2"
entry = "main"
capabilities = ["catalog", "search", "resolve"]

[permissions]
network = []
storage = "1MiB"
secrets = []
EOF
touch demo-addon/main

# 2. Validate, sign, package, install
rvah validate demo-addon/manifest.toml
rvah keygen -o keys
rvah package demo-addon -o demo.rvah --key keys/signing.key
rvah install demo.rvah --pubkey keys/verify.pub --data-dir ./data --approve

# 3. Drive it through the simulator (mock UI)
rvah addons --data-dir ./data
rvah browse com.example.demo --data-dir ./data
rvah browse com.example.demo /movies --data-dir ./data
rvah resolve com.example.demo m1 --data-dir ./data
rvah play com.example.demo m1 --data-dir ./data     # persists resume state
rvah diagnostics --data-dir ./data                  # redacted event log
```

The CLI simulator drives installed add-ons with the in-process fixture
provider; the desktop app executes wasm add-ons for real (see below). Store
an add-on's declared secret with
`printf '%s' "$TOKEN" | rvah secret-set <id> <name> --data-dir DIR`.

## Security model

- **Manifest is the boundary.** Reverse-DNS id, semver version, SDK range,
  declared capabilities, network domain allowlist, storage quota, named
  secrets. Anything undeclared is denied at the host bridge.
- **Signed packages.** A `.rvah` file is a tar.gz with a detached ed25519
  signature over its sha256; installs verify before parsing, updates that
  change the permission set require re-approval, and installed packages can be
  re-verified against the recorded hash.
- **Mediated services.** HTTP goes through an allowlist + request quota, with
  redirects re-checked per hop and DNS resolved once, vetted against a
  non-global address policy, and pinned for the connection; storage is
  size-quota'd per add-on; secrets live in an encrypted vault and are only
  readable if declared; every log line is redacted before persistence.
- **Contained failures.** Provider calls run under a wall-clock timeout; an
  add-on failure never crashes the host, and three consecutive failures
  quarantine the add-on. The optional Wasmtime runtime (`cargo test -p
  rvah-addon-host --features wasm`) adds memory caps and fuel/epoch CPU limits.

## Desktop app

`rvah-app` provides home/browse/search/player/permissions/settings/
diagnostics/auth screens over the same core the CLI uses. Installed add-ons
whose package ships a wasm module run in the Wasmtime sandbox against the
real permission bridge (allowlisted + DNS-pinned HTTP, quota'd storage,
declared secrets, capability gating); `examples/addons/real-debrid` is a
complete real-service add-on. Playback
uses system `mpv` over JSON IPC when present (a Unix socket on unix, a named
pipe on Windows). On Linux, video is embedded into the app's X11/Xwayland
player view; where that is not possible (Windows, macOS, Wayland without
Xwayland) the app falls back to the mock player with an on-screen note, or,
when started with `rvah-app --mpv-window`, to mpv in its own window. Built with `--features keychain`, add-on secrets go
to the OS keychain (freedesktop Secret Service) instead of the encrypted
file vault when a daemon is available. Diagnostics can be
exported as a redacted JSON bundle. Remote-friendly keys: Up/Down move a
D-pad focus across rows, Left/Right across a row's actions, Enter activates;
Escape goes home, Backspace goes back, Alt+arrows walk browse history, Space
toggles play/pause, S stops. Paginated catalogs load further pages via the
Load-more row (FR-024).

## Android TV

`android/` is the primary native product shell: Kotlin, Compose for TV,
Media3 ExoPlayer, `PlayerSurface`, and an activity-scoped `MediaSession`.
Rust prepares a validated `PreparedPlayback`; Kotlin applies its headers to a
resolving data source, submits the `MediaItem`, calls `prepare` and `play` in
one operation, and reports session-scoped state/progress back to Rust. Android
add-ons run through Wasmi with the same `.rvah` JSON ABI, fuel budget, memory
ceiling, permission bridge, and signed-package policy as desktop. See
`android/README.md` for prerequisites and build commands.

## Repositories

Publish: `rvah repo-index <packages-dir> -o index.json --key signing.key`.
Consume: `rvah repo-check` / `rvah repo-install … --approve` with `--index`
(HTTP(S) URL or local path) and the repo's pinned `--pubkey`. Channels via
`--channel`; deprecation notices surface in `repo-check`. See
`docs/repository-policy.md` and `docs/developer-guide.md`.

Linked-account credentials are device-local by default. For encrypted export,
cross-device enrollment, and the proposed Veilid synchronization/media design,
see [`docs/portable-profiles-and-veilid.md`](docs/portable-profiles-and-veilid.md).

## Deferred (post-MVP)

- DRM is out of scope; protected sources fail with an explicit unsupported-DRM
  error. Clear DASH and resolver-provided subtitle tracks play on capable
  native backends.
- Telemetry: deliberately none (see docs/developer-guide.md)

## Development

```sh
cargo test --workspace                 # full suite
cargo test --workspace --all-features  # includes the wasm sandbox tests
cargo clippy --workspace --all-features -- -D warnings
cargo fmt --all
```

See [CONTRIBUTING.md](CONTRIBUTING.md) before submitting changes. Report
security issues privately as described in [SECURITY.md](SECURITY.md). The
project is dual-licensed under [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE).
