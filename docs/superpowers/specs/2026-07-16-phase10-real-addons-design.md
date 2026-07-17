# Phase 10: Real Add-ons — Design

User-directed: the desktop app must run a real add-on against Real-Debrid.
(Project direction note: the eventual primary target is Android TV — the
signed wasm `.rvah` package is the artifact that carries over, which is why
the wasm path is the one being completed here.)

## A. Wasm execution in the desktop app

Today `bootstrap` registers the fixture `CatalogEchoProvider` + `NoopHost`
for every installed add-on. Change: for each installed add-on whose manifest
`entry` ends in `.wasm` (and the `wasm` feature is on — default for
rvah-app), register `WasmRuntime::load(entry bytes)` with a real
`BridgedHostApi` (UreqTransport + `SystemResolver` DNS pinning, session
request quota); anything else keeps the fixture provider (dev simulator
behavior, unchanged tests).

- `BridgedHostApi::new` gains a resolver hook (`with_resolver` constructor
  variant) so production traffic gets the Phase 7 pinning policy.
- Timeouts: `DEFAULT_INVOKE_TIMEOUT` (2 s) and `WasmLimits::timeout` (2 s)
  assume fixture-fast providers; network add-ons need ~30 s. Bootstrap
  raises both for wasm add-ons (`AppCore::invoke_timeout`, custom
  `WasmLimits`); sandbox memory/fuel limits stay.
- Test: package a WAT echo fixture as a `.rvah`, install into a temp data
  dir, `bootstrap()`, browse → the wasm page (not the echo provider's).

## B. `rvah secret-set`

Add-ons read secrets via `secret_get`; nothing lets a user store one. New
CLI command: `rvah secret-set <addon-id> <name> --data-dir DIR` reading the
value from stdin (not argv — keeps tokens out of shell history), writing to
the same `EncryptedFileVault` the app opens.

## C. Real-Debrid add-on (`examples/addons/real-debrid`)

Standalone crate (own `[workspace]`), cdylib for `wasm32-unknown-unknown`,
depending on `rvah-addon-sdk` by path for the shared types; ABI v0.1 shell
(`rvah_alloc` leak-based allocator, `rvah_invoke` JSON dispatch,
`rvah_host_call` import wrappers).

- Manifest: id `com.real-debrid.downloads`, capabilities
  `catalog`/`resolve`/`account_status`, network `["api.real-debrid.com"]`,
  secrets `["api_token"]`, entry `module.wasm`.
- `list`: `GET /rest/1.0/downloads?page=N&limit=50`, Bearer token; each
  entry → `MediaItem` (title = filename); page number is the cursor; while
  listing, cache `id → download URL` in add-on storage.
- `resolve`: storage lookup first, then a paged re-fetch fallback; returns
  one https `PlayableSource` (RD `download` links are already direct).
  Missing token → `ResolveResult::AuthRequired`-adjacent `Auth` error.
- `account_status`: `GET /rest/1.0/user` → premium/free.
- Core logic is plain functions unit-tested natively in the crate (the
  crate builds for both native tests and wasm release); `build.sh` produces
  `dist/` (manifest + module.wasm) ready for `rvah package`.

## D. Verification + setup docs

- Automated: the bootstrap WAT-fixture test (A); native unit tests (C).
- Local (not committed to CI — needs the wasm toolchain): compile the
  add-on, load the actual `module.wasm` through `WasmRuntime` +
  `BridgedHostApi` with `MockTransport` fixtures of the RD API, and drive
  list → resolve; then package/sign/install into a scratch data dir.
- Docs: developer-guide's stale "host-call dispatch currently supports
  `log`" line fixed; a "Use a real service (Real-Debrid example)" setup
  section: build, package, install, `secret-set`, install mpv, run app.
