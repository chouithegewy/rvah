# RVAH Phase 1 MVP Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the RVAH (Rust Video Add-on Host) Phase 1 MVP Core: a Cargo workspace implementing the typed add-on SDK, manifest/package/signing pipeline, permissioned add-on host with quotas, allowlisted HTTP client, SQLite data layer with migrations, secret vault with redaction, player state machine with mock backend, fixture add-ons, and a developer CLI that acts as the mock UI — plus a best-effort Wasmtime sandbox runtime behind the `AddonRuntime` trait.

**Architecture:** Modular Cargo workspace per SDD §4. The host owns security, storage, networking, and playback; add-ons implement a narrow typed provider interface (`list`/`search`/`resolve`/`account_status`) and reach host services only through capability-checked APIs. The runtime is an adapter (`AddonRuntime` trait): an in-process fixture runtime powers tests and the developer simulator (FR-027), and a Wasmtime-backed runtime provides the untrusted-code sandbox (FR-020).

**Tech Stack:** Rust stable, serde/serde_json/toml, semver, thiserror, url, rusqlite (bundled), ed25519-dalek + sha2 (signing), aes-gcm (vault), tar + flate2 (packaging), ureq (real HTTP transport), tempfile (tests), wasmtime (final task only).

## Global Constraints

- Primary implementation language: Rust (SRD front matter).
- Not Kodi-compatible; no Kodi code or assets (A-02, A-03, SDD §9 "License contamination").
- Add-ons never get raw sockets/OS/credential access — all host-mediated (SDD §2.1, FR-080).
- Manifest is the security boundary: id, version (semver), sdk range, entry, capabilities, permissions.network allowlist, storage quota, secrets, settings schema (SDD §5).
- Add-on failures must never crash the host (FR-023, NFR Reliability); repeated offenders are quarantined (NFR Reliability, TC-NFR-005).
- Secrets redacted from all logs/diagnostics (FR-084, TC-SEC-003, TC-DATA-003).
- Watched threshold and resume position persisted per media item and provider (FR-065).
- Player supports play/pause/resume/stop/seek/volume/progress via structured events and errors (FR-062, FR-066).
- Only resolved, validated playable sources reach the player; header policy allows only user-agent/referer from resolver outputs (FR-060, FR-063).
- DB migrations are transactional and roll back on failure (FR-105, TC-DATA-002).
- Workspace crate names per SDD §4: rvah-core, rvah-addon-sdk, rvah-addon-host, rvah-player, rvah-data, rvah-auth, rvah-net, rvah-cli, rvah-testkit (rvah-app/rvah-ui deferred to Phase 2 "MVP UX"; rvah-player-mpv and rvah-repo deferred per Phase 1 scope "mock UI, mock player tests"; the wasm runtime lives in rvah-addon-host behind a feature).

---

### Task 1: Workspace scaffold + rvah-addon-sdk (typed SDK data model)

**Files:**
- Create: `Cargo.toml` (workspace), `.gitignore`, `rust-toolchain.toml`
- Create: `crates/rvah-addon-sdk/Cargo.toml`, `crates/rvah-addon-sdk/src/lib.rs`, `crates/rvah-addon-sdk/src/types.rs`, `crates/rvah-addon-sdk/src/error.rs`, `crates/rvah-addon-sdk/src/provider.rs`
- Test: inline `#[cfg(test)]` in each module

