# RVAH web development

This is a static, installable PWA backed by Rust/WebAssembly profile, add-on,
and relay boundaries. It can enroll and synchronize encrypted profiles, verify
and run constrained signed add-ons in a Web Worker, and play bounded clear
media. Desktop and Android remain the full native hosts.

## Start developing

Prerequisites: the repository Rust toolchain, `wasm32-unknown-unknown`,
`wasm-pack`, Node 24+, and npm.

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-pack --locked
cd web
npm install
npm run dev
```

`npm run dev` rebuilds the RVAH WASM package once and starts Vite. After Rust
changes, run `npm run wasm:dev` again; Vite reloads the generated package. Pure
HTML/CSS/JavaScript changes reload immediately. The build wrapper clears
machine-global `RUSTFLAGS` for reproducibility; set `RVAH_RUSTFLAGS` if a local
toolchain deliberately needs custom flags.

Useful checks:

```sh
npm run check
npm run test:mock-veilid # two in-memory nodes; zero sockets or external requests
npm run build
npm run smoke # production build + real headless-Chrome profile/IndexedDB/PWA flow
cargo test -p rvah-profile -p rvah-sync -p rvah-stream-relay -p rvah-web
cargo check -p rvah-web --target wasm32-unknown-unknown
```

For profile-sync and private-route development without contacting a real
Veilid network, enter `mock://local` as the bootstrap while running Vite on a
development or loopback origin. The test double implements DHT writer/read
separation, private-route import/release, and bounded application calls wholly
in memory. It never imports the Veilid WASM binding, opens a socket, or calls
`fetch`. Use synthetic credentials and media even though nothing leaves the
process. Run `npm run test:mock-veilid` for an isolated two-node test.

## Signed add-ons and playback

The browser accepts a `.rvah` package, detached signature, and publisher public
key. Rust verifies the signature and SHA-256 before bounded decompression. The
UI displays the publisher fingerprint and manifest permissions, then requires
explicit approval before starting catalog/search/resolve calls in a dedicated
worker. The browser bridge supplies approved settings only: network, secrets,
storage, and logging are denied even if the manifest declares them.

Resolved finite HTTP media is capped at 16 MiB and checked against the signed
network allowlist. A canonical `rvah+veilid://media/<id>` source instead
requires a separately imported, expiring private-session descriptor. The
descriptor and media remain in memory and are never written to IndexedDB or a
service-worker cache.

`/relay/` is a manual operator proof of concept. It encrypts one authorized
finite file into RAM-only frames, creates a short-lived Veilid private route,
and downloads a descriptor for the intended client. A verified receipt or TTL
expiry releases the frames. This is not an unattended proxy, transcoder,
adaptive streamer, or DRM bypass.

## Account workflow

1. Export a linked native profile with `rvah profile-export`, or create a new
   browser profile.
2. Import the `.rvah-profile` and its separately transferred recovery key.
3. The browser validates/decrypts in WASM, then persists only ciphertext plus a
   recovery key wrapped by a non-extractable WebCrypto AES-GCM key in IndexedDB.
4. Add manual API-key settings if needed. Existing secret values are listed by
   name only and never rendered.
5. Pair another browser with the ten-minute ECDH request/response flow, or
   publish only encrypted chunks to a Veilid record.

Pairing responses grant access to the profile but not the Veilid writer key.
Only the device that created a DHT record can update it. Rotating the profile
key protects future snapshots, but cannot erase credentials or ciphertext a
previously enrolled device cached. Provider tokens must also be revoked for
strong device revocation.

## Veilid browser setup

The pinned `@bmv437/veilid-wasm` binding is loaded only when Connect is clicked.
Browser nodes cannot use UDP/TCP or DNS/TXT discovery, so enter a direct
WebSocket bootstrap:

```sh
VITE_VEILID_BOOTSTRAP=ws://bootstrap-host:5150/ws npm run dev
VITE_VEILID_BOOTSTRAP=wss://bootstrap.example.com/ws npm run build
```

An HTTPS frontend must use `wss://`. The reverse proxy must serve `.wasm` as
`application/wasm`. The development server also sets COOP/COEP headers; mirror
these in production if future threaded WASM modules require them:

```text
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

The pinned Veilid binding currently contains a generated dynamic-code loader,
so test its initialization before enforcing a Content Security Policy that
omits `unsafe-eval`. The RVAH profile WASM module itself does not need dynamic
code evaluation.

## Production build

```sh
cd web
npm ci
npm run build
```

Serve `web/dist/` as static files. The build contains application code only,
never `.rvah-profile` files, profile keys, Veilid writer keys, provider tokens,
or deployment-specific upstream credentials.
