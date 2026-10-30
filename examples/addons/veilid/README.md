# Veilid Media provider

`com.rvah.stream.veilid` is an ABI v0.1 catalog/search/resolve guest. It reads
one declared setting, `catalog`, containing a JSON array:

```json
[{"id":"demo","title":"My authorized demo clip"}]
```

IDs contain 1–128 ASCII letters, digits, underscores or hyphens. Titles are
nonempty and at most 512 bytes. The catalog is limited to 1,000 entries and
256 KiB, with 50 results per page. Search matches titles case-insensitively.
Unknown fields and duplicate IDs are rejected. The default empty catalog
contains no bundled media.

Resolution returns `rvah+veilid://media/demo` with protocol `veilid`. The
browser host must separately register an authorized session for that ID.
The guest never receives routes, relay/session/profile keys, upstream URLs
or authentication headers. It requests no network, storage or secrets.
Native playback currently rejects these references because it has no
Veilid-to-player adapter.

Build, test and sign from this directory:

```sh
env RUSTFLAGS= cargo test --locked
sh build.sh
rvah validate dist/manifest.toml
rvah package dist -o veilid.rvah --key /private/path/signing.key
```

Build requires the `wasm32-unknown-unknown` Rust target. Keep signing keys and
generated packages outside source control. Install the package, detached
signature and independently trusted public key through the browser add-on
UI, approve its declared permissions, then set the catalog and configure the
matching host session. See `../../../web/README.md` for transport and playback
setup. Local provider tests verify metadata behavior only; they are not a
live Veilid-network or browser codec compatibility test.