**Interfaces (Produces):**
- `enum ContentType { Movie, Episode, Series, Season, Clip, Other }`
- `struct Artwork { poster: Option<String>, fanart: Option<String>, thumb: Option<String> }`
- `struct DirectoryItem { id, title, subtitle: Option<String>, artwork, path: String, badges: Vec<String> }`
- `struct MediaItem { id, content_type, title, year: Option<u16>, season: Option<u32>, episode: Option<u32>, runtime_secs: Option<u32>, synopsis: Option<String>, rating: Option<f32>, artwork, provider_ids: BTreeMap<String,String> }`
- `enum PageItem { Directory(DirectoryItem), Media(MediaItem) }`
- `struct Page { items: Vec<PageItem>, cursor_next: Option<String>, total_hint: Option<u64> }`
- `enum Protocol { Http, Https, Hls, Dash, File }` with `Protocol::from_url(&str) -> Option<Protocol>`
- `struct SubtitleTrack { url, lang, label: Option<String> }`
- `struct PlayableSource { url, protocol, headers: BTreeMap<String,String>, subtitles: Vec<SubtitleTrack>, quality: Option<String>, audio_lang: Option<String>, expires_at: Option<u64> }` (unix secs)
- `enum ResolveResult { Playables(Vec<PlayableSource>), AuthRequired { provider_hint: String }, Unavailable { reason: String } }`
- `enum AccountStatus { SignedOut, SignedIn { display_name: Option<String> }, Expired }`
- `struct Filter { key: String, value: String }`
- `struct AddonError { code: AddonErrorCode, message: String, retryable: bool, user_visible: bool, diagnostics_id: Option<String> }` with `enum AddonErrorCode { Network, Auth, NotFound, Quota, Timeout, Internal, Malformed, PermissionDenied }`
- `trait AddonProvider { fn list(&self, ctx: &dyn HostApi, path: Option<&str>, cursor: Option<&str>) -> Result<Page, AddonError>; fn search(&self, ctx, query: &str, filters: &[Filter], cursor: Option<&str>) -> Result<Page, AddonError>; fn resolve(&self, ctx, item_id: &str, source_id: Option<&str>) -> Result<ResolveResult, AddonError>; fn account_status(&self, ctx) -> Result<AccountStatus, AddonError> }` (default impls return `AddonErrorCode::Internal` "capability not implemented")
- `trait HostApi { fn http_fetch(&self, req: HttpRequest) -> Result<HttpResponse, AddonError>; fn setting_get(&self, key: &str) -> Option<String>; fn secret_get(&self, name: &str) -> Result<Option<String>, AddonError>; fn storage_get(&self, key: &str) -> Option<Vec<u8>>; fn storage_set(&self, key: &str, value: &[u8]) -> Result<(), AddonError>; fn storage_delete(&self, key: &str); fn log(&self, level: LogLevel, message: &str) }`
- `struct HttpRequest { method: String, url: String, headers: BTreeMap<String,String>, body: Option<Vec<u8>> }`, `struct HttpResponse { status: u16, headers: BTreeMap<String,String>, body: Vec<u8> }`, `enum LogLevel { Trace, Debug, Info, Warn, Error }`

All types `Serialize + Deserialize + Clone + Debug + PartialEq` (traits excepted).

- [x] Write failing serde round-trip tests: `page_roundtrips_through_json`, `resolve_result_roundtrips`, `protocol_from_url` (https/hls `.m3u8`/file/unknown-scheme cases)
- [x] Run `cargo test -p rvah-addon-sdk` — expect compile failure
- [x] Implement types/error/provider modules
- [x] `cargo test -p rvah-addon-sdk` — PASS; commit `feat(sdk): typed add-on SDK data model and provider trait`

### Task 2: rvah-addon-host manifest parsing and validation

**Files:**
- Create: `crates/rvah-addon-host/Cargo.toml`, `src/lib.rs`, `src/manifest.rs`
- Test: inline tests with TOML fixtures as string literals

**Interfaces:**
- Consumes: nothing (pure)
- Produces: `struct Manifest { addon: AddonSection, permissions: PermissionsSection, settings: BTreeMap<String, SettingSpec> }`
  - `AddonSection { id, name, version: semver::Version, sdk: semver::VersionReq, entry: String, capabilities: Vec<Capability> }`
  - `enum Capability { Catalog, Search, Resolve, Auth, AccountStatus }`
  - `PermissionsSection { network: Vec<String>, storage_bytes: u64, secrets: Vec<String> }` (storage parsed from "10MiB"-style strings)
  - `SettingSpec { r#type: String, default: toml::Value, r#enum: Option<Vec<String>> }`
- `Manifest::parse(toml_str) -> Result<Manifest, ManifestError>`; `enum ManifestError { MissingField(String), InvalidVersion(String), InvalidSdkRange(String), InvalidId(String), InvalidPermission(String), Toml(String) }`
- Validation: id must be reverse-DNS (`[a-z0-9-]+(\.[a-z0-9-]+)+`), version valid semver, sdk valid range, entry non-empty, network entries are bare domains (no scheme/path; `*.` prefix wildcard allowed)
- `HOST_SDK_VERSION: semver::Version = 0.1.0`; `Manifest::is_sdk_compatible(&self) -> bool`

