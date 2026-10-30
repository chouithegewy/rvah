# Portable profiles and Veilid transport

Status: portable profiles, exact signed-package reconstruction, Android
import/export, browser enrollment and pairing, and the browser Veilid DHT
adapter are implemented. A live Veilid deployment has not been configured or
network-verified. The bounded relay core, manual in-browser private-route
sender/receiver, signed browser add-on runtime, finite clear-media player, and
metadata-only Veilid add-on are implemented. An unattended upstream proxy,
transcoding, native-player adapters, and live end-to-end deployment remain
integration work.

## What RVAH saves today

RVAH deliberately separates ordinary application state from credentials:

- `rvah.sqlite` contains installed add-on metadata and grants, explicit add-on
  settings, quota-limited add-on storage, favorites, watch state, cache metadata,
  and diagnostics.
- Linux desktop builds use the freedesktop Secret Service when the `keychain`
  feature can connect to it. Otherwise `secrets.vault` contains an AES-256-GCM
  encrypted map and `vault.key` contains its device-local random key.
- Android stores one encrypted value per `rvah/<addon-id>/<name>` in private
  preferences. The non-exportable AES-256-GCM wrapping key lives in Android
  Keystore.
- Add-ons can request only secret names declared in their signed manifest. The
  host returns values only to that add-on and redacts them from diagnostics.

`Link account` currently writes provider results into this vault. Trakt stores
`access_token` and `refresh_token`; Real-Debrid stores `api_token` and its OAuth
refresh material. TMDB, MDBList, Prowlarr, and SubDL use user-supplied API keys.
Copying the SQLite database alone therefore does not copy a linked account.

## Implemented portable profile

The host can now create one versioned profile containing:

- installed add-on IDs, source versions, and an optional exact package digest;
- explicit user settings; and
- the named secrets already held for those add-ons.

It intentionally excludes package bytes, repository/trust configuration,
permission grants, caches, diagnostics, favorites, watch history, and arbitrary
add-on storage. A plain import is an overlay: it neither deletes target-only
values nor silently grants an add-on new permissions, and it may happen before
packages are installed. `profile-restore` can instead verify a signed repository,
locate the exact recorded version and package digest, install it after an
explicit `--approve`, preserve its enabled/disabled state, and then apply the
settings and credentials. The target must approve permissions again.

The `.rvah-profile` body is authenticated and encrypted with AES-256-GCM under
a random 256-bit profile recovery key. A fresh nonce is used for every export,
and the format marker is authenticated as associated data. Bundles are capped
at 512 KiB. Key and decrypted buffers are zeroed when released where the Rust
type boundary permits it. The key is a separate 0600 file and must never be
uploaded beside the bundle.

```sh
rvah profile-keygen -o household.profile.key
rvah profile-export -o household.rvah-profile \
  --key household.profile.key --data-dir "$XDG_DATA_HOME/rvah"

# On another Linux device, after transferring the key out of band:
rvah profile-import household.rvah-profile \
  --key household.profile.key --data-dir "$XDG_DATA_HOME/rvah"

# Optional: verify and reconstruct the exact signed add-ons first.
rvah profile-restore household.rvah-profile \
  --key household.profile.key --index index.json \
  --pubkey verify.pub --data-dir "$XDG_DATA_HOME/rvah"
# Review the plan, then repeat with --approve.
```

Android Settings now exposes a Portable profile screen. It wraps the recovery
key with a non-exportable Android Keystore AES-GCM key and uses Android's
document picker to import/export the encrypted bundle and recovery key as
separate documents. The browser PWA similarly persists ciphertext in IndexedDB
and wraps the recovery key with a non-extractable WebCrypto key. Do not build a
"pre-signed-in" public ELF, APK, or web bundle. An embedded reusable credential
is extractable and cannot be revoked per device. Distribute identical public
application builds, then enroll each device into the encrypted profile.

## Sign in once: target architecture

Treat the profile key as a small household root key and each device as its own
identity.

