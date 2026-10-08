#!/bin/sh
# An Android TV emulator for testing and debugging RVAH without a TV.
#
#   android/tv-emulator.sh setup        install the emulator + TV image, create the AVD
#   android/tv-emulator.sh start        boot it (HEADLESS=1: no window) and wait for boot
#   android/tv-emulator.sh install      build the x86_64 core + debug APK, install it
#   android/tv-emulator.sh addons       push the repo root's *.rvah (+ .sig) to the app
#   android/tv-emulator.sh launch       start the app
#   android/tv-emulator.sh logs         follow the app's log (Rust core lines: tag "rvah")
#   android/tv-emulator.sh key K...     press remote keys: up down left right ok back
#                                       home play rewind forward
#   android/tv-emulator.sh screenshot F save the screen as a PNG
#   android/tv-emulator.sh run          start + install + addons + launch
#
# Needs ANDROID_HOME (with cmdline-tools/latest), and hardware virtualization:
# the x86_64 image does not boot without /dev/kvm.
set -eu

AVD=${AVD:-rvah-tv}
IMAGE=${IMAGE:-"system-images;android-36;android-tv;x86_64"}
APP=com.rvah.tv.debug
ADDONS_DIR=/sdcard/Android/data/$APP/files/addons

android_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_dir=$(dirname "$android_dir")

: "${ANDROID_HOME:?set ANDROID_HOME to your Android SDK}"
sdkmanager="$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager"
avdmanager="$ANDROID_HOME/cmdline-tools/latest/bin/avdmanager"
emulator="$ANDROID_HOME/emulator/emulator"
adb="$ANDROID_HOME/platform-tools/adb"

require_kvm() {
  if [ ! -e /dev/kvm ]; then
    echo "No /dev/kvm: hardware virtualization is off or unsupported." >&2
    echo "Enable it in the firmware (AMD: SVM Mode; Intel: VT-x), reboot," >&2
    echo "and check that 'lsmod | grep kvm_' lists kvm_amd or kvm_intel." >&2
    exit 1
  fi
  if [ ! -r /dev/kvm ] || [ ! -w /dev/kvm ]; then
    echo "/dev/kvm is not accessible: sudo usermod -aG kvm \"\$USER\", then log in again." >&2
    exit 1
  fi
}

setup() {
  yes | "$sdkmanager" --licenses >/dev/null
  "$sdkmanager" "platform-tools" "emulator" "$IMAGE"
  if "$avdmanager" list avd -c | grep -qx "$AVD"; then
    echo "AVD $AVD already exists."
  else
    echo no | "$avdmanager" create avd -n "$AVD" -k "$IMAGE" -d tv_1080p
  fi
}

start() {
  require_kvm
  if "$adb" get-state >/dev/null 2>&1; then
    echo "A device is already connected."
  else
    window=""
    [ "${HEADLESS:-0}" = 1 ] && window="-no-window"
    # shellcheck disable=SC2086 # $window is empty or one flag
    nohup "$emulator" -avd "$AVD" -no-snapshot-save $window \
      >"${TMPDIR:-/tmp}/rvah-tv-emulator.log" 2>&1 &
    echo "Booting $AVD (log: ${TMPDIR:-/tmp}/rvah-tv-emulator.log)…"
  fi
  "$adb" wait-for-device
  until [ "$("$adb" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ]; do
    sleep 2
  done
  echo "Booted."
}

install() {
  cd "$repo_dir"
  # The emulator is x86_64; build only that ABI of the core.
  rustup target add x86_64-linux-android
  cargo ndk -t x86_64 -o android/app/src/main/jniLibs build --release -p rvah-android --lib
  cargo build -p rvah-android --lib
  cargo run -p rvah-android --bin uniffi-bindgen -- generate \
    --library target/debug/librvah_android.so --language kotlin \
    --out-dir android/app/src/main/generated
  keys=""
  [ -f keys/verify.pub ] && keys="-Prvah.trustedKeys=$(cat keys/verify.pub)"
  cd "$android_dir"
  # shellcheck disable=SC2086 # $keys is empty or one property
  ./gradlew :app:installDebug $keys
}

addons() {
  found=0
  for pkg in "$repo_dir"/*.rvah; do
    [ -f "$pkg" ] && [ -f "$pkg.sig" ] || continue
    found=1
    "$adb" shell mkdir -p "$ADDONS_DIR"
    "$adb" push "$pkg" "$pkg.sig" "$ADDONS_DIR/"
  done
  [ "$found" = 1 ] || echo "No signed packages (*.rvah with .sig) in $repo_dir." >&2
}

launch() {
  "$adb" shell am start -n "$APP/com.rvah.tv.MainActivity"
}

logs() {
  pid=$("$adb" shell pidof -s "$APP" | tr -d '\r')
  if [ -z "$pid" ]; then
    echo "$APP is not running; showing the rvah tag only." >&2
    exec "$adb" logcat -s rvah
  fi
  exec "$adb" logcat --pid="$pid"
}

key() {
  for name in "$@"; do
    case "$name" in
      up) code=DPAD_UP ;;
      down) code=DPAD_DOWN ;;
      left) code=DPAD_LEFT ;;
      right) code=DPAD_RIGHT ;;
      ok) code=DPAD_CENTER ;;
      back) code=BACK ;;
      home) code=HOME ;;
      play) code=MEDIA_PLAY_PAUSE ;;
      rewind) code=MEDIA_REWIND ;;
      forward) code=MEDIA_FAST_FORWARD ;;
      *) echo "unknown key: $name" >&2; exit 1 ;;
    esac
    "$adb" shell input keyevent "KEYCODE_$code"
  done
}

screenshot() {
  "$adb" exec-out screencap -p >"${1:?usage: screenshot FILE.png}"
  echo "Saved $1"
}

command=${1:-}
[ $# -gt 0 ] && shift
case "$command" in
  setup) setup ;;
  start) start ;;
  install) install ;;
  addons) addons ;;
  launch) launch ;;
  logs) logs ;;
  key) key "$@" ;;
  screenshot) screenshot "$@" ;;
  run) start; install; addons; launch ;;
  *) sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