- [x] Failing tests: `parses_full_manifest` (SDD §5 example), `rejects_missing_id`, `rejects_bad_semver`, `rejects_unsupported_sdk_range`, `rejects_url_in_network_permission`, `parses_storage_quota_mib`
- [x] Implement; PASS; commit `feat(host): manifest parsing and validation (FR-002, FR-003)`

### Task 3: rvah-net allowlisted HTTP client

**Files:**
- Create: `crates/rvah-net/Cargo.toml`, `src/lib.rs`, `src/allowlist.rs`, `src/client.rs`, `src/mock.rs`

**Interfaces:**
- Consumes: `HttpRequest`/`HttpResponse` from rvah-addon-sdk
- Produces: `trait Transport: Send + Sync { fn execute(&self, req: &HttpRequest) -> Result<HttpResponse, NetError> }`
- `struct UreqTransport` (real), `struct MockTransport` (route table: method+url prefix → canned response or error; records requests)
- `struct Allowlist(Vec<String>)` with `fn permits(&self, host: &str) -> bool` — exact match or `*.example.com` suffix match on label boundary; never permits IP-literals under wildcards
- `struct PermissionedClient { transport, allowlist, max_requests: u32, counter }` — `fn fetch(&self, req) -> Result<HttpResponse, NetError>`; denies non-http(s) schemes, disallowed hosts (`NetError::DomainDenied`), and over-quota calls (`NetError::QuotaExceeded`)
- `enum NetError { DomainDenied(String), SchemeDenied(String), QuotaExceeded, Transport(String), InvalidUrl(String) }`

- [x] Failing tests: `allows_exact_domain`, `wildcard_matches_subdomain_not_bare_or_suffix` (`*.ex.com` matches `a.ex.com`, not `ex.com`, not `evilex.com`), `denies_unlisted_domain_before_transport` (MockTransport records zero calls), `denies_file_scheme` (TC-SEC-002), `request_quota_enforced`
- [x] Implement; PASS; commit `feat(net): permissioned HTTP client with domain allowlists (FR-080, FR-081)`

### Task 4: rvah-data SQLite store, migrations, repositories

**Files:**
- Create: `crates/rvah-data/Cargo.toml`, `src/lib.rs`, `src/migrations.rs`, `src/repos.rs`

**Interfaces:**
- Produces: `struct Database` (`open(path) / open_in_memory()` runs migrations), `db.conn()` internal
- Migrations: `static MIGRATIONS: &[Migration { version, sql }]`; applied in one transaction each; `schema_version` table; failed migration rolls back leaving prior version intact (test injects bad SQL migration)
- Tables per SDD §10: `addons(id TEXT PK, version, package_hash, status TEXT CHECK(status IN ('enabled','disabled','quarantined')), install_source, sdk_range, capabilities_json)`, `addon_permissions(addon_id, permissions_json, manifest_permission_hash, approved_at)`, `addon_settings(addon_id, key, value_json, PK(addon_id,key))`, `addon_storage(addon_id, key, value BLOB, PK(addon_id,key))`, `favorites(addon_id, item_id, title, artwork_json, added_at, PK(addon_id,item_id))`, `watch_state(addon_id, item_id, position_secs, duration_secs, watched INTEGER, last_played_at, PK(addon_id,item_id))`, `cache_index(key PK, addon_id, size_bytes, expires_at)`, `diagnostic_events(id INTEGER PK AUTOINCREMENT, ts, level, addon_id, code, message)`
- Repos (methods on `Database`): `upsert_addon(AddonRecord)`, `get_addon(id) -> Option<AddonRecord>`, `list_addons()`, `set_addon_status(id, AddonStatus)`, `remove_addon(id, purge_data: bool)`, `record_permissions(addon_id, permissions_json, hash)`, `get_permission_hash(addon_id)`, `set_setting/get_setting`, `storage_get/storage_set (with per-addon quota check: total bytes ≤ manifest quota → Err(DataError::StorageQuotaExceeded))/storage_delete/storage_used(addon_id)`, `add_favorite/list_favorites/remove_favorite`, `set_watch_state(WatchState)/get_watch_state(addon_id,item_id)`, `log_event(DiagnosticEvent)/recent_events(limit)`
- `struct AddonRecord { id, version, package_hash, status: AddonStatus, install_source, sdk_range, capabilities: Vec<String> }`, `enum AddonStatus { Enabled, Disabled, Quarantined }`, `struct WatchState { addon_id, item_id, position_secs: f64, duration_secs: f64, watched: bool, last_played_at: u64 }`

