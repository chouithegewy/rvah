# RVAH Security Review — Phase 3

Review of the implementation against the SDD §9 risk table. Each risk lists
the mitigation as built, the tests that pin it, and known gaps.

## Risk-by-risk

### Malicious add-on exfiltrates secrets
- **Built:** secrets live in `rvah-auth` (`EncryptedFileVault`, AES-256-GCM,
  key file 0600) keyed by `(addon_id, name)`; `BridgedHostApi::secret_get`
  denies any name not declared in `manifest.permissions.secrets`; values read
  through the bridge are auto-registered with the `Redactor` so they cannot
  reach diagnostics.
- **Tests:** `vault_roundtrip_and_isolation_by_addon`,
  `bridged_api_denies_undeclared_secret`, `log_redacts_fetched_secrets`,
  `registered_secret_never_survives` (property).
- **OS keychain (Phase 9):** `KeychainVault` (rvah-auth `keychain` feature)
  stores secrets in the freedesktop Secret Service, one item per
  `(addon_id, name)` with searchable attributes; D-Bus transport uses DH
  session encryption. `rvah-app --features keychain` prefers it and falls
  back to the encrypted file vault when no daemon answers. Test:
  `keychain_roundtrip_isolation_and_list` (skips without a daemon; verified
  against gnome-keyring).
- **Gap:** the keychain item scope is the user session, so any process in
  the session with Secret Service access can read the items — the file
  vault's key-file permission model does not apply there.
- **Device-code sign-in (UC-04):** credentials an add-on returns from
  `auth_poll`/`auth_refresh` are written to the vault by the host, only
  under names declared in `permissions.secrets` (all-or-nothing), and never
  returned to the UI or across the Android FFI. The device code stays in
  the core. `Debug` for `Credentials`, `PollOutcome` and `DeviceAuth` prints
  names only, so an unexpected-response error message cannot carry a token
  into diagnostics. The verification URL shown to the user must be `https`.
  Polls are paced to the provider's interval by the core, not the UI.
  An `Auth` failure triggers at most one refresh and one retry, and only
  for add-ons that declare `auth`.
- **Tests:** `device_code_sign_in_stores_credentials_only_in_the_vault`,
  `sign_in_refuses_secret_names_the_manifest_does_not_declare`,
  `polls_sooner_than_the_interval_never_reach_the_provider`,
  `an_expired_token_is_refreshed_once_and_the_call_retried`,
  `add_ons_without_the_auth_capability_are_never_refreshed` (rvah-core
  flows), `credentials_never_debug_print_their_values` (SDK),
  `poll_outcome_debug_hides_secret_values` (rvah-auth), and
  `real_debrid_guest_links_refreshes_and_retries` (rvah-android, ignored
  until the guest is built), which also checks no token reaches diagnostics.

### Broad web scraping / arbitrary hosts
- **Built:** all add-on HTTP flows through `PermissionedClient`: scheme check
  (http/https only), allowlist (exact or `*.` wildcard on label boundary, no
  IP literals under wildcards), request-count quota — all before the
  transport runs.
- **Tests:** `denies_unlisted_domain_before_transport`, `denies_file_scheme`,
  `request_quota_enforced`, `wildcard_only_matches_true_subdomains` and
  `wildcard_never_permits_ipv4` (property).
- **Redirect policy (Phase 6):** the transport never follows redirects;
  `PermissionedClient` follows up to 5 hops itself, re-running scheme +
  allowlist checks and consuming quota on every hop (303 downgrades to a
  body-less GET). Tests: `follows_redirect_within_allowlist`,
  `redirect_to_disallowed_host_denied`, `redirect_loop_hits_hop_limit`.
- **DNS pinning (Phase 7):** with a resolver installed
  (`PermissionedClient::with_resolver`, production uses `SystemResolver`),
  every hop's host is resolved once, the addresses are rejected if any is
  non-global (loopback, RFC-1918, link-local, CGNAT, ULA, IPv4-mapped
  disguises — DNS rebinding), and the transport connects only to the checked
  addresses (`execute_pinned`; `UreqTransport` installs a pinning
  `ureq::Resolver`). An IP literal that passes the allowlist by exact match
  is treated as an explicit user grant. Tests:
  `resolver_pins_checked_addresses_through_transport`,
  `name_resolving_to_non_global_address_denied`,
  `redirect_hop_re_resolves_and_denies_private_target`,
  `allowlisted_ip_literal_is_an_explicit_grant`.
- **Gap:** pinning covers one `fetch`; a later `fetch` re-resolves (by
  design — records would go stale), so a slow-flux name can still move
  between calls, within the allowlist and non-global policy.

### Add-on hangs or allocates excessive memory
- **Built:** `InProcessRuntime` enforces a wall-clock timeout per invoke;
  `WasmRuntime` adds `StoreLimits` memory caps, fuel metering, and an epoch
  watchdog. Three consecutive failures quarantine the add-on (`AppCore`).
