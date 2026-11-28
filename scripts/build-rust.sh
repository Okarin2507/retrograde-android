#!/usr/bin/env bash
# Builds the Rust FFI crate for Android and generates the Kotlin UniFFI bindings.
#
#   1. cargo-ndk  -> android/app/src/main/jniLibs/<abi>/librustygrade_ffi.so
#   2. host build + uniffi-bindgen (library mode) -> android/app/src/main/java/uniffi/
#
# Env: ANDROID_NDK_HOME (auto-detected under $ANDROID_HOME / ~/Android/Sdk), ABIS, PROFILE=release|debug
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_DIR="$ROOT/rust"
APP_MAIN="$ROOT/android/app/src/main"
JNI_DIR="$APP_MAIN/jniLibs"
BINDINGS_DIR="$APP_MAIN/java"
MIN_API=28
PROFILE="${PROFILE:-release}"
ABIS="${ABIS:-arm64-v8a x86_64}"

if [[ -z "${ANDROID_NDK_HOME:-}" ]]; then
  SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"
  if [[ -d "$SDK/ndk" ]]; then
    ANDROID_NDK_HOME="$(find "$SDK/ndk" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -n1)"
  fi
fi
if [[ -z "${ANDROID_NDK_HOME:-}" || ! -d "$ANDROID_NDK_HOME" ]]; then
  echo "error: Android NDK not found. Set ANDROID_NDK_HOME or install via sdkmanager." >&2
  exit 1
fi
export ANDROID_NDK_HOME

case "$PROFILE" in
  release) CARGO_FLAGS=(--release) ;;
  debug)   CARGO_FLAGS=() ;;
  *) echo "error: PROFILE must be 'release' or 'debug' (got '$PROFILE')" >&2; exit 1 ;;
esac
# A relative CARGO_TARGET_DIR means "relative to where the user ran this script". Pin it to an absolute path
# now, before any `cd "$RUST_DIR"`, so cargo and the paths used below agree on the same directory.
if [[ -n "${CARGO_TARGET_DIR:-}" && "$CARGO_TARGET_DIR" != /* ]]; then
  CARGO_TARGET_DIR="$PWD/$CARGO_TARGET_DIR"
fi
if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
  export CARGO_TARGET_DIR
fi
TARGET_DIR="${CARGO_TARGET_DIR:-$RUST_DIR/target}"

NDK_TARGETS=()
for abi in $ABIS; do NDK_TARGETS+=(-t "$abi"); done

echo "==> NDK: $ANDROID_NDK_HOME"
echo "==> cargo ndk ($ABIS, API $MIN_API, $PROFILE)"
# Wipe old libs so a partial-ABI rebuild can never leave a stale .so with a mismatched UniFFI checksum.
rm -rf "$JNI_DIR"
mkdir -p "$JNI_DIR"
(cd "$RUST_DIR" && cargo ndk "${NDK_TARGETS[@]}" -P "$MIN_API" -o "$JNI_DIR" \
  build -p rustygrade-ffi ${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"})

echo "==> host build for binding metadata"
(cd "$RUST_DIR" && cargo build -p rustygrade-ffi -p uniffi-bindgen)

case "$(uname -s)" in
  Darwin) HOST_LIB="librustygrade_ffi.dylib" ;;
  *)      HOST_LIB="librustygrade_ffi.so" ;;
esac

echo "==> uniffi-bindgen -> $BINDINGS_DIR"
rm -rf "$BINDINGS_DIR/uniffi"
mkdir -p "$BINDINGS_DIR"
# uniffi-bindgen shells out to `cargo metadata`, so it must run inside the workspace.
(cd "$RUST_DIR" && "$TARGET_DIR/debug/uniffi-bindgen" generate \
  --library "$TARGET_DIR/debug/$HOST_LIB" --language kotlin --no-format --out-dir "$BINDINGS_DIR")

echo "==> done"
