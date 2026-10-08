# RVAH Add-on Developer Guide

RVAH add-ons are signed packages that implement a narrow, typed provider
interface. The host mediates everything else: network, storage, secrets,
settings, and playback.

## The provider model

An add-on implements up to seven operations (`rvah-addon-sdk::AddonProvider`):

| Operation | Capability required | Purpose |
|---|---|---|
| `list(path, cursor)` | `catalog` | Directory-style browsing; returns a `Page` of items |
| `search(query, filters, cursor)` | `search` | Normalized search results |
| `resolve(item_id, source_id)` | `resolve` | Turn an item into playable sources (or `AuthRequired`/`Unavailable`) |
| `account_status()` | `account_status` | Signed-in state for the UI |
| `auth_begin()` | `auth` | Start a device-code sign-in: user code, verification URL, poll interval |
| `auth_poll(device_code)` | `auth` | `Pending`, `Denied`, or `Authorized(credentials)` |
| `auth_refresh()` | `auth` | Trade stored refresh material for fresh credentials |

Pages contain `DirectoryItem`s (navigation) and `MediaItem`s (playable).
Pagination is cursor-based: return `cursor_next` until the last page.

Host services come through the `HostApi` context handed to every call:
`http_fetch` (allowlisted domains only), `setting_get`, `secret_get`
(declared secrets only), `storage_get/set/delete` (quota'd), `log`
(redacted). There is no other I/O: no sockets, no filesystem, no processes.

## Signing in (`auth`)

Device-code sign-in is host-managed (UC-04). The host calls `auth_begin`,
shows the user code and verification URL (which must be `https`), and calls
`auth_poll` no faster than the interval you return, until the code expires.
When `auth_poll` returns `Authorized(credentials)`, the host writes each
entry to the vault under its name. Every name must be declared in
`permissions.secrets`, or the whole result is refused. Credentials never
reach the UI. Read them later with `secret_get`.

Refresh is central too. When any operation fails with `AddonErrorCode::Auth`
and your add-on declares `auth`, the host calls `auth_refresh` once, stores
the returned credentials the same way, and retries the operation once. So
map an expired token to an `Auth` error and keep whatever `auth_refresh`
needs (a refresh token, a per-user client) in a declared secret. The
Real-Debrid example stores its access token as `api_token` and its refresh
material as `oauth_client`.

## The manifest (`manifest.toml`)

```toml
[addon]
id = "com.example.myaddon"          # reverse-DNS, lowercase
name = "My Add-on"
version = "1.0.0"                    # semver
sdk = ">=0.1, <0.2"                  # host SDK range
entry = "module.wasm"                # runtime module
capabilities = ["catalog", "search", "resolve"]

[permissions]
network = ["api.example.com", "*.cdn.example.com"]  # bare domains only
storage = "10MiB"
secrets = ["api_token"]

[settings.region]
type = "string"
default = "us"
enum = ["us", "eu", "jp"]
```

Rules enforced at install: valid reverse-DNS id, semver version, SDK range
compatible with the host, bare-domain network entries (no schemes/paths;
`*.` wildcards match subdomains only), and explicit user approval of the
permission block. Updates that change the permission set require
re-approval.

## Build, sign, package

```sh
rvah validate myaddon/manifest.toml
rvah keygen -o keys
rvah package myaddon -o myaddon.rvah --key keys/signing.key
```

A `.rvah` package is a tar.gz of your add-on directory with a detached
ed25519 signature (`myaddon.rvah.sig`) over its sha256.

## Test against the simulator

```sh
rvah install myaddon.rvah --pubkey keys/verify.pub --data-dir ./dev --approve
rvah browse com.example.myaddon --data-dir ./dev
rvah resolve com.example.myaddon <item-id> --data-dir ./dev
rvah play com.example.myaddon <item-id> --data-dir ./dev
rvah diagnostics --data-dir ./dev
```

The simulator runs your manifest against the host's fixture provider so you
can exercise install, permissions, browsing surfaces, and playback plumbing
end-to-end.

## Run for real in the desktop app

`rvah-app` executes installed **wasm** add-ons in the Wasmtime sandbox
(ABI v0.1: `rvah_alloc`/`rvah_invoke` exports, JSON-serialized ops; the
`env.rvah_host_call` import dispatches the full `HostApi` — `http_fetch`,
`setting_get`, `secret_get`, `storage_*`, `log`). The bridge enforces the
manifest's allowlist (with DNS pinning), storage quota, declared secrets,
and capability gating; memory, fuel, and wall-clock limits contain the
guest. Packages whose `entry` is not a `.wasm` module fall back to the
fixture provider (dev simulator behavior).

Write the add-on as a Rust `cdylib` for `wasm32-unknown-unknown` —
`examples/addons/real-debrid` is a complete, tested reference (catalog +
resolve + account_status against a real HTTP API, with a secret). Typical
loop:

```sh
./examples/addons/real-debrid/build.sh          # dist/ = manifest + module.wasm
rvah package dist -o addon.rvah --key keys/signing.key
rvah install addon.rvah --pubkey keys/verify.pub --data-dir DIR --approve
printf '%s' "$TOKEN" | rvah secret-set <addon-id> <secret-name> --data-dir DIR
rvah-app   # same --data-dir (or XDG default)
```

## Publish to a repository

```sh
rvah repo-index ./packages -o index.json --key keys/signing.key --base-url https://repo.example.com
# serve index.json, index.json.sig, and the .rvah files at the base URL
```

Users then check and install updates:

```sh
rvah repo-check   --index https://repo.example.com/index.json --pubkey repo.pub --data-dir DIR
rvah repo-install com.example.myaddon --index … --pubkey repo.pub --data-dir DIR --approve
```

Channels: entries default to `stable`; pass `--channel <id>=<channel>` at
index time and `--channel <channel>` at check/install time for beta tracks.

## Telemetry

RVAH collects none. The only data that leaves a user's machine is the
diagnostics bundle they explicitly export, and it passes through secret
redaction first. Add-ons must not attempt their own analytics collection;
repository policy treats hidden data collection as grounds for removal.

## Error handling contract

Return structured `AddonError`s with an appropriate code (`Network`, `Auth`,
`NotFound`, `Quota`, `Timeout`, `Internal`, `Malformed`,
`PermissionDenied`). The host maps codes to user-facing error classes;
`retryable` errors get retry affordances in the UI. Panics and hangs are
contained (timeouts, quarantine after 3 consecutive failures) but count
against your add-on — fail fast with an error instead.