- **Tests:** `runtime_times_out_slow_provider`,
  `wasm_infinite_loop_trapped_by_fuel`, `wasm_memory_hog_capped`,
  `repeated_failures_quarantine_addon`.
- **Android (Wasmi):** `WasmiRuntime` has the same memory cap and fuel, and
  enforces the wall-clock budget (`min(limits.timeout, ctx.timeout)`) at
  every host-call boundary, before and after the call, since Wasmi has no
  epoch interruption. Pure compute between host calls is bounded by fuel.
  Both runtimes share one ABI module (`abi.rs`: host-call dispatch and
  overflow-checked guest pointer ranges).
- **Tests:** `rvah-addon-host/tests/wasmi_limits.rs`,
  `hostile_host_call_pointers_do_not_panic_the_host` (both runtimes).
- **Gap:** the in-process runtime cannot bound memory (native code); only the
  wasm runtime provides real containment. Timed-out native provider threads
  are stranded, not killed.

### Package tampering
- **Built:** `.rvah` packages carry a detached ed25519 signature over the
  archive sha256; install verifies before extraction; tar extraction rejects
  path traversal; `verify_installed` re-hashes the stored package against the
  recorded hash; repository indexes are signature-verified before parsing and
  downloads hash-checked against the index.
- **Tests:** `tampered_package_rejected`, `install_rejects_tampered_package`
  (CLI), `tampered_index_rejected`, `download_hash_mismatch_rejected`.
- **Consent/runtime manifest mismatch (fixed):** a signed archive could carry
  `manifest.toml` twice; the install preview and recorded permissions read the
  first entry while extraction left the last one on disk for the runtime's
  allowlist and capabilities. Every package read now walks the whole archive
  first and refuses duplicate paths (case-insensitively, for macOS/Windows
  extraction), `..`/absolute paths, and anything but regular files and
  directories (symlinks, hard links, devices); extraction validates before
  writing anything.
- **Tests:** `duplicate_manifest_is_refused_by_preview_and_extraction`,
  `dot_prefixed_duplicate_is_refused`, `case_colliding_entries_are_refused`,
  `symlink_entries_are_refused` (`rvah-addon-host/tests/package_adversarial.rs`).

### Resolver output reaching native players
- **Built:** `validate_source_opts` gates every `PlayableSource` before mpv or
  Media3 sees it: the URL must be an absolute `http`/`https` URL with a host
  regardless of the declared protocol (bare paths, drive letters, `file:`,
  and mpv/ffmpeg pseudo-protocols such as `edl://`, `av://`, `lavf://` are
  refused), subtitle URLs obey the same rule, header names are limited to
  `User-Agent`/`Referer`, and header values may not contain control
  characters (no CRLF request-line smuggling). DRM-flagged sources fail
  closed.
- **Tests:** `rvah-player/tests/source_adversarial.rs`.
- **Accepted:** loopback and private-network hosts stay playable — local
  media servers are a legitimate add-on use, and the player only streams the
  response, it never hands it back to the add-on.

### Insecure update
- **Built:** updates re-verify signatures; a changed permission hash requires
  explicit re-approval (`PermissionExpansionNeedsApproval`); repo updates run
  through the same `AddonManager::update` path.
- **Tests:** `update_with_expanded_permissions_requires_reapproval`,
  `install_from_repo_end_to_end`.

### Log leakage
- **Built:** `Redactor` masks registered secret values plus pattern matches
  (`Authorization:`, `Cookie:`/`Set-Cookie:`, `Bearer`, `api_key=`,
  `access_token=`, `cookie=`) at write time in the host bridge, and again at
  display/export time (CLI diagnostics, UI diagnostics export).
- **Tests:** `redactor_masks_registered_and_patterns`,
  `cookie_headers_and_pairs_are_masked`,
  `diagnostics_output_contains_no_secret` (CLI end-to-end),
  `bearer_tokens_pattern_masked` (property).
- **Gap:** pattern list is fixed; it should keep growing with the bridge API
  surface.

### License contamination
- **Built:** no Kodi code or assets; the workspace is original code. All
  dependencies come from crates.io. `cargo deny` runs in CI (licenses,
  advisories, sources); the license allowlist and accepted-advisory
  rationale live in `deny.toml`. Wasmtime is pinned to the 36 LTS line after
  the 2026 Winch/pooling advisories (RVAH uses neither Winch nor the pooling
  allocator, but runs the patched release regardless).

## Additional observations

1. The manifest parser is property-tested to never panic on arbitrary input;
   accepted ids are re-validated against the reverse-DNS rule.
2. Player source validation refuses `file:` URLs even when the declared
   protocol lies (`validator_rejects_unsupported_protocol`), and the header
   policy allows only User-Agent/Referer through to mpv.
3. Wasm ABI v0.1 host-call dispatch covers the full `HostApi`
   (http/settings/secrets/storage/log) but adds no policy of its own —
   every call lands in `BridgedHostApi`, so wasm guests face exactly the
   same permission checks as in-process providers.
4. mpv runs as a separate process; a compromised add-on cannot reach its IPC
   socket through any host API (sockets are host-only, no path is exposed).
