# Real-Debrid Downloads (wasm add-on)

Browse your [Real-Debrid](https://real-debrid.com) downloads history and
play the direct links. This is the reference *real-service* add-on: a Rust
crate compiled to `wasm32-unknown-unknown`, speaking ABI v0.1
(`rvah_alloc`/`rvah_invoke` exports, `env.rvah_host_call` import).

- **catalog** — `GET /rest/1.0/downloads`, paged by RD's page numbers
  (the RVAH cursor); item id → direct URL is cached in add-on storage.
- **resolve** — the cached direct URL (RD download entries are already
  unrestricted), with a history re-scan fallback.
- **account_status** — `GET /rest/1.0/user`.
- **secret** — `api_token`, your token from
  <https://real-debrid.com/apitoken>. Missing/rejected tokens surface as
  user-visible auth errors.

## Build & install

```sh
rustup target add wasm32-unknown-unknown   # once
./build.sh                                 # → dist/{manifest.toml,module.wasm}

rvah keygen -o keys                        # once
rvah package dist -o real-debrid.rvah --key keys/signing.key
rvah install real-debrid.rvah --pubkey keys/verify.pub --data-dir <DIR> --approve
printf '%s' "$YOUR_TOKEN" | rvah secret-set com.real-debrid.downloads api_token --data-dir <DIR>
```

Then run `rvah-app` with the same data dir. The permission grant covers
exactly `api.real-debrid.com`, 64 KiB of storage, and the one named secret.

## Tests

`cargo test` in this directory runs the logic natively against a scripted
host (no network, no wasm toolchain needed).