- [x] Failing tests: `migrations_apply_fresh`, `failed_migration_rolls_back` , `addon_crud_roundtrip`, `storage_quota_enforced` (FR-101), `watch_state_roundtrip`, `remove_addon_purges_or_retains_data` (TC-ADD-007/008)
- [x] Implement; PASS; commit `feat(data): SQLite store, transactional migrations, repositories (FR-100, FR-101, FR-105)`

### Task 5: rvah-auth secret vault and redaction

**Files:**
- Create: `crates/rvah-auth/Cargo.toml`, `src/lib.rs`, `src/vault.rs`, `src/redact.rs`

**Interfaces:**
- Produces: `trait SecretVault: Send + Sync { fn set(&self, addon_id: &str, name: &str, value: &str) -> Result<(), VaultError>; fn get(&self, addon_id: &str, name: &str) -> Result<Option<String>, VaultError>; fn delete(&self, addon_id: &str, name: &str) -> Result<(), VaultError>; fn list(&self, addon_id: &str) -> Result<Vec<String>, VaultError> }`
- `MemoryVault` (tests), `EncryptedFileVault::open(path, key_path)` — AES-256-GCM, random nonce per write, key auto-generated 0600; wrong key → `VaultError::Crypto`
- `struct Redactor` — `register_secret(value)`, `redact(&self, text) -> String` replaces registered values and pattern matches (`Authorization: ...`, `Bearer <tok>`, `api_key=...`, `access_token=...`) with `[REDACTED]`
- `struct DeviceCodeFlow` state machine: `Pending { user_code, verification_url, expires_at } → poll() → Authorized { token } | Expired | Denied` driven by injected poller fn (mock in tests) — stores token via vault on success (UC-04, TC-AUTH-001/002)

- [x] Failing tests: `vault_roundtrip_and_isolation_by_addon`, `encrypted_vault_persists_across_open`, `encrypted_vault_wrong_key_fails`, `redactor_masks_registered_and_patterns` (TC-SEC-003), `device_code_success_stores_token`, `device_code_expiry_stores_nothing` (TC-AUTH-002)
- [x] Implement; PASS; commit `feat(auth): encrypted secret vault, redaction, device-code flow (FR-083, FR-084)`

### Task 6: rvah-player state machine, source validator, progress

**Files:**
- Create: `crates/rvah-player/Cargo.toml`, `src/lib.rs`, `src/backend.rs`, `src/controller.rs`, `src/validate.rs`, `src/progress.rs`

**Interfaces:**
- Consumes: `PlayableSource`, `Protocol` from rvah-addon-sdk
- Produces: `trait PlayerBackend { fn load(&mut self, source: &PlayableSource, start_at: f64) -> Result<(), PlayerError>; fn play/pause/stop(&mut self) -> Result<(), PlayerError>; fn seek(&mut self, secs: f64) -> Result<(), PlayerError>; fn set_volume(&mut self, v: f32) -> Result<(), PlayerError>; fn poll_events(&mut self) -> Vec<PlayerEvent> }`
- `enum PlayerEvent { Loaded { duration_secs: f64 }, Buffering, Progress { position_secs: f64 }, Ended, Error(PlayerError) }`
- `struct PlayerError { code: PlayerErrorCode, message: String, fatal: bool }`, `enum PlayerErrorCode { BackendMissing, UnsupportedMedia, Network, Io, Internal }` (FR-066)
- `enum PlayerState { Idle, Loading, Buffering, Playing, Paused, Seeking, Ended, Error }` (SDD §8)
- `struct PlayerController<B: PlayerBackend>` — `load_source/play/pause/seek/stop`, `handle_events()` applies backend events to state; illegal transitions (`play` from Idle) → `Err(PlayerError Internal, fatal:false)`
- `fn validate_source(source: &PlayableSource, now_unix: u64) -> Result<(), SourceRejection>`; `enum SourceRejection { UnsupportedProtocol(String), Expired, ForbiddenHeader(String), LocalPathDenied(String) }` — allows Http/Https/Hls; rejects Dash (MVP), File, expired `expires_at <= now`, headers outside {user-agent, referer} case-insensitive (FR-063, TC-PLY-003/004/005)
- `struct ProgressTracker::new(watched_threshold: f32 /*0.9*/, debounce_secs: f64)` — `fn on_progress(&mut self, position, duration, now) -> Option<ProgressUpdate { position_secs, duration_secs, watched }>` emits at most once per debounce window, watched=true at ≥ threshold (FR-065, TC-PLY-007)
- `struct MockPlayer` implementing PlayerBackend with scripted event queues (lives here so rvah-core tests can use it; re-exported by testkit)

