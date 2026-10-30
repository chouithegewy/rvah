# Web Player for Add-ons Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an add-on return a `web` source that Android TV opens full screen in a locked-down, ad-blocking WebView. Add a `max_bytes` cap on add-on HTTP fetches, and a Home tile for add-ons that fit no Home section.

**Architecture:**
- **SDK:** gains `Protocol::Web`, `PlayableSource.never_block`, `HttpRequest.max_bytes` and `HttpResponse.truncated`.
- **Gates:** the host accepts a web source only from add-ons whose manifest declares `web = true`, and only on hosts whose `PlaybackCapabilities` has `web`. Only Android TV has it.
- **Blocking:** `rvah-android` wraps adblock-rust as a UniFFI `AdBlocker`.
- **Android:** a Kotlin `WebPlayerScreen` replaces the Media3 surface for web sources. Home grows a per-add-on tile that opens `Screen.Browse`.

**Tech Stack:** Rust (rvah-addon-sdk, rvah-net, rvah-addon-host, rvah-player, rvah-core, rvah-android with UniFFI), `adblock` 0.13.5, Kotlin + Compose for TV, Android WebView, Coil 3 (+ `coil-svg` 3.2.0).

**Spec:** `docs/superpowers/specs/2026-10-29-web-player-addon-design.md`. The private add-on that consumes this has its own plan at `~/projects/rvah-private/addons/live-sports/PLAN.md` (outside this repo).

## Global Constraints

- **Public repo:** nothing site-specific in tracked files. No site name, host, markup or fixture of the private add-on. Tests use `example.com`-style hosts.
- **Web source shape:** `Protocol::Web` serializes as `"web"`; `Protocol::from_url` never returns it. A web source must be `https://`, has no headers, and `never_block` holds at most 16 plain hostnames, only on web sources.
- **Manifest:** `[permissions] web = true`, default false. Absent or false must leave `permission_hash` unchanged for existing add-ons.
- **Capabilities:** `PlaybackCapabilities::media3()` keeps `web: false`. Only Android TV's capability set has `web: true`.
- **Blocking:** the main-frame request and `never_block` hosts are never blocked. There are no built-in host exceptions. Lists are EasyList `https://easylist.to/easylist/easylist.txt` and EasyPrivacy `https://easylist.to/easylist/easyprivacy.txt`, downloaded, cached in app files, and refreshed when 7 days old. They are never bundled.
- **Web player:** JS and DOM storage on, autoplay without gesture, third-party cookies on, file and content access off, `javaScriptCanOpenWindowsAutomatically = false`, pop-ups refused, main-frame navigation off the source host refused, downloads ignored, permission requests denied, screen kept on.
- **Remote:** OK / Enter / NumpadEnter / Play-Pause tap the centre; Back leaves fullscreen first and otherwise exits; D-pad arrows are consumed.
- **`max_bytes`:** an optional cap on bytes read. `truncated` is true only when the body was longer than the cap.
- **Build environment** (this machine):
  - `JAVA_HOME=/home/david/Unity/Hub/Editor/6000.3.25f1/Editor/Data/PlaybackEngines/AndroidPlayer/OpenJDK`
  - `ANDROID_HOME=/home/david/Unity/Hub/Editor/6000.3.25f1/Editor/Data/PlaybackEngines/AndroidPlayer/SDK`
  - `ANDROID_NDK_HOME=/home/david/Unity/Hub/Editor/6000.3.25f1/Editor/Data/PlaybackEngines/AndroidPlayer/NDK`
  - `GRADLE_USER_HOME=/home/david/projects/rvah/android/.gradle-local`
  - The system Java 25 is too new for Gradle.
- **Device:** the Shield is `adb -s 192.168.4.205:5555`. Never touch the Quest on USB.

## Review Focus

1. **Older add-ons:** add-ons installed before this change must keep their `permission_hash`, or updating them would re-prompt or be refused. Owner: Task 3, test `permission_hash_unchanged_without_web`.
2. **Non-Android hosts:** an add-on returning `web` on desktop or CLI should get a clear "unsupported protocol" error, not a crash or a Media3 attempt. Owner: Task 4, test `web_source_rejected_without_web_capability`. Task 5 makes the `NativeProtocol` mapping total.
3. **A `never_block` entry that is a URL** (`https://x.com/a`) or a wildcard is refused, not silently matched. Owner: Task 4, test `never_block_must_be_plain_hosts`.
4. **A redirect hop inside a capped fetch** must keep the cap; otherwise a 302 to a 1 MB page bypasses it. Owner: Task 2, test `max_bytes_survives_redirect`.
5. **Back pressed while the page is still loading**, or the web player closed by Home or Back, must end the playback session, so the core doesn't think something is still playing. Owner: Task 8, by calling `owner.endWebSession` from `DisposableEffect.onDispose` (verified on device in Task 10).

---

### Task 1: SDK web source types

**Files:**
- Modify: `crates/rvah-addon-sdk/src/types.rs` (the `Protocol` enum near line 98, `PlayableSource` near line 205)
- Modify: `crates/rvah-addon-sdk/tests/prop.rs` (exhaustive `match` on `Protocol`)
- Modify: `docs/portable-profiles-and-veilid.md` only if it lists the protocols; also `crates/rvah-addon-sdk` docs or README if they enumerate protocols (grep `Veilid` in `*.md`)

**Interfaces:**
- Produces:
  - `Protocol::Web` (serde `"web"`).
  - `PlayableSource.never_block: Vec<String>` with `#[serde(default, skip_serializing_if = "Vec::is_empty")]`.

