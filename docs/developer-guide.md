# RVAH Add-on Developer Guide

RVAH add-ons are signed packages that implement a narrow, typed provider
interface. The host mediates everything else: network, storage, secrets,
settings, and playback.

## The provider model

An add-on implements up to four operations (`rvah-addon-sdk::AddonProvider`):

| Operation | Capability required | Purpose |
|---|---|---|
| `list(path, cursor)` | `catalog` | Directory-style browsing; returns a `Page` of items |
| `search(query, filters, cursor)` | `search` | Normalized search results |
| `resolve(item_id, source_id)` | `resolve` | Turn an item into playable sources (or `AuthRequired`/`Unavailable`) |
| `account_status()` | `account_status` | Signed-in state for the UI |

Pages contain `DirectoryItem`s (navigation) and `MediaItem`s (playable).
Pagination is cursor-based: return `cursor_next` until the last page.

Host services come through the `HostApi` context handed to every call:
`http_fetch` (allowlisted domains only), `setting_get`, `secret_get`
(declared secrets only), `storage_get/set/delete` (quota'd), `log`
(redacted). There is no other I/O: no sockets, no filesystem, no processes.

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
end-to-end. Wasm module execution (ABI v0: `rvah_alloc`/`rvah_invoke`
exports, JSON-serialized ops) runs under the `wasm` feature of
`rvah-addon-host`; host-call dispatch currently supports `log`.

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