- [x] Failing tests: `controller_full_lifecycle` (load→play→pause→seek→resume→stop per TC-PLY-001 using MockPlayer), `illegal_transition_is_nonfatal_error`, `validator_rejects_unsupported_protocol`, `validator_rejects_expired`, `validator_rejects_forbidden_header_allows_ua_referer`, `progress_debounced_and_watched_at_threshold`, `backend_fatal_error_moves_to_error_state` (TC-PLY-010)
- [x] Implement; PASS; commit `feat(player): controller state machine, source validation, progress (FR-060..066)`

### Task 7: rvah-addon-host runtime, host API bridge, packaging/signing, lifecycle

**Files:**
- Modify: `crates/rvah-addon-host/Cargo.toml`, `src/lib.rs`
- Create: `src/runtime.rs`, `src/host_api.rs`, `src/package.rs`, `src/install.rs`

**Interfaces:**
- Consumes: Manifest (Task 2), PermissionedClient/Transport (Task 3), Database (Task 4), SecretVault/Redactor (Task 5), sdk types (Task 1)
- Produces:
  - `trait AddonRuntime: Send { fn invoke(&self, op: AddonOp, ctx: &InvocationCtx) -> Result<AddonResponse, AddonError> }`
  - `enum AddonOp { List { path: Option<String>, cursor: Option<String> }, Search { query: String, filters: Vec<Filter>, cursor: Option<String> }, Resolve { item_id: String, source_id: Option<String> }, AccountStatus }`
  - `enum AddonResponse { Page(Page), Resolve(ResolveResult), Account(AccountStatus) }`
  - `struct InvocationCtx { timeout: Duration, host: Arc<dyn HostApi> }`
  - `InProcessRuntime::new(Arc<dyn AddonProvider + Send + Sync>)` — runs invoke on worker thread, enforces wall-clock timeout via `mpsc::recv_timeout` → `AddonError { code: Timeout, retryable: true }` (FR-022, TC-RUN-004); capability check: op requires matching manifest capability else `PermissionDenied`
  - `struct BridgedHostApi` implements `HostApi`: http via PermissionedClient; settings from Database (fallback to manifest default); secrets only if named in `manifest.permissions.secrets` else `AddonError PermissionDenied` (TC-RUN-006); storage via Database with quota; `log` writes redacted diagnostic events
  - Packaging (`package.rs`): `.rvah` file = tar.gz of add-on dir; detached signature file = ed25519 over sha256(archive bytes). `fn keygen() -> (SigningKey, VerifyingKey)`, `fn package_dir(dir, out) -> Result<[u8;32] /*sha256*/>`, `fn sign_package(pkg_path, key) -> [u8;64]`, `fn verify_package(pkg_path, sig, pubkey) -> Result<(), PackageError>` (FR-004, TC-ADD-002, TC-SEC-001)
  - Lifecycle (`install.rs`): `struct AddonManager { db, install_root }` — `install(pkg_path, sig, pubkey, InstallDecision) -> Result<InstalledAddon, InstallError>` (verify sig → extract to `install_root/<id>/<version>` → parse+validate manifest → SDK-range check (TC-ADD-003) → duplicate-id check → compute permission hash (sha256 of canonical permissions JSON) → record); `enum InstallDecision { Approve, Deny }` for permission grant; `update(pkg…)` — if permission hash differs from recorded grant, returns `InstallError::PermissionExpansionNeedsApproval` unless `Approve` (TC-ADD-005), preserves settings/storage/secrets (TC-ADD-004); `enable/disable(id)`; `uninstall(id, purge: bool)` (TC-ADD-007/008); `verify_installed(id)` re-hashes package vs stored hash (TC-SEC-001)