1. The first device generates the profile key and a Veilid record owner key.
2. It encrypts a profile snapshot locally and publishes only ciphertext.
3. A new browser device generates its own ephemeral keypair and exports a
   ten-minute pairing request containing its public key and a one-time challenge.
4. The existing browser verifies the request and encrypts the profile bundle,
   profile key, reader record key, and epoch to that device with ECDH P-256,
   HKDF-SHA-256, and AES-256-GCM. The DHT writer key is deliberately withheld.
5. The new device stores its unwrapped profile key in Secret Service, Android
   Keystore, or a non-extractable WebCrypto key wrapper in IndexedDB.
6. Devices watch the profile head, authenticate an increasing epoch, download
   the ciphertext, decrypt locally, and overlay the new settings and tokens.

The recovery-key file is the offline fallback. A passphrase-wrapped recovery
format can be added later, using a reviewed memory-hard KDF rather than treating
a human password as an AES key.

Revoking a device requires rotating the profile key and publishing a new epoch
for the remaining devices. Because an already enrolled device may have cached
provider refresh tokens, strong revocation also requires revoking those tokens
at each provider and linking again. Cryptographic access revocation cannot erase
plaintext a previously authorized device already saw.

### Veilid record layout

The browser adapter uses Veilid as an untrusted replication layer:

- subkey 0: signed head `{format, epoch, ciphertext_sha256, chunk_count}`;
- subkeys 1..N: chunks of the already encrypted `.rvah-profile` bytes;
- a DFLT owner schema initially; delegated SMPL writers only after conflict and
  revocation behavior is implemented;
- one writer publishes every encrypted chunk, advances the head last, and
  flushes the record; readers force fresh reads and ignore incomplete,
  reordered, hash-mismatched, or stale epochs.

The shared `rvah-sync` crate fixes the profile cap at 512 KiB, splits it into at
most 22 chunks of 24 KiB, and derives the 23-subkey DFLT schema count exported
to JavaScript. The transport-neutral assembly boundary is covered by
digest/layout/epoch tests. A real bootstrap/node deployment and a
multi-device network smoke test are still required before calling remote sync
operational.

