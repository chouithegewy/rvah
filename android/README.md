# RVAH Android TV

The Android app is a native Kotlin/Compose TV shell. Rust owns add-on trust,
Wasmi sandboxing, permissions, networking, settings, secrets policy, source
selection, and watch history. Media3 owns the activity-scoped player, surface,
media session, lifecycle, and remote controls.

Requirements: JDK 17, Android SDK/API 36, NDK, Gradle 8.11+, and `cargo-ndk`.

```sh
sh android/build-rust.sh
cd android
./gradlew :app:assembleDebug -Prvah.trustedKeys=$(cat ../keys/verify.pub)
```

`build-rust.sh` packages `arm64-v8a`, `armeabi-v7a`, and `x86_64` native
libraries and generates the UniFFI Kotlin sources (both gitignored). All
exported Rust calls are blocking; the app makes them on `Dispatchers.IO`,
including opening the core at startup.

## Trusted keys and installing add-ons

A build installs only `.rvah` packages signed by one of its trusted Ed25519
keys, given as hex (comma-separated for several) through the
`rvah.trustedKeys` Gradle property — on the command line as above, or once in
`~/.gradle/gradle.properties`. A build without keys can install nothing and
says so on the Install screen.

Copy a package and its detached signature into the app's add-on folder (no
storage permission needed; the Install screen shows the exact path):

```sh
adb push real-debrid.rvah real-debrid.rvah.sig \
  /sdcard/Android/data/com.rvah.tv.debug/files/addons/
```

Install add-on → pick the package → review the consent screen (signer
fingerprint, capabilities, network hosts, storage, secrets) → Approve. The
core re-verifies the signature on approval and refuses the install if the
package's permissions no longer match what was shown.

For Real-Debrid: Settings → Real-Debrid Downloads → **Link account**. The TV
shows a code; open `real-debrid.com/device` on a phone or computer, sign in
there, and enter it. The TV notices within a few seconds and shows the linked
account. The tokens go straight to the Keystore-backed vault, and the host
refreshes the access token when it expires, so linking is a one-time step.
Typing your private `api_token` (real-debrid.com/apitoken) on the same
screen still works. Version 0.2.0 of the add-on is needed for linking.

An add-on that failed three times in a row is quarantined; the same screen
re-enables it once its credentials are fixed. Diagnostics lists recent core
events, including add-ons whose runtime failed to load.

## Remote

D-pad moves between controls and Select activates them. In the player, the
play/pause button has focus first; media keys (play/pause, fast-forward,
rewind) work regardless of focus; Back stops playback and returns to the
catalog.

## Verification status

Rust side: `cargo test -p rvah-android` covers install preview and consent,
trusted-signer checks, secrets, setting validation, quarantine recovery, page
cursors for browse and search, and device-code sign-in;
the crate cross-compiles for `arm64-v8a` and `armeabi-v7a` with NDK 27.
Kotlin side: `./gradlew :app:testDebugUnitTest` runs the JVM tests;
`assembleDebug`, `assembleDebugAndroidTest`, and the R8-minified
`assembleRelease` build (JNA and UniFFI classes survive shrinking).
`PlaybackMediaInstrumentedTest` needs a device or emulator
(`connectedDebugAndroidTest`), since `MediaItem` uses `android.net.Uri`.
First-frame rendering, D-pad focus, hardware decode, and audio sync still
need a physical Android TV smoke test.

Browse and search lists end in a "Load more" row while the add-on reports
another page; the new rows take focus as they land. Loaded pages survive a
trip to the player or into a subdirectory, and Back refocuses the row that was
opened. The paging rules are in `Listing.kt` (JVM-tested in `ListingTest`).