- [ ] **Step 1: Write the failing tests** in the `types.rs` test module:

```rust
#[test]
fn web_protocol_round_trips_as_web() {
    assert_eq!(serde_json::to_string(&Protocol::Web).unwrap(), "\"web\"");
    assert_eq!(serde_json::from_str::<Protocol>("\"web\"").unwrap(), Protocol::Web);
}

#[test]
fn from_url_never_infers_web() {
    for url in ["https://ex.com/page.php", "https://ex.com/", "http://ex.com/embed"] {
        assert_ne!(Protocol::from_url(url), Some(Protocol::Web));
    }
}

#[test]
fn never_block_defaults_empty_and_is_skipped() {
    let s: PlayableSource = serde_json::from_str(r#"{"url":"https://ex.com/","protocol":"web"}"#).unwrap();
    assert!(s.never_block.is_empty());
    assert!(!serde_json::to_string(&s).unwrap().contains("never_block"));
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p rvah-addon-sdk web_protocol never_block from_url_never`
Expected: compile error, because `Protocol::Web` / `never_block` don't exist yet.

- [ ] **Step 3: Implement.**
  - Add `Web` with a doc comment ("An https web page the host opens in its locked-down web player; never inferred from a URL") after `Veilid`.
  - Add the `never_block` field ("Host suffixes a web player must never block; web sources only").
  - Fix every `PlayableSource { .. }` literal in the workspace by adding `never_block: Vec::new()` (`grep -rn 'PlayableSource {' crates examples`).
  - In `tests/prop.rs`, add `Protocol::Web => prop_assert!(false, "from_url inferred web")`.
  - The compiler then lists every other exhaustive `match` on `Protocol` (`rvah-player` validate, `rvah-core` playback, `rvah-android`). Add a `Protocol::Web` arm that behaves like today's `Protocol::File` (refuse), so the workspace builds. Tasks 4 and 5 replace those arms.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rvah-addon-sdk && cargo build --workspace`
Expected: PASS, and the workspace builds.

- [ ] **Step 5: Commit**

```bash
git add -A crates && git commit -m "feat(sdk): web protocol and never_block on playable sources"
```

---

### Task 2: `max_bytes` on add-on HTTP fetches

**Files:**
- Modify: `crates/rvah-addon-sdk/src/provider.rs` (`HttpRequest`, `HttpResponse`)
- Modify: `crates/rvah-addon-sdk/src/guest.rs`:
  - add `http_get_capped`;
  - make `ScriptedHost` honour `max_bytes`;
  - `HttpResponse` literals at about lines 67 and 422.
- Modify: `crates/rvah-net/src/client.rs`:
  - `UreqTransport::execute` body read (line ~117), and `execute_pinned`;
  - `PermissionedClient::fetch` `hop_req` (line ~238).
- Modify: `crates/rvah-net/src/mock.rs` (`MockTransport::execute`)
- Modify: every other `HttpResponse { .. }` literal; add `truncated: false`:
  - `crates/rvah-repo/src/lib.rs:112`
  - `crates/rvah-addon-host/src/wasm_runtime.rs:320`
  - `crates/rvah-android/tests/{subdl,prowlarr,real_debrid,tmdb,discover}_guest.rs`
- Test: `crates/rvah-net/src/client.rs` tests module; `crates/rvah-addon-sdk/src/guest.rs` tests module

**Interfaces:**
- Produces:
  - `HttpRequest.max_bytes: Option<u64>`, with `#[serde(default, skip_serializing_if = "Option::is_none")]`; `HttpRequest::get` sets `None`.
  - `HttpResponse.truncated: bool`, with `#[serde(default, skip_serializing_if = "std::ops::Not::not")]`.
  - `pub fn http_get_capped(host: &dyn Host, url: &str, headers: BTreeMap<String, String>, max_bytes: u64) -> Result<HttpResponse, AddonError>` in `rvah_addon_sdk::guest`.
  - `fn read_capped(reader: impl Read, cap: u64) -> std::io::Result<(Vec<u8>, bool)>` (private, `rvah-net`).

- [ ] **Step 1: Write the failing tests.**

In `rvah-net/src/client.rs` tests:

```rust
#[test]
fn max_bytes_truncates_and_flags() {
    let mock = MockTransport::default();
    mock.route("GET", "https://api.example.com/big", 200, vec![b'x'; 1000]);
    let c = PermissionedClient::new(Box::new(mock), Allowlist::new(["api.example.com"]), 10);
    let mut req = HttpRequest::get("https://api.example.com/big");
    req.max_bytes = Some(100);
    let resp = c.fetch(&req).unwrap();
    assert_eq!(resp.body.len(), 100);
    assert!(resp.truncated);
}

#[test]
fn max_bytes_larger_than_body_is_not_truncated() { /* cap 5000 on 1000 bytes: len 1000, !truncated */ }

#[test]
fn max_bytes_survives_redirect() {
    // route_redirect /old -> /big (1000 bytes); cap 100 on /old: len 100, truncated
}

#[test]
fn read_capped_reads_one_past_cap_only() {
    let (body, cut) = read_capped(&b"abcdef"[..], 4).unwrap();
    assert_eq!((body.as_slice(), cut), (&b"abcd"[..], true));
    let (body, cut) = read_capped(&b"abcd"[..], 4).unwrap();
    assert_eq!((body.as_slice(), cut), (&b"abcd"[..], false));
}
```

