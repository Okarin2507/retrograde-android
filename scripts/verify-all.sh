#!/usr/bin/env bash
# End-to-end verification suite for rustygrade-android:
#   1. Rust clippy + unit/integration tests
#   2. Android Rust cross-build (.so + UniFFI) + symbol export check
#   3. Android lintDebug, assembleDebug
#   4. Release ABI split APKs + AAB (R8 + shrinkResources)
#   5. R8/dexdump, ABI isolation & launcher icon resource check
#   6. Ephemeral release keystore signing + apksigner verification
#      (the unsigned release output dir is backed up first and always restored on exit)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
BUILD_TOOLS="$(find "$ANDROID_HOME/build-tools" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -n1)"
if [[ -z "$BUILD_TOOLS" || ! -x "$BUILD_TOOLS/dexdump" || ! -x "$BUILD_TOOLS/apksigner" ]]; then
  echo "error: Android build-tools (dexdump/apksigner) not found under $ANDROID_HOME/build-tools" >&2
  exit 1
fi

REL_DIR="$ROOT/android/app/build/outputs/apk/release"
TMP_DIR="$(mktemp -d)"
REL_BACKUP="$TMP_DIR/unsigned_rel"
REL_BACKED_UP=0
cleanup() {
  # Step 6 re-signs the release APKs with a throwaway key. On ANY exit (success or failure) put the
  # untouched unsigned output (APKs + output-metadata.json) back so no smoke-signed APK is left behind.
  if [[ "$REL_BACKED_UP" -eq 1 && -d "$REL_BACKUP" ]]; then
    rm -rf "$REL_DIR"
    mkdir -p "$REL_DIR"
    cp -a "$REL_BACKUP/." "$REL_DIR/"
  fi
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

# Prefer the NDK's llvm-nm (portable across Linux/macOS hosts); fall back to the host nm.
find_nm() {
  local ndk="${ANDROID_NDK_HOME:-}" candidate
  if [[ -z "$ndk" || ! -d "$ndk" ]]; then
    ndk="$(find "$ANDROID_HOME/ndk" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | sort -V | tail -n1)"
  fi
  if [[ -n "$ndk" ]]; then
    candidate="$(find "$ndk/toolchains/llvm/prebuilt" -path '*/bin/llvm-nm' -type f 2>/dev/null | head -n1)"
    if [[ -n "$candidate" && -x "$candidate" ]]; then
      echo "$candidate"
      return 0
    fi
  fi
  command -v nm
}
NM_BIN="$(find_nm)"
[[ -n "$NM_BIN" ]] || { echo "error: neither NDK llvm-nm nor nm found" >&2; exit 1; }

echo "==> [1/6] Rust clippy + workspace tests"
(cd "$ROOT/rust" && cargo clippy --workspace --all-targets -- -D warnings)
RUST_TEST_LOG="$TMP_DIR/cargo-test.log"
(cd "$ROOT/rust" && timeout 250 cargo test --workspace) | tee "$RUST_TEST_LOG"

echo "==> [2/6] Build Rust Android .so & UniFFI bindings"
"$ROOT/scripts/build-rust.sh"
for abi in arm64-v8a x86_64; do
  so="$ROOT/android/app/src/main/jniLibs/$abi/librustygrade_ffi.so"
  [[ -f "$so" ]] || { echo "error: missing $so" >&2; exit 1; }
  count="$("$NM_BIN" -D "$so" | grep -c 'uniffi_' || true)"
  echo "    $abi exported uniffi_ symbols: $count"
  [[ "$count" -gt 0 ]] || { echo "error: $so lost uniffi_ dynamic exports" >&2; exit 1; }
done

echo "==> [3/6] Android lintDebug, assembleDebug"
(cd "$ROOT/android" && ./gradlew lintDebug assembleDebug)

echo "==> [4/6] Release ABI split APKs + AAB (R8 + shrinkResources)"
rm -rf "$ROOT/android/app/build/outputs/apk/release" "$ROOT/android/app/build/outputs/bundle/release"
(cd "$ROOT/android" && ./gradlew assembleRelease -PsplitApks=true)
(cd "$ROOT/android" && ./gradlew bundleRelease)

ARM_APK="$(find "$REL_DIR" -maxdepth 1 -name '*arm64-v8a*release*.apk' | head -n1)"
X86_APK="$(find "$REL_DIR" -maxdepth 1 -name '*x86_64*release*.apk' | head -n1)"
UNI_APK="$(find "$REL_DIR" -maxdepth 1 -name '*universal*release*.apk' | head -n1)"
AAB_FILE="$(find "$ROOT/android/app/build/outputs/bundle/release" -maxdepth 1 -name '*.aab' | head -n1)"

[[ -f "$ARM_APK" && -f "$X86_APK" && -f "$UNI_APK" && -f "$AAB_FILE" ]] || {
  echo "error: expected release split APKs or AAB missing" >&2
  exit 1
}

echo "==> [5/6] Inspect ABI isolation, launcher icon & R8 DEX retention"
assert_apk_has() {
  local apk="$1" pattern="$2" entries
  entries="$(zipinfo -1 "$apk")"
  grep -qE "$pattern" <<<"$entries" || { echo "error: $apk missing $pattern" >&2; exit 1; }
}
assert_apk_lacks() {
  local apk="$1" pattern="$2" entries
  entries="$(zipinfo -1 "$apk")"
  if grep -qE "$pattern" <<<"$entries"; then
    echo "error: $apk unexpectedly contains $pattern" >&2
    exit 1
  fi
}

assert_apk_has "$ARM_APK" '^lib/arm64-v8a/librustygrade_ffi\.so$'
assert_apk_has "$ARM_APK" '^lib/arm64-v8a/libjnidispatch\.so$'
assert_apk_lacks "$ARM_APK" '^lib/x86_64/'

assert_apk_has "$X86_APK" '^lib/x86_64/librustygrade_ffi\.so$'
assert_apk_has "$X86_APK" '^lib/x86_64/libjnidispatch\.so$'
assert_apk_lacks "$X86_APK" '^lib/arm64-v8a/'

assert_apk_has "$UNI_APK" '^lib/arm64-v8a/librustygrade_ffi\.so$'
assert_apk_has "$UNI_APK" '^lib/x86_64/librustygrade_ffi\.so$'
assert_apk_lacks "$UNI_APK" '^lib/(armeabi|armeabi-v7a|x86|mips|mips64)/'

RES_DUMP="$("$BUILD_TOOLS/aapt2" dump resources "$UNI_APK")"
# Adaptive launcher icon: aapt2 lists the density-independent XML under (anydpi).
grep -A12 'mipmap/ic_launcher' <<<"$RES_DUMP" | grep -q '(anydpi) (file) .*type=XML' || {
  echo "error: adaptive launcher icon (mipmap anydpi XML) missing from release APK" >&2
  exit 1
}

# Packaging hygiene: no coroutines debug blob / AndroidX version markers in any release APK.
for apk in "$ARM_APK" "$X86_APK" "$UNI_APK"; do
  assert_apk_lacks "$apk" '(^|/)DebugProbesKt\.bin$'
  assert_apk_lacks "$apk" '^META-INF/[^/]*\.version$'
done

# Multi-APK versionCodes must be distinct (universal < x86_64 < arm64-v8a) so stores can rank them.
python3 -I - "$REL_DIR/output-metadata.json" <<'PY'
import json, sys
elements = json.load(open(sys.argv[1]))["elements"]
codes = {e["outputFile"]: e["versionCode"] for e in elements}
if len(codes) != 3 or len(set(codes.values())) != 3:
    sys.exit(f"error: expected 3 APKs with distinct versionCodes, got {codes}")
print("    release versionCodes: " + ", ".join(f"{k}={v}" for k, v in sorted(codes.items())))
PY

DEX_DIR="$TMP_DIR/dex"
mkdir -p "$DEX_DIR"
unzip -q -o "$UNI_APK" 'classes*.dex' -d "$DEX_DIR"
DEX_CLASSES="$TMP_DIR/dex-classes.txt"
: > "$DEX_CLASSES"
for dex in "$DEX_DIR"/classes*.dex; do
  "$BUILD_TOOLS/dexdump" "$dex" | grep 'Class descriptor' >> "$DEX_CLASSES"
done
for cls in 'Luniffi/rustygrade_ffi/RustygradeEngine;' \
           'Luniffi/rustygrade_ffi/PlayerEngine;' \
           'Luniffi/rustygrade_ffi/UniffiLib;' \
           'Luniffi/rustygrade_ffi/UsbAudioDriver;' \
           'Luniffi/rustygrade_ffi/R128BatchScanner;' \
           'Lcom/sun/jna/Native;'; do
  grep -qF "$cls" "$DEX_CLASSES" || { echo "error: R8 stripped required class $cls" >&2; exit 1; }
done
echo "    R8 retained UniFFI/JNA bridge classes"

echo "==> [6/6] Ephemeral release keystore signing smoke test"
# Snapshot the pristine unsigned output (APKs + output-metadata.json). The EXIT trap restores it on
# success *and* failure, so the smoke-signed APKs never outlive this script.
cp -a "$REL_DIR" "$REL_BACKUP"
REL_BACKED_UP=1
KEYSTORE="$TMP_DIR/smoke.p12"
keytool -genkeypair -v -keystore "$KEYSTORE" -storetype PKCS12 -alias rustygrade \
  -keyalg RSA -keysize 2048 -validity 1 \
  -storepass smoke1234 -keypass smoke1234 -dname "CN=RustygradeTest" >/dev/null 2>&1

(
  cd "$ROOT/android"
  RUSTYGRADE_KEYSTORE_PATH="$KEYSTORE" \
  RUSTYGRADE_KEYSTORE_PASSWORD=smoke1234 \
  RUSTYGRADE_KEY_ALIAS=rustygrade \
  RUSTYGRADE_KEY_PASSWORD=smoke1234 \
  ./gradlew assembleRelease -PsplitApks=true
)
for abi in arm64-v8a x86_64 universal; do
  apk="$REL_DIR/rustygrade-$abi-release.apk"
  [[ -f "$apk" ]] || { echo "error: signed $abi release APK missing ($apk)" >&2; exit 1; }
  "$BUILD_TOOLS/apksigner" verify "$apk" >/dev/null
  echo "    apksigner verified: $(basename "$apk")"
done
SIGNED_COUNT="$(find "$REL_DIR" -maxdepth 1 -name '*-release.apk' | wc -l)"
[[ "$SIGNED_COUNT" -eq 3 ]] || { echo "error: expected 3 signed release APKs, found $SIGNED_COUNT" >&2; exit 1; }

# Restore now (the trap would too) so the summary below lists the real, unsigned artifacts.
rm -rf "$REL_DIR"
cp -a "$REL_BACKUP" "$REL_DIR"
REL_BACKED_UP=0
echo "    restored unsigned release output ($(find "$REL_DIR" -maxdepth 1 -name '*-unsigned.apk' | wc -l) APKs)"

echo ""
echo "=== VERIFICATION SUMMARY ==="
grep 'test result:' "$RUST_TEST_LOG"
ls -lh "$ROOT/android/app/build/outputs/apk/debug/"*.apk \
       "$REL_DIR"/*.apk \
       "$AAB_FILE"
