# Phase 5: Extensions — Design

The SDD roadmap ends at Phase 4; Phase 5 is user-directed continuation,
scoped to the highest-value deferred items from the Phase 1–4 gap lists.

## A. Wasm host-call dispatch (ABI v0.1)

`env.rvah_host_call(ptr, len) -> i64` grows from `log`-only to the full
`HostApi`, keeping the JSON envelope:

| Request | Response (JSON in guest memory, packed ptr/len) |
|---|---|
| `{"fn":"http_fetch","req":{HttpRequest}}` | `{"Ok":{HttpResponse}}` / `{"Err":{AddonError}}` |
| `{"fn":"setting_get","key":K}` | `{"Ok":{"value":Option<String>}}` |
| `{"fn":"secret_get","name":N}` | `{"Ok":{"value":Option<String>}}` / `{"Err":…}` |
| `{"fn":"storage_get","key":K}` | `{"Ok":{"value":Option<[u8]>}}` |
| `{"fn":"storage_set","key":K,"value":[u8]}` | `{"Ok":null}` / `{"Err":…}` |
| `{"fn":"storage_delete","key":K}` | `{"Ok":null}` |
| `{"fn":"log","level":L,"message":M}` | `{"Ok":null}` |

The host writes the response into guest memory via the guest's own
`rvah_alloc` (re-entrant call through the Caller) and returns
`ptr << 32 | len`; `0` signals a malformed request or allocation failure.
Capability/permission checks stay where they are — in `BridgedHostApi` —
so the wasm layer adds no policy of its own.

Testing: dispatch logic is factored into
`dispatch_host_call(&dyn HostApi, &[u8]) -> Vec<u8>` and unit-tested in
Rust for every op (including denial passthrough); one WAT guest test proves
the FFI plumbing end-to-end (guest logs through the import, host records it).

## B. GUI install flow (UC-01, FR-005 in the app)

Two-step, mirroring the CLI's show-then-approve contract:

- `CoreRequest::PreviewInstall { pkg, pubkey }` → verify signature *first*,
  parse the manifest, reply `CoreEvent::InstallPreview { id, version,
  network, storage_bytes, secrets }`. Bad signature/manifest → routed error.
- `CoreRequest::ConfirmInstall { pkg, pubkey }` → `AddonManager::install`
  (or `update` if the id exists) with `Approve`, register the simulator
  runtime for it, reply with a refreshed Home.

UI: Home gains an "Install add-on" box (package path + pubkey path fields,
Preview button) and an Install screen showing the permission grant with
Approve/Cancel. Reducer/intent logic tested; egui stays thin.

## C. DASH behind backend capability

`PlayerBackend::supports_dash()` (default `false`; mpv `true` — mpv demuxes
DASH natively). `validate_source_opts(source, now, allow_dash)` supersedes
the hardcoded rejection; `AppCore::resolve_and_play` passes the backend's
capability. MockPlayer gains a constructor with DASH enabled for tests.

## D. Dependency auditing

`deny.toml` (license allowlist + advisory checks) and a cargo-deny CI job.
Run locally once to make the config real (cargo-deny 0.19 is installed).

## Out of scope (still deferred)

FR-045 remote focus, Windows mpv transport, subtitle rendering, DNS
pinning/redirect policy, platform installers.