- [x] Failing tests: `in_process_runtime_lists_pages`, `runtime_times_out_slow_provider`, `capability_gate_denies_undeclared_op`, `bridged_api_denies_undeclared_secret`, `bridged_api_denies_unlisted_domain`, `package_sign_verify_roundtrip`, `tampered_package_rejected`, `install_rejects_bad_sdk_range`, `update_with_expanded_permissions_requires_reapproval`, `uninstall_purges_or_retains`
- [x] Implement; PASS; commit `feat(host): runtime with quotas, host API bridge, signed packaging, lifecycle (FR-001..007, FR-020..026, FR-004)`

### Task 8: rvah-testkit fixtures + rvah-core orchestration

**Files:**
- Create: `crates/rvah-testkit/Cargo.toml`, `src/lib.rs`, `src/fixtures.rs` (catalog_echo, resolver_static, slow_addon, bad_permissions, large_catalog providers per STP §5; `fn fixture_manifest(id, caps, network, secrets) -> String`; `fn write_fixture_package(dir) -> (pkg, sig, pubkey)`)
- Create: `crates/rvah-core/Cargo.toml`, `src/lib.rs`, `src/app.rs`, `src/errors.rs`, `src/nav.rs`
- Test: `crates/rvah-core/tests/flows.rs` (integration: browse/search/resolve→play/quarantine/favorites/resume)

**Interfaces:**
- Consumes: everything above
- Produces: `struct AppCore { manager: AddonManager, runtimes: HashMap<String, Box<dyn AddonRuntime>>, db, vault, player: PlayerController<MockPlayer or generic B> }`
  - `fn browse(&mut self, addon_id, path: Option<String>, cursor: Option<String>) -> Result<Page, CoreError>` — checks addon enabled, invokes runtime, records failure counts; 3 consecutive failures → status Quarantined + diagnostic event (TC-NFR-005)
  - `fn search(&mut self, addon_id, query) -> Result<Page, CoreError>` and `search_all(query) -> Vec<(String, Result<Page, CoreError>)>` (FR-044)
  - `fn resolve_and_play(&mut self, addon_id, item_id) -> Result<PlaybackSession, CoreError>` — resolve → `validate_source` → resume position lookup → controller.load
  - `fn tick_playback(&mut self)` — drains events through ProgressTracker → persists watch_state
  - `fn grant/revoke_permissions(addon_id)` (FR-046), `fn favorites_add/list/remove`
  - `enum CoreError { AddonDisabled, AddonNotFound, Provider(AddonError), SourceRejected(SourceRejection), Player(PlayerError), Data(String) }` with `fn class(&self) -> ErrorClass`; `enum ErrorClass { UserActionable, Recoverable, Developer, FatalHost }` (SDD §11)
  - `struct NavHistory` — `push(page_ref)/back()/forward()` (FR-042)

- [x] Failing tests: `browse_catalog_echo_root_and_nested`, `pagination_over_large_catalog`, `malformed_provider_response_surfaces_error_host_survives` (TC-RUN-003 — a fixture returning invalid data mapped to Developer class), `repeated_failures_quarantine_addon`, `resolve_direct_mp4_and_full_playback_with_resume` (TC-PLY-001/008), `expired_source_rejected_with_retryable_error` (TC-PLY-005), `search_single_addon`, `nav_history_back_forward`, `favorites_roundtrip`
- [x] Implement; PASS; commit `feat(core): use-case orchestration, quarantine, navigation, playback persistence`

### Task 9: rvah-cli developer CLI (mock UI)

**Files:**
- Create: `crates/rvah-cli/Cargo.toml`, `src/main.rs`, `src/commands.rs`
- Test: `crates/rvah-cli/tests/cli.rs` (spawn binary via `env!("CARGO_BIN_EXE_rvah")`)

