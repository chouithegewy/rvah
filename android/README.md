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

For Real-Debrid: Settings → Real-Debrid Downloads → enter `api_token` → Save.
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
trusted-signer checks, secrets, setting validation, and quarantine recovery.
The Kotlin side has JVM unit tests (`./gradlew :app:testDebugUnitTest`), but
first-frame rendering, D-pad focus, hardware decode, and audio sync still
need a physical Android TV smoke test. Catalog pagination ("load more") is
not wired on Android yet; only the first page of a directory is shown.