In `guest.rs` tests:

```rust
#[test]
fn scripted_host_honours_max_bytes() {
    let mut host = ScriptedHost::default();
    host.http.insert("https://ex.com/p".into(), (200, vec![b'y'; 500]));
    let resp = http_get_capped(&host, "https://ex.com/p", BTreeMap::new(), 64).unwrap();
    assert_eq!(resp.body.len(), 64);
    assert!(resp.truncated);
    assert_eq!(host.requests()[0].max_bytes, Some(64));
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p rvah-net max_bytes read_capped && cargo test -p rvah-addon-sdk --features guest scripted_host_honours`
Expected: compile errors (no `max_bytes`, `truncated`, `read_capped`, `http_get_capped`).

- [ ] **Step 3: Implement.**
  - **`read_capped`:** reads `cap + 1` bytes with `take`. If it got more than `cap`, it truncates to `cap` and returns `true`.
  - **`UreqTransport`:** both `execute` and `execute_pinned` use `read_capped` with `req.max_bytes.unwrap_or(32 MiB).min(32 MiB)`. Keep today's 32 MiB ceiling when no cap is given; at that ceiling, `truncated` stays false, as today.
  - **`PermissionedClient::fetch`:** copies `max_bytes` into `hop_req`.
  - **`MockTransport`:** applies the same truncation to the routed body.
  - **`http_get_capped`:** sends `max_bytes` through the existing `http_send` path.
  - **`ScriptedHost`:** the `http_fetch` branch truncates the routed body and sets `truncated`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rvah-net && cargo test -p rvah-addon-sdk --all-features && cargo test -p rvah-addon-host && cargo build --workspace --tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A crates && git commit -m "feat(net): max_bytes cap on add-on HTTP fetches"
```

---

### Task 3: Manifest `web` permission and install consent

**Files:**
- Modify: `crates/rvah-addon-host/src/manifest.rs` (`RawPermissions` ~line 50, `PermissionsSection` ~line 65, `Manifest::parse` ~line 146)
- Modify: `crates/rvah-android/src/lib.rs` (`InstallPreview` ~line 113, `preview_install` ~line 829)
- Modify: `android/app/src/main/java/com/rvah/tv/RvahTvApp.kt` (`ConsentPanel` ~line 1282)
- Modify: `SECURITY.md` (one line: add-ons may open web pages only when they declare `web = true`; shown at install)
- Test: `crates/rvah-addon-host/src/manifest.rs` tests module

**Interfaces:**
- Produces:
  - `RawPermissions.web: bool` (`#[serde(default)]`).
  - `PermissionsSection.web: bool` (`#[serde(default, skip_serializing_if = "std::ops::Not::not")]`).
  - `InstallPreview.web: bool`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn web_permission_defaults_false_and_parses_true() {
    assert!(!Manifest::parse(BASE).unwrap().permissions.web);
    let with = BASE.replace("[permissions]", "[permissions]\nweb = true");
    assert!(Manifest::parse(&with).unwrap().permissions.web);
}

#[test]
fn permission_hash_unchanged_without_web() {
    let plain = Manifest::parse(BASE).unwrap().permission_hash();
    let explicit_false = BASE.replace("[permissions]", "[permissions]\nweb = false");
    assert_eq!(plain, Manifest::parse(&explicit_false).unwrap().permission_hash());
    let on = BASE.replace("[permissions]", "[permissions]\nweb = true");
    assert_ne!(plain, Manifest::parse(&on).unwrap().permission_hash());
}
```

`BASE` is the module's existing test manifest that has a `[permissions]` table; reuse it, or add a small one.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p rvah-addon-host web_permission permission_hash_unchanged`
Expected: compile error, because there is no field `web`.

- [ ] **Step 3: Implement.**
  - Thread `web` from `RawPermissions` into `PermissionsSection`.
  - Set `InstallPreview.web` from the manifest.
  - In `ConsentPanel`, add `if (preview.web) Body("• open web pages full screen")` after the secrets line.

- [ ] **Step 4: Run the tests, regenerate the bindings, and check the app compiles**

Run: `cargo test -p rvah-addon-host && cargo test -p rvah-android && PATH=$HOME/.cargo/bin:$PATH android/build-rust.sh && (cd android && ./gradlew --offline :app:testDebugUnitTest)`
Expected: PASS. The regenerated `InstallPreview` binding has `web`, and `ConsentPanel` compiles.

- [ ] **Step 5: Commit**

```bash
git add -A crates android SECURITY.md && git commit -m "feat(addons): web permission in manifests and install consent"
```

---

### Task 4: Validation and core gating of web sources

**Files:**
- Modify: `crates/rvah-player/src/validate.rs`, `crates/rvah-player/src/lib.rs` (export)
- Modify: `crates/rvah-core/src/playback.rs` (`PlaybackCapabilities`)
- Modify: `crates/rvah-core/src/app.rs`:
  - `prepare_playback` ~line 466, `prepare_release` ~line 505;
  - `prepare_resolved` ~line 527, and the validation closure ~line 558.