**Interfaces:**
- Consumes: all crates. Binary name `rvah`.
- Commands (hand-rolled arg parsing, no clap dependency needed but clap 4 acceptable):
  - `rvah validate <manifest.toml>` — prints OK or validation errors, exit 1 on invalid
  - `rvah keygen -o keydir` — writes `signing.key` / `verify.pub` (hex)
  - `rvah package <addon-dir> -o pkg.rvah --key keydir/signing.key` — creates package + `pkg.rvah.sig`
  - `rvah install <pkg.rvah> --pubkey keydir/verify.pub --data-dir DIR [--approve]` — shows permissions, installs
  - `rvah addons --data-dir DIR` / `rvah enable|disable|uninstall <id> [--purge]`
  - `rvah browse <id> [path] --data-dir DIR` — runs fixture simulator runtime, prints items (FR-027 simulator)
  - `rvah resolve <id> <item-id> --data-dir DIR` — prints validated sources
  - `rvah play <id> <item-id> --data-dir DIR` — mock-player session, prints progress events and stores resume state
  - `rvah diagnostics --data-dir DIR` — prints recent redacted events (FR-103)

- [x] Failing CLI tests: `validate_accepts_good_rejects_bad_manifest`, `package_install_browse_flow` (end-to-end with fixture add-on dir written by testkit), `install_rejects_tampered_package`, `diagnostics_output_contains_no_secret`
- [x] Implement; PASS; commit `feat(cli): developer CLI and simulator (mock UI)`

### Task 10 (best-effort): Wasmtime sandbox runtime

**Files:**
- Create: `crates/rvah-addon-host/src/wasm_runtime.rs` behind `feature = "wasm"`; guest fixture `fixtures/wasm-echo/` (own crate, `crate-type = ["cdylib"]`, target wasm32-unknown-unknown)

**Interfaces:**
- `WasmRuntime::load(module_bytes, limits: WasmLimits { max_memory_bytes, fuel, timeout }) -> Result<Self>` implementing `AddonRuntime`
- ABI v0: guest exports `rvah_alloc(len: i32) -> i32` and `rvah_invoke(ptr: i32, len: i32) -> i64` (packed ptr<<32|len of JSON `Result<AddonResponse, AddonError>`); host passes JSON-serialized `AddonOp`; host imports `rvah_host_call(ptr,len)->i64` (JSON HostApi dispatch: `{"fn":"http_fetch",...}`) capability-checked via BridgedHostApi
- Enforcement: `StoreLimits` memory cap (TC-RUN-005), fuel metering + epoch deadline for CPU/wall-clock (TC-RUN-004)
- [x] Tests: `wasm_echo_lists_page`, `wasm_infinite_loop_trapped_by_fuel`, `wasm_memory_hog_capped`
- [x] If wasmtime build proves impractical in-session, document status in README and keep `AddonRuntime` boundary; commit whatever compiles green. (Built fine; guest fixtures embedded as WAT instead of a wasm32 cdylib crate — no extra toolchain target needed.)

### Task 11: Workspace polish

- [ ] `cargo fmt --all`, `cargo clippy --workspace -- -D warnings` clean, full `cargo test --workspace` green
- [ ] Write `README.md`: what RVAH is, scope decision (not Kodi-compatible), crate map, CLI quickstart with fixture add-on, security model summary, what's deferred (real UI, mpv backend, repositories/FR-008, DASH)
- [ ] Commit `docs: README and workspace polish`

## Self-Review Notes

- Spec coverage: FR-001..007 (Task 7), FR-020..027 (Tasks 1,7,9,10), FR-040..046 partially — full GUI is Phase 2 per SDD roadmap; CLI provides the Phase 1 "mock UI" with browse/search/error/permission surfaces (Tasks 8-9); FR-060..067 (Task 6,8; DASH/subtitle-backend marked deferred, DRM excluded), FR-080..085 (Tasks 3,5,7), FR-100..105 (Task 4; FR-104 post-MVP). FR-008/FR-045/FR-104 are Post-MVP per SRD — excluded.
- Type consistency: `Page/PageItem/AddonError` names used identically across Tasks 1,7,8,9. `AddonRuntime::invoke(AddonOp)` consistent in Tasks 7,8,10.