Veilid's DHT records are signed and current DHT operations are transport
encrypted, but application encryption remains mandatory: caching nodes and
anyone given a record key must still see only RVAH ciphertext. The official
[DHT documentation](https://veilid.gitlab.io/developer-book/concepts/dht.html)
also limits an entire value, including its schema and subkeys, to 1 MiB. A
profile should stay far below that limit.

A remote `veilid-server` can improve availability by keeping the profile record
open and rehydrated, but it is not the account authority and never receives the
profile key. GitHub, the add-on repository, and the public web root contain code
and signed packages only—never profile ciphertext plus its key, and never raw
credentials.

## Public web and private services

Use the public RVAH hostname for a static/PWA frontend and Veilid WebSocket
bootstrap. Keep Prowlarr and the media relay on loopback or a private container
network. Clients should reach them through authenticated RVAH/Veilid operations,
not a public Prowlarr listener. Deployment hostnames and upstream addresses
belong in server-side deployment configuration, not this public repository.

Veilid core supports desktop, mobile, and in-browser WebAssembly applications.
The implemented browser target is a signed-in profile/enrollment PWA with
IndexedDB, WebCrypto key wrapping, expiring pairing, WebSocket bootstrap, and a
shared Rust/WASM profile codec. It also verifies signed `.rvah` packages before
bounded decompression and runs catalog/search/resolve calls in a Web Worker
using Wasmi. Browser add-ons receive approved settings but no host HTTP,
secrets, storage, logs, profile key, Veilid node key, or relay session key.
Clear finite HTTP sources and verified private-route relay descriptors can be
played from bounded memory. Desktop and Android remain the full native hosts;
the browser path does not provide DRM, adaptive streaming, background proxying,
or persistent media storage.

## Transient media relay: bounded core and manual browser path implemented

Veilid's documented `BLOC` block-store capability is
[not currently implemented](https://veilid.gitlab.io/developer-book/concepts/nodes.html),
and the DHT's 1 MiB value ceiling makes it the wrong data plane for video. The
first media implementation should use Veilid only for discovery, session
control, private routes, and peer availability.

The `rvah-stream-relay` crate now implements the transport-neutral security and
retention core:

- a random 256-bit key per stream session;
- independently AES-256-GCM-authenticated frames of at most 48 KiB plaintext,
  with session, segment, ordering, length, and whole-segment digest bound as
  associated data;
- client-side reassembly and whole-segment SHA-256 verification;
- HMAC-SHA-256 delivery receipts covering session ID, segment number, byte
  length, and plaintext digest with a domain-separated receipt key;
- discard immediately after every required client has acknowledged, plus strict
  TTL, segment-count, and byte-count eviction; and
- no filesystem API or disk persistence in the relay core.

Implemented proof-of-concept components:

- `rvah-stream-relay` encrypts bounded frames in RAM, reassembles them, verifies
  whole-media digests, authenticates receipts, and discards acknowledged data;
- `/relay/` is an explicit operator page that accepts one authorized finite file
  of at most 16 MiB, opens a short-lived private route, serves authenticated
  application-call fragments, and releases memory after receipt or expiry;
- the PWA imports an out-of-band session descriptor, fetches the private-route
  fragments, verifies and decrypts them, acknowledges acceptance, and exposes
  only a temporary object URL to its media element; and
- `com.rvah.stream.veilid` is a metadata-only example add-on that emits canonical
  `rvah+veilid://media/<id>` sources without access to node, profile, or session
  keys.

Still-required production components are an authenticated unattended upstream
proxy, optional CMAF/fMP4 conversion, congestion control/retransmission beyond
the bounded request protocol, multi-segment Media Source playback, and native
desktop/Android private-route adapters.

The acknowledgement must cover session ID, segment number, byte length, and
plaintext or ciphertext digest. A transport receipt alone proves delivery to a
network endpoint, not that the client verified and accepted the bytes. The relay
may discard a segment after the required client acknowledgement or a strict TTL,
whichever comes first. RAM limits, per-session quotas, cancellation, and
backpressure are mandatory.

Consequences of the no-persistence rule:

- arbitrary backward seeking is not guaranteed after the rolling window expires;
- if no peer retains a segment, the relay must reacquire it from the authorized
  upstream;
- availability ends when the source and all peers leave;
- transcoding necessarily exposes plaintext media in relay memory, even though
  source transport and Veilid application frames are encrypted on the wire;
- crash dumps and swap must be disabled or protected if the relay handles
  sensitive plaintext in RAM.

Only content the user is authorized to access may enter the relay. The design
does not bypass DRM or provider access controls.

## Delivery sequence

1. Portable profile library, CLI, and exact signed-package restore (implemented).
2. Android document import/export UI and Keystore-protected profile key
   (implemented; physical-TV smoke test remains).
3. Verified encrypted-snapshot layout and browser Veilid DHT adapter
   (implemented and locally tested, including a socket-free two-node mock;
   live network test remains).
4. Browser profile shell with WebCrypto/IndexedDB, ten-minute ECDH pairing, and
   optional Veilid WASM connectivity (implemented and browser-smoke-tested).
5. Signed browser add-on runtime plus bounded clear-media and private-route
   playback (implemented and Rust-tested; browser add-on UI smoke test remains).
6. Transient relay protocol and bounded in-memory reference implementation
   (implemented and unit-tested), plus a manual browser private-route sender and
   receiver (implemented and offline-startup-smoke-tested); live network and
   unattended upstream proxy tests remain.
7. Metadata-only streaming add-on (implemented); adaptive streaming,
   native/Android adapters, and cross-device end-to-end playback remain.

Each phase must keep the public build reproducible and the private enrollment
material separate.
