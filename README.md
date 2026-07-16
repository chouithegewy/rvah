# RVAH — Rust Video Add-on Host

RVAH is a media-center *host* for third-party video add-ons, written in Rust.
Add-ons implement a narrow, typed provider interface (`list` / `search` /
`resolve` / `account_status`) and reach the outside world only through
capability-checked host services. The host owns everything security-relevant:
networking, storage, secrets, playback, and add-on lifecycle.

**Scope decision:** RVAH is *not* Kodi-compatible. It shares no code, add-on
format, or assets with Kodi; the add-on model is a clean design with the
manifest as the security boundary.

## Crate map

| Crate | Role |
|---|---|
| `rvah-addon-sdk` | Typed data model (`Page`, `MediaItem`, `PlayableSource`, …), `AddonProvider` and `HostApi` traits |
| `rvah-addon-host` | Manifest parsing/validation, signed `.rvah` packaging, install/update/uninstall lifecycle, runtime boundary (`AddonRuntime`) with in-process and Wasmtime (`--features wasm`) implementations, capability-checked `HostApi` bridge |
| `rvah-net` | Allowlisted, quota'd HTTP client — add-ons never get raw sockets |
| `rvah-data` | SQLite persistence: transactional migrations, add-on registry, per-add-on quota'd storage, favorites, watch state, diagnostics |
| `rvah-auth` | Encrypted secret vault (AES-256-GCM), log redaction, device-code auth flow |
| `rvah-player` | Player state machine, source validation, watch-progress tracking, mock backend |
| `rvah-core` | Use-case orchestration: browse/search/resolve→play, quarantine, navigation, resume |
| `rvah-testkit` | Fixture providers and package builders for tests and the simulator |
| `rvah-cli` | `rvah` binary: developer tooling, simulator, and repository publish/consume commands |
| `rvah-player-mpv` | mpv `PlayerBackend` over JSON IPC (Unix; no libmpv linkage) |
| `rvah-ui` | egui presentation adapter: core-service thread, pure view-model reducers, thin views |
| `rvah-app` | `rvah-app` binary: desktop application bootstrap and backend selection |
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

The simulator runs installed add-ons against the in-process fixture provider;
real add-on code execution is the job of the Wasm runtime (below).

## Security model

- **Manifest is the boundary.** Reverse-DNS id, semver version, SDK range,
  declared capabilities, network domain allowlist, storage quota, named
  secrets. Anything undeclared is denied at the host bridge.
- **Signed packages.** A `.rvah` file is a tar.gz with a detached ed25519
  signature over its sha256; installs verify before parsing, updates that
  change the permission set require re-approval, and installed packages can be
  re-verified against the recorded hash.
- **Mediated services.** HTTP goes through an allowlist + request quota;
  storage is size-quota'd per add-on; secrets live in an encrypted vault and
  are only readable if declared; every log line is redacted before persistence.
- **Contained failures.** Provider calls run under a wall-clock timeout; an
  add-on failure never crashes the host, and three consecutive failures
  quarantine the add-on. The optional Wasmtime runtime (`cargo test -p
  rvah-addon-host --features wasm`) adds memory caps and fuel/epoch CPU limits.

## Desktop app

`rvah-app` (Phase 2 MVP UX) provides home/browse/search/player/permissions/
settings/diagnostics/auth screens over the same core the CLI uses. Playback
uses system `mpv` over JSON IPC when present (video in mpv's own window),
falling back to the mock player with an on-screen note. Diagnostics can be
exported as a redacted JSON bundle.

## Repositories

Publish: `rvah repo-index <packages-dir> -o index.json --key signing.key`.
Consume: `rvah repo-check` / `rvah repo-install … --approve` with `--index`
(HTTP(S) URL or local path) and the repo's pinned `--pubkey`. Channels via
`--channel`; deprecation notices surface in `repo-check`. See
`docs/repository-policy.md` and `docs/developer-guide.md`.

## Deferred (post-MVP)

- DASH protocol and subtitle rendering backend; DRM is out of scope entirely
- Wasm ABI v0 host-call dispatch beyond `log` (http/storage/secrets bridge
  exists in-process; wiring it through the wasm boundary is next)
- FR-045 remote-friendly focus navigation; GUI install-permission prompt
  (install stays in the CLI); Windows mpv transport; SBOM tooling in CI
- Telemetry: deliberately none (see docs/developer-guide.md)

## Development

```sh
cargo test --workspace                 # full suite
cargo test --workspace --all-features  # includes the wasm sandbox tests
cargo clippy --workspace --all-features -- -D warnings
cargo fmt --all
```
