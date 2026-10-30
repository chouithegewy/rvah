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

Settings → Portable profile creates or imports a profile recovery key and
protects it with an Android Keystore AES-256-GCM key. Use the document picker
to export/import an encrypted `.rvah-profile` and its recovery-key text file as
separate documents. Import the key first on a new device. Imported settings and
credentials are overlaid, but add-on packages and permissions are never
silently installed or granted; use the native CLI's signed `profile-restore`
flow when exact package reconstruction is required.

An add-on that failed three times in a row is quarantined; the same screen
re-enables it once its credentials are fixed. Diagnostics lists recent core
events, including add-ons whose runtime failed to load.

## Remote

D-pad moves between controls and Select activates them. In the player, the
play/pause button has focus first; media keys (play/pause, fast-forward,
rewind) work regardless of focus; Back stops playback and returns to the
catalog.

Home keeps discovery lightweight: search is at the top, then two columns of
Trending & popular, Recently added, Genres, Trakt, Downloads, and any remaining
collections. Opening one loads a poster grid; moving focus updates its synopsis
and ratings pane. A small first batch of artwork and details is prefetched into
bounded Coil caches. A valid YouTube trailer opens in an in-app embed, and the
first trailer starts after four idle seconds on a focused title.

## Playback buffer

Media is never cached to disk; ExoPlayer reads ahead in memory (`BufferPlan.kt`).
The read-ahead stops at 120 s or at a byte budget: half the Java heap
(`largeHeap` is on), at most an eighth of RAM, between 16 and 256 MiB, and
a quarter-size budget on low-RAM devices. On a typical 2 GB TV that is
256 MiB, about 27 s of an 80 Mbps 4K remux and the full 120 s below
about 18 Mbps. ExoPlayer's defaults were 50 s or about 138 MiB.

Playback starts after 1 s of buffer and resumes 5 s after a stall. Nothing
is kept behind the playhead. ExoPlayer would count that against the same
budget, so rewinding re-fetches instead, which takes about a second. Debug
builds show the live read-ahead and budget under the player controls and log
the plan as `buffer plan:` under the tag `rvah`.

The video surface is fitted to Media3's decoded display ratio, including
non-square pixels. Media3 text cues are drawn in a separate Compose overlay;
choosing an online subtitle explicitly enables text, marks that external track
as preferred, and resumes the same playback position.

## Emulator

`android/tv-emulator.sh` runs the app on an Android TV emulator (API 36,
x86_64, 1080p) for testing and debugging without a TV:

```sh
export ANDROID_HOME=~/Android/Sdk          # needs cmdline-tools/latest
android/tv-emulator.sh setup               # once: emulator, TV image, AVD "rvah-tv"
android/tv-emulator.sh run                 # boot, build + install, push add-ons, launch
android/tv-emulator.sh logs                # the app's log; Rust core lines are tagged rvah
android/tv-emulator.sh key down down ok    # drive it like a remote
android/tv-emulator.sh screenshot shot.png
```

`run` builds only the x86_64 core and installs the debug APK, trusting
`keys/verify.pub` if it exists. `addons` pushes every signed `*.rvah` in the
repository root; install them from the app's Install screen as on a TV. In
the emulator window, the arrow keys and Enter work as the D-pad.

The emulator needs hardware virtualization: `/dev/kvm` must exist and be
writable by you. On AMD, enable "SVM Mode" in the firmware settings (Intel:
VT-x), reboot, and add yourself to the `kvm` group if needed
(`sudo usermod -aG kvm "$USER"`). Without it, `start` stops with that advice.

Debug builds send the Rust core's debug lines (each add-on call with its
outcome and duration, credential refreshes) to logcat under the tag `rvah`,
on a physical TV as well: `adb logcat -s rvah`.

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

Browse and search grids include a "More" tile while the add-on reports another
page. Loaded pages survive a trip to the player or into a subdirectory, and
Back refocuses the poster that was opened. The paging rules are in `Listing.kt`
(JVM-tested in `ListingTest`).