- Test: `crates/rvah-player/src/validate.rs` tests; `crates/rvah-core` tests (follow the existing `prepare_playback` tests' fixture style)

**Interfaces:**
- Consumes:
  - `Protocol::Web`, `PlayableSource.never_block` (Task 1);
  - `PermissionsSection.web` (Task 3).
- Produces:
  - `pub struct SourcePolicy { pub dash: bool, pub web: bool }`, deriving `Debug, Clone, Copy, Default`.
  - `pub fn validate_source_with(source: &PlayableSource, now_unix: u64, policy: SourcePolicy) -> Result<(), SourceRejection>`.
    `validate_source_opts(s, now, allow_dash)` becomes a wrapper with `web: false`.
  - New `SourceRejection` variants:
    - `InvalidNeverBlock(String)`: `#[error("never_block entry is not a host name: {0}")]`.
    - `WebNotDeclared(String)`: `#[error("add-on {0} did not declare permission to open web pages")]`.
  - `PlaybackCapabilities.web: bool`. `media3()` and `for_backend` set `false`; `pub const fn android_tv() -> Self` is `media3()` with `web: true`.
  - `prepare_resolved(&mut self, resp, resolver: &str, progress: (&str, &str), capabilities)`.
    `resolver` is the add-on that produced the source: `addon_id` from `prepare_playback`, `debrid_id` from `prepare_release`.

- [ ] **Step 1: Write the failing tests.**

In `validate.rs`:

```rust
fn web(url: &str) -> PlayableSource { let mut s = source(Protocol::Web, url); s.protocol = Protocol::Web; s }
const WEB: SourcePolicy = SourcePolicy { dash: false, web: true };

#[test]
fn web_needs_the_policy() {
    assert!(matches!(validate_source_with(&web("https://ex.com/p"), 0, SourcePolicy::default()),
        Err(SourceRejection::UnsupportedProtocol(_))));
    assert!(validate_source_with(&web("https://ex.com/p"), 0, WEB).is_ok());
}

#[test]
fn web_must_be_https_and_headerless() {
    assert!(validate_source_with(&web("http://ex.com/p"), 0, WEB).is_err());
    let mut s = web("https://ex.com/p");
    s.headers.insert("Referer".into(), "https://ex.com".into());
    assert!(matches!(validate_source_with(&s, 0, WEB), Err(SourceRejection::ForbiddenHeader(_))));
}

#[test]
fn never_block_must_be_plain_hosts() {
    let mut s = web("https://ex.com/p");
    s.never_block = vec!["cdn.example.net".into(), "Example.ORG".into()];
    assert!(validate_source_with(&s, 0, WEB).is_ok());
    for bad in ["https://x.com", "x.com/a", "*.x.com", "x.com:443", "", "-x.com", "x..com"] {
        s.never_block = vec![bad.into()];
        assert!(matches!(validate_source_with(&s, 0, WEB), Err(SourceRejection::InvalidNeverBlock(_))), "{bad}");
    }
    s.never_block = (0..17).map(|i| format!("h{i}.example.com")).collect();
    assert!(validate_source_with(&s, 0, WEB).is_err());
    let mut hls = source(Protocol::Hls, "https://ex.com/a.m3u8");
    hls.never_block = vec!["cdn.example.net".into()];
    assert!(validate_source_with(&hls, 0, WEB).is_err());
}
```

In `rvah-core`, test with the existing scripted or fake add-on harness used by the `prepare_playback` tests:

```rust
#[test]
fn web_source_rejected_without_web_capability() // media3(): Err(SourceRejected(UnsupportedProtocol(_)))
#[test]
fn web_source_rejected_when_addon_lacks_web_permission() // android_tv(), manifest without web: Err(SourceRejected(WebNotDeclared(id)))
#[test]
fn web_source_prepared_for_declared_addon_on_android_tv() // android_tv(), web = true: Ok, source.protocol == Web
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p rvah-player web_ never_block && cargo test -p rvah-core web_source`
Expected: compile errors (no `SourcePolicy`, `android_tv`, new variants).

- [ ] **Step 3: Implement.**
  - **Validator:** in `validate_source_with`, replace the Task 1 placeholder arm with:
    - `Protocol::Web if policy.web` → require scheme `https` (else `UnsupportedProtocol("web over http")`);
    - `Protocol::Web` otherwise → `UnsupportedProtocol("web")`.
  - **Never-block check:** after the protocol match:
    - non-empty `never_block` on a non-web source → `InvalidNeverBlock`;
    - more than 16 entries → `InvalidNeverBlock`;
    - any entry that isn't a hostname (labels of `[a-z0-9-]` 1–63 chars, not starting or ending with `-`, separated by single dots; case-insensitive) → `InvalidNeverBlock`.
  - **Headers:** for web, any header at all is `ForbiddenHeader`.
  - **Capabilities:** `PlaybackCapabilities::supports` gets `Protocol::Web => self.web`.
  - **Core gate:** in the core closure, before validating, if `s.protocol == Protocol::Web && !self.installed_manifest(resolver)?.permissions.web`, record `WebNotDeclared(resolver)` and skip. Read the manifest once before the closure.
  - **Call:** call `validate_source_with(s, now, SourcePolicy { dash: capabilities.dash, web: capabilities.web })`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rvah-player && cargo test -p rvah-core && cargo test --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A crates && git commit -m "feat(core): gate web sources on host capability and add-on permission"
```

---

### Task 5: Android bridge — web playback and bindings

**Files:**
- Modify: `crates/rvah-android/src/lib.rs`:
  - `NativeProtocol` ~line 430;
  - `NativePreparedPlayback` and `From<PreparedPlayback>` ~line 470–500;
  - `prepare_playback` ~line 1027, `prepare_release` ~line 1055.
- Modify: `android/app/src/main/java/com/rvah/tv/PlaybackMedia.kt` (`toSpec` `when`)
- Test: `crates/rvah-android/src/lib.rs` tests (near line 1588); `android/app/src/test/java/com/rvah/tv/PlaybackMediaTest.kt`

**Interfaces:**
- Consumes: `PlaybackCapabilities::android_tv()` (Task 4).
- Produces:
  - `NativeProtocol::Web`.
  - `NativePreparedPlayback.never_block: Vec<String>` (Kotlin: `neverBlock: List<String>`).
  - Kotlin `NativePreparedPlayback.isWeb: Boolean` (extension in `PlaybackMedia.kt`: `protocol == NativeProtocol.WEB`).
  - `toSpec()` throws `IllegalArgumentException("web sources open in the web player")` for WEB.

- [ ] **Step 1: Write the failing tests.**

Rust:

```rust
#[test]
fn web_source_maps_to_native_web_with_never_block() {
    // build a PreparedPlayback whose source is Protocol::Web, never_block ["cdn.example.net"]
    let native = NativePreparedPlayback::from(prepared);
    assert!(matches!(native.protocol, NativeProtocol::Web));
    assert_eq!(native.never_block, vec!["cdn.example.net".to_string()]);
}
```

Kotlin (`PlaybackMediaTest`):

```kotlin
@Test fun webPreparedPlaybackIsWebAndHasNoMediaSpec() {
    val p = prepared(protocol = NativeProtocol.WEB, url = "https://ex.com/p")
    assertTrue(p.isWeb)
    assertThrows(IllegalArgumentException::class.java) { p.toSpec() }
}
```

Use the test file's existing builder for `NativePreparedPlayback`, adding `neverBlock = emptyList()`.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p rvah-android web_source_maps`
Expected: compile error.

- [ ] **Step 3: Implement.**
  - Map `Protocol::Web => NativeProtocol::Web`, replacing Task 1's placeholder. The `unreachable!` arm keeps only `File | Veilid`.
  - Copy `source.never_block`.
  - Both `prepare_playback` and `prepare_release` pass `PlaybackCapabilities::android_tv()`.
  - In Kotlin, add `isWeb`, and make `toSpec()` throw for WEB.

- [ ] **Step 4: Regenerate the bindings and run the tests**

Run, from the repo root with the Global Constraints env:

```
PATH=$HOME/.cargo/bin:$PATH android/build-rust.sh
(cd android && ./gradlew --offline :app:testDebugUnitTest)
```

Expected: the Rust tests pass, the bindings regenerate, and the Kotlin tests pass. Task 3's `ConsentPanel` change compiles now.

- [ ] **Step 5: Commit**

```bash
git add -A crates android/app/src && git commit -m "feat(android): bridge web sources to Kotlin"
```

---

### Task 6: The ad blocker (`AdBlocker` over adblock-rust)

**Files:**
- Create: `crates/rvah-android/src/adblock.rs`
- Modify: `crates/rvah-android/src/lib.rs` (`mod adblock; pub use adblock::AdBlocker;`)
- Modify: `crates/rvah-android/Cargo.toml`: `adblock = { version = "0.13.5", default-features = false, features = ["embedded-domain-resolver", "full-regex-handling"] }`. Without the default `single-thread` feature, the engine can sit behind `Arc`.
- Test: tests module in `adblock.rs`

**Interfaces:**
- Produces, as a UniFFI object:
  - `AdBlocker::new(lists: Vec<String>) -> Arc<AdBlocker>` (`#[uniffi::constructor]`). Each string is a whole list's text; network rules only.
  - `AdBlocker::should_block(&self, url: String, source_url: String, request_type: String) -> bool`. Unparseable requests return `false`.
  - `AdBlocker::rule_count(&self) -> u32`, for the log line.
  - Kotlin: `AdBlocker(lists)`, `.shouldBlock(url, sourceUrl, requestType)`.

- [ ] **Step 1: Write the failing tests**

```rust
const RULES: &str = "||tracker.example^\n||ads.example^$third-party\n@@||ads.example/ok^\n";

#[test]
fn blocks_listed_tracker() {
    let b = AdBlocker::new(vec![RULES.into()]);
    assert!(b.should_block("https://tracker.example/t.js".into(), "https://site.example/".into(), "script".into()));
    assert!(!b.should_block("https://cdn.example.net/v.ts".into(), "https://site.example/".into(), "xmlhttprequest".into()));
}

#[test]
fn exceptions_and_garbage_are_allowed() {
    let b = AdBlocker::new(vec![RULES.into()]);
    assert!(!b.should_block("https://ads.example/ok".into(), "https://site.example/".into(), "image".into()));
    assert!(!b.should_block("not a url".into(), "https://site.example/".into(), "other".into()));
}

#[test]
fn empty_lists_block_nothing() {
    assert!(!AdBlocker::new(vec![]).should_block("https://tracker.example/".into(), "https://s.example/".into(), "script".into()));
}

fn _send_sync() { fn check<T: Send + Sync>() {} check::<AdBlocker>(); }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p rvah-android adblock`
Expected: compile error (no module).

- [ ] **Step 3: Implement** with `adblock::FilterSet::new(false)`, `add_filters(&lines, ParseOptions::default())`, `Engine::from_filter_set(set, true)`, `Request::new(url, source_url, request_type)`, `engine.check_network_request(&req).matched`.
  - If `Engine` turns out not to be `Sync` under these features, hold it in a `Mutex<Engine>`; the `_send_sync` check must compile either way.
  - Log the rule count once via the crate's existing debug logger.

- [ ] **Step 4: Run the tests, then check the 32-bit Shield build compiles**

Run: `cargo test -p rvah-android adblock && PATH=$HOME/.cargo/bin:$PATH android/build-rust.sh`
Expected: PASS; `librvah_android.so` builds for `armeabi-v7a` (the Shield's ABI); the Kotlin binding has `AdBlocker`.

- [ ] **Step 5: Commit**

```bash
git add -A crates Cargo.lock android/app/src && git commit -m "feat(android): AdBlocker over adblock-rust"
```

---

### Task 7: Web policy helpers and filter-list cache (Kotlin, JVM-tested)

**Files:**
- Create: `android/app/src/main/java/com/rvah/tv/WebPolicy.kt`
- Create: `android/app/src/main/java/com/rvah/tv/FilterLists.kt`
- Test: `android/app/src/test/java/com/rvah/tv/WebPolicyTest.kt`

**Interfaces:**
- Consumes: Kotlin `AdBlocker` (Task 6).
- Produces in `WebPolicy.kt`, all pure:
  - `fun requestType(url: String, isMainFrame: Boolean, accept: String?): String` returns one of `"document"`, `"script"`, `"stylesheet"`, `"image"`, `"media"`, `"font"`, `"xmlhttprequest"`, `"other"`:
    - main frame → `"document"`;
    - else by extension (`js`, `css`, `png|jpg|jpeg|gif|webp|svg|ico`, `m3u8|ts|mp4|m4s|aac`, `woff|woff2|ttf`);
    - else by `accept` (`text/html` → `"document"`, which is a sub-frame; `image/` → image; `text/css` → stylesheet);
    - else `"xmlhttprequest"` when `accept` contains `json`, and `"other"` otherwise.
  - `fun neverBlocked(host: String, neverBlock: List<String>): Boolean`: case-insensitive; equal, or ends with `".$entry"`.
  - `fun navigationAllowed(sourceUrl: String, targetUrl: String, isMainFrame: Boolean): Boolean`: sub-frames always; main frame only when the host equals the source's host.
  - `fun listsStale(lastModifiedMs: Long, nowMs: Long): Boolean`: true when `lastModifiedMs == 0L` or `nowMs - lastModifiedMs >= 7 * 24 * 3600 * 1000L`.
- Produces in `FilterLists.kt`:
  - `internal object FilterLists`, with:
    - `const val EASYLIST = "https://easylist.to/easylist/easylist.txt"`;
    - `const val EASYPRIVACY = "https://easylist.to/easylist/easyprivacy.txt"`;
    - `suspend fun blocker(context: Context): AdBlocker?`, which caches the built engine in memory.
  - `blocker` reads `filesDir/filters/{easylist,easyprivacy}.txt`. When `listsStale` (by file mtime), it re-downloads on `Dispatchers.IO` with `HttpURLConnection` (15 s timeouts), writes to a temp file, and renames.
  - It builds `AdBlocker(texts)` from whatever is on disk, or returns `null` when nothing is. Download failures are logged and fall back to the cached copy.
  - `fun warmUp(context: Context, scope: CoroutineScope)` launches `blocker` in the background.

- [ ] **Step 1: Write the failing tests** (`WebPolicyTest.kt`):

```kotlin
@Test fun mainFrameIsDocument() = assertEquals("document", requestType("https://a.example/x.php", true, null))
@Test fun typesByExtensionThenAccept() {
    assertEquals("script", requestType("https://a.example/p.js?v=2", false, null))
    assertEquals("media", requestType("https://a.example/seg/1.ts", false, null))
    assertEquals("document", requestType("https://b.example/embed.php", false, "text/html,application/xhtml+xml"))
    assertEquals("image", requestType("https://b.example/i", false, "image/avif,image/webp"))
    assertEquals("other", requestType("https://b.example/i", false, null))
}
@Test fun neverBlockedMatchesHostAndSubdomains() {
    val list = listOf("cdn.example.net")
    assertTrue(neverBlocked("cdn.example.net", list)); assertTrue(neverBlocked("p16.CDN.example.net", list))
    assertFalse(neverBlocked("evilcdn.example.net", list)); assertFalse(neverBlocked("example.net", list))
}
@Test fun onlyMainFrameNavigationIsHeldToTheSourceHost() {
    val src = "https://embed.example/ch.php"
    assertTrue(navigationAllowed(src, "https://embed.example/other", true))
    assertFalse(navigationAllowed(src, "https://ads.example/pop", true))
    assertTrue(navigationAllowed(src, "https://player.example/frame", false))
}
@Test fun listsGoStaleAfterSevenDays() {
    val day = 24 * 3600 * 1000L
    assertTrue(listsStale(0L, 10 * day)); assertFalse(listsStale(day, 7 * day)); assertTrue(listsStale(day, 8 * day))
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `(cd android && ./gradlew --offline :app:testDebugUnitTest --tests '*WebPolicyTest*')`
Expected: compile failure (unresolved references).

- [ ] **Step 3: Implement** both files as specified.
  - Call `FilterLists.warmUp(applicationContext, lifecycleScope)` from `MainActivity.onCreate`, so the engine is ready before the first web playback.

- [ ] **Step 4: Run the tests**

Run: `(cd android && ./gradlew --offline :app:testDebugUnitTest)`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A android/app/src && git commit -m "feat(tv): web policy helpers and filter list cache"
```

---

### Task 8: `WebPlayerScreen` and the player branch

**Files:**
- Create: `android/app/src/main/java/com/rvah/tv/WebPlayer.kt`
- Modify: `android/app/src/main/java/com/rvah/tv/RvahTvApp.kt` (`PlayerScreen` ~line 747: branch on `isWeb`)
- Modify: `android/app/src/main/java/com/rvah/tv/RvahPlayerOwner.kt` (add `endWebSession`)

**Interfaces:**
- Consumes:
  - `NativePreparedPlayback.isWeb`, `.neverBlock`, `.url`, `.sessionId` (Task 5);
  - `requestType`, `neverBlocked`, `navigationAllowed`, `FilterLists.blocker` (Task 7);
  - `AdBlocker.shouldBlock` (Task 6).
- Produces:
  - `@Composable internal fun WebPlayerScreen(url: String, neverBlock: List<String>, onExit: () -> Unit)`.
  - `fun RvahPlayerOwner.endWebSession(sessionId: ULong)`, which calls `core.stopPlayback` on IO like `stop()` does.

- [ ] **Step 1: Implement `WebPlayerScreen`.** It has no JVM test: WebView isn't available on the JVM. The Task 7 helpers carry the logic, and Task 10 verifies on the device.
  - **Model:** `TrailerScreen` (`Trailer.kt`): an `AndroidView` WebView, black background, `stopLoading` and `destroy` on dispose.
  - **View:** a `FrameLayout` holding the WebView, so a custom fullscreen view can sit on top. `FLAG_KEEP_SCREEN_ON` is set while shown, through `LocalView.current.keepScreenOn = true` in a `DisposableEffect`.
  - **Settings, lock-down, and remote:** exactly as in Global Constraints.
  - **Blocking:** `shouldInterceptRequest` returns an empty `WebResourceResponse("text/plain", "utf-8", 204, "No Content", emptyMap(), ByteArrayInputStream(ByteArray(0)))` when all of these hold:
    - the request isn't for the main frame;
    - `neverBlocked(host, neverBlock)` is false;
    - `blocker?.shouldBlock(url, sourceUrl, requestType(url, false, request.requestHeaders["Accept"]))` is true.
  - **Blocker load:** obtain `blocker` from `FilterLists.blocker(context)` in a `LaunchedEffect` before calling `loadUrl`. If it's null, load anyway.
  - **Navigation:** `shouldOverrideUrlLoading` returns `!navigationAllowed(url, request.url.toString(), request.isForMainFrame)`.
  - **Remote keys:** handled in `onPreviewKeyEvent` on the `AndroidView` modifier, plus a `BackHandler`.
    - The centre tap dispatches `MotionEvent` `ACTION_DOWN` then `ACTION_UP` (50 ms apart) with `source = InputDevice.SOURCE_TOUCHSCREEN` at the target view's centre. The target is the custom view if one is shown, else the WebView. This is the same as the spike.
  - **Main-frame failure:** `onReceivedError` / `onReceivedHttpError` for the main frame swaps in `Page("This server isn't working") { Body("Try another server.", ErrorText) }`, and Back still exits.

- [ ] **Step 2: Branch in `PlayerScreen`.**
  - Add `var web by remember(target) { mutableStateOf<NativePreparedPlayback?>(null) }`.
  - On prepare success:
    - if `it.isWeb`, set `started = true; web = it`;
    - otherwise call `owner.play(...)` as today.
  - Before the Media3 UI: `web?.let { w -> WebPlayerScreen(w.url, w.neverBlock, onExit); DisposableEffect(w.sessionId) { onDispose { owner.endWebSession(w.sessionId) } }; return }`.

- [ ] **Step 3: Build and run the unit tests**

Run: `(cd android && ./gradlew --offline :app:testDebugUnitTest :app:assembleDebug -Prvah.trustedKeys=$(cat ../keys/verify.pub))`
Expected: BUILD SUCCESSFUL.

- [ ] **Step 4: Commit**

```bash
git add -A android/app/src && git commit -m "feat(tv): full-screen locked-down web player for web sources"
```

---

### Task 9: Home tile for add-ons that fit no section, and SVG artwork

**Files:**
- Modify: `android/app/src/main/java/com/rvah/tv/HomeLayout.kt` (`HomeLayout`, `HomeSection`, `homeSections`, `homeLayout`)
- Modify: `android/app/src/main/java/com/rvah/tv/Home.kt` (`HomeScreen` params; `HomeTiles` `onOpen` handling ~line 166)
- Modify: `android/app/src/main/java/com/rvah/tv/RvahTvApp.kt` (`HomeScreen(...)` call ~line 306: pass `onBrowse`)
- Modify: `android/app/build.gradle.kts`: `implementation("io.coil-kt.coil3:coil-svg:3.2.0")`
- Modify: `android/app/src/main/java/com/rvah/tv/Discover.kt` (`ArtworkLoader`: `.components { add(SvgDecoder.Factory()) }`)
- Test: `android/app/src/test/java/com/rvah/tv/HomeLayoutTest.kt`

**Interfaces:**
- Produces:
  - `HomeLayout.names: Map<String, String>` (add-on id → name), filled by `homeLayout`.
  - `data class BrowseTarget(val addonId: String, val addonName: String)`.
  - `HomeSection.browse: BrowseTarget? = null`; such a section has `id = "addon:<addonId>"`, `title = addonName`, `subtitle = "Browse"`, and no columns.
  - `HomeScreen(..., onBrowse: (BrowseTarget) -> Unit)`.
  - `RvahTvApp` passes `{ screen = Screen.Browse(it.addonId, it.addonName) }`.

- [ ] **Step 1: Write the failing tests**

```kotlin
@Test fun addOnWhoseFoldersAllFallToMoreGetsItsOwnBrowseTile() {
    val addons = listOf(addon(DISCOVER_ADDON, "catalog"), addon("com.example.sports", "catalog", name = "Live Sports"))
    val roots = mapOf(
        DISCOVER_ADDON to listOf(folder("Trending", "trending"), folder("Odd shelf", "odd")),
        "com.example.sports" to listOf(folder("NFL", "/nfl/"), folder("NBA", "/nba/")),
    )
    val sections = homeSections(homeLayout(addons, roots))
    val tile = sections.single { it.browse != null }
    assertEquals(BrowseTarget("com.example.sports", "Live Sports"), tile.browse)
    assertEquals("addon:com.example.sports", tile.id)
    // Its folders are not merged into More; Discover's odd shelf still is.
    assertEquals(listOf("Odd shelf"), sections.single { it.id == "more" }.columns.map { it.title })
}

@Test fun addOnWithAnyMatchingFolderStaysInSections() {
    // an add-on with folders "Trending" and "NFL": no browse tile; "NFL" goes to More
}
```

Extend the test file's `addon(...)` helper with an optional `name`, if it lacks one.

- [ ] **Step 2: Run them to see them fail**

Run: `(cd android && ./gradlew --offline :app:testDebugUnitTest --tests '*HomeLayoutTest*')`
Expected: compile failure (`browse`, `BrowseTarget`).

- [ ] **Step 3: Implement.**
  - In `homeSections`, after the keyword takes, build the tiles. Take each add-on id in `remaining` whose remaining-column count equals its total count in `layout.discover`, and which has no `layout.groups`. Remove its columns from `remaining` and add a browse `HomeSection`, after Downloads and before More, in `layout.discover` order.
  - In `HomeScreen`'s tile handler: `section.browse?.let(onBrowse) ?: run { home.tab = HomeTab.Section(section) }`.
  - Add `coil-svg` and register its decoder.

- [ ] **Step 4: Run the tests**

Run: `(cd android && ./gradlew --offline :app:testDebugUnitTest)`
Expected: PASS, including the existing `HomeLayoutTest` cases unchanged.

- [ ] **Step 5: Commit**

```bash
git add -A android && git commit -m "feat(tv): Home tile for add-ons that fit no section; SVG artwork"
```

---

### Task 10: Whole-branch verification and Shield check

**Files:** none new.

- [ ] **Step 1: Run the full Rust checks**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: all clean and passing.

- [ ] **Step 2: Run the add-on crates' own tests**, since CI loops over `examples/addons/*`:

Run: `for d in examples/addons/*/; do [ -f "$d/Cargo.toml" ] && (cd "$d" && cargo test) ; done`
Expected: all pass. They build against the changed SDK, so the `truncated` / `never_block` / `max_bytes` additions must not break them.

- [ ] **Step 3: Build and install the debug APK on the Shield**

Run: `PATH=$HOME/.cargo/bin:$PATH android/build-rust.sh && (cd android && ./gradlew --offline :app:assembleDebug -Prvah.trustedKeys=$(cat ../keys/verify.pub)) && adb -s 192.168.4.205:5555 install -r android/app/build/outputs/apk/debug/app-debug.apk`
Expected: `Success`. If adb says `unauthorized`, ask David to toggle Network debugging off and on with the TV awake.

- [ ] **Step 4: Check that existing behaviour still works on the Shield.** Home loads; a Discover title plays in Media3; installing an add-on still shows the consent panel.

- [ ] **Step 5: Hand over to the private plan.** The end-to-end web check (a Live Sports server plays after one OK, Back returns, trackers are blocked, no pop-ups) is Task 6 of `~/projects/rvah-private/addons/live-sports/PLAN.md`. Do it there.
  - If that task measures `list` above 2 s on the Shield, come back and do Task 11.
  - Otherwise skip Task 11, and finish the branch with superpowers:finishing-a-development-branch (merge locally into master).

---

### Task 11 (only if the private plan's Shield measurement needs it): a declared longer `list` deadline

**Files:**
- Modify: `crates/rvah-addon-host/src/manifest.rs` (`AddonSection`)
- Modify: `crates/rvah-core/src/app.rs` (where `DEFAULT_INVOKE_TIMEOUT` applies to `AddonOp::List`; follow how `sources_timeout` is chosen)

**Interfaces:**
- Produces: manifest `[addon] list_timeout_secs = <u32>` (optional), clamped to 2–10 s. Absent means the 2 s default.

- [ ] **Step 1: Write the failing tests:**
  - `list_timeout_defaults_to_two_seconds`;
  - `list_timeout_is_clamped_to_ten`, which parses `list_timeout_secs = 60` and asserts 10 s;
  - in core, `list_uses_the_declared_deadline`, following the existing timeout tests.
- [ ] **Step 2: Run them to see them fail.** Run: `cargo test -p rvah-addon-host list_timeout && cargo test -p rvah-core list_uses`. Expected: compile error.
- [ ] **Step 3: Implement.** Add the field (`#[serde(default)] Option<u32>`), and choose the `list` deadline from the manifest the same way `sources_timeout` is chosen.
- [ ] **Step 4: Run the tests.** Run: `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5: Commit.** Run: `git commit -am "feat(addons): optional longer list deadline in the manifest"`.
