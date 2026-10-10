# Real-Debrid Downloads (wasm add-on)

Browse your [Real-Debrid](https://real-debrid.com) downloads history and
play the direct links. This is the reference *real-service* add-on: a Rust
crate compiled to `wasm32-unknown-unknown`, speaking ABI v0.1
(`rvah_alloc`/`rvah_invoke` exports, `env.rvah_host_call` import).

- **catalog** — `GET /rest/1.0/downloads`, paged by RD's page numbers
  (the RVAH cursor); item id → direct URL is cached in add-on storage.
- **search** — RD cannot search the history, so the add-on reads it newest
  first, 200 entries per call, and returns the downloads whose file name
  contains every word of the query, in any order (`coyote acme` finds
  `Coyote.vs.Acme.2026.mkv`). **Load more** reads the next 200.
- **resolve** — the cached direct URL (RD download entries are already
  unrestricted), with a history re-scan fallback.
- **account_status** — `GET /rest/1.0/user`.
- **auth** — device-code sign-in with Real-Debrid's open-source client id:
  the app shows a code, you enter it at <https://real-debrid.com/device>,
  and the host stores the access token as `api_token` and the refresh
  material as `oauth_client`. When the access token expires (401), the host
  refreshes it and retries, so you only link once.
- **secrets** — `api_token` and `oauth_client`. Instead of linking, you can
  set `api_token` to your private token from
  <https://real-debrid.com/apitoken>. Missing or rejected tokens surface as
  user-visible auth errors.

## Build & install

```sh
rustup target add wasm32-unknown-unknown   # once
./build.sh                                 # → dist/{manifest.toml,module.wasm}

rvah keygen -o keys                        # once
rvah package dist -o real-debrid.rvah --key keys/signing.key
rvah install real-debrid.rvah --pubkey keys/verify.pub --data-dir <DIR> --approve
```

Then run `rvah-app` with the same data dir and choose **Sign in** next to the
add-on, or **Link account** in its settings on Android TV. To use a private
token instead:

```sh
printf '%s' "$YOUR_TOKEN" | rvah secret-set com.real-debrid.downloads api_token --data-dir <DIR>
```

The permission grant covers exactly `api.real-debrid.com`, 64 KiB of
storage, and the two named secrets. Version 0.2.0 adds `auth` and
`oauth_client`, so updating from 0.1.0 asks for approval again; 0.3.0 adds
search without new permissions.

Each call gets 50M instructions of fuel and 2 s. Host answers arrive as
JSON with HTTP bodies as arrays of byte values, so `host.rs` decodes them
without `serde_json::Value` and reads each body once; that keeps a page of
100 history entries near 12M fuel.

## Tests

`cargo test` in this directory runs the logic natively against a scripted
host (no network, no wasm toolchain needed). After `./build.sh`,
`cargo test -p rvah-android --test real_debrid_guest -- --ignored` in the
repository root runs the built module under the Android sandbox through
sign-in, token expiry and refresh.
