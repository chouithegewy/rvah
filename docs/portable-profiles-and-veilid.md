# Portable profiles and Veilid transport

Status: portable profile format and local import/export implemented; Veilid
replication, device-pairing UI, browser host, and transient media relay are a
design proposal.

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

The host can now create one versioned profile containing only:

- installed add-on IDs and their source versions (informational);
- explicit user settings; and
- the named secrets already held for those add-ons.

It intentionally excludes packages, signing trust, permission grants, caches,
diagnostics, favorites, watch history, and arbitrary add-on storage. Import is
an overlay: it neither deletes target-only values nor silently grants an add-on
new permissions. An import may happen before packages are installed.

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
```

The Android Rust boundary exposes the same generate/export/import operations;
an Android UI and safe share/pairing flow still need to be added. Do not build a
"pre-signed-in" public ELF, APK, or web bundle. An embedded reusable credential
is extractable and cannot be revoked per device. Distribute identical public
application builds, then enroll each device into the encrypted profile.

## Sign in once: target architecture

Treat the profile key as a small household root key and each device as its own
identity.

1. The first device generates the profile key and a Veilid record owner key.
2. It encrypts a profile snapshot locally and publishes only ciphertext.
3. A new device generates its own keypair and scans a short-lived QR invitation
   containing a record key, an ephemeral public key, and a one-time challenge.
4. The existing device verifies the challenge, wraps the profile key to the new
   device key, and publishes/sends that envelope over a Veilid private route.
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

Use Veilid as an untrusted replication and notification layer:

- subkey 0: signed head `{format, epoch, ciphertext_sha256, chunk_count}`;
- subkeys 1..N: chunks of the already encrypted `.rvah-profile` bytes;
- a DFLT owner schema initially; delegated SMPL writers only after conflict and
  revocation behavior is implemented;
- one writer publishes a complete new epoch, verifies network consensus, then
  advances the head; readers ignore incomplete or hash-mismatched epochs.

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

Veilid core supports desktop, mobile, and in-browser WebAssembly applications,
but a browser RVAH host is still a separate product target: it needs IndexedDB,
WebCrypto key wrapping, a browser player, WebSocket bootstrap, and a WebAssembly
runtime policy compatible with browser constraints. A static UI build alone is
not a working signed-in web client.

## Transient media relay: do not use DHT values as video storage

Veilid's documented `BLOC` block-store capability is
[not currently implemented](https://veilid.gitlab.io/developer-book/concepts/nodes.html),
and the DHT's 1 MiB value ceiling makes it the wrong data plane for video. The
first media implementation should use Veilid only for discovery, session
control, private routes, and peer availability.

Proposed components:

- `rvah-stream-relay`: reads an authorized upstream, optionally converts it to
  CMAF/fMP4 media segments, and retains only a bounded rolling RAM window;
- host-owned Veilid transport: fragments encrypted segments into application
  messages over private routes, with flow control and retransmission;
- `com.rvah.stream.veilid` add-on: resolves a content/session descriptor into
  an `rvah+veilid` playback source; it never receives node or profile keys;
- client segment cache: verifies and decrypts each segment, commits it to the
  player/cache, then returns an authenticated application-level acknowledgement.

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

1. Portable profile library and CLI (implemented).
2. Android share/import UI and protected profile-key storage.
3. Veilid profile-record adapter with a fake transport, epoch/hash tests, and
   two-device pairing/revocation tests.
4. Browser shell with WebCrypto/IndexedDB and Veilid WASM connectivity.
5. Transient relay protocol and bounded in-memory reference implementation.
6. Streaming add-on and native/browser/Android end-to-end playback tests.

Each phase must keep the public build reproducible and the private enrollment
material separate.
