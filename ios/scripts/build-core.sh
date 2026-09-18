#!/bin/sh
# Builds the shared Rust core as a static library for the platform Xcode is
# building. Run by the app target's "Build Rust core" phase; also runnable by
# hand: PLATFORM_NAME=iphonesimulator ios/scripts/build-core.sh
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"

case "${PLATFORM_NAME:-iphoneos}" in
  iphonesimulator) triple=aarch64-apple-ios-sim ;;
  *) triple=aarch64-apple-ios ;;
esac

# Homebrew's rustc has no iOS standard library; prefer rustup's toolchain.
PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"
if command -v rustup >/dev/null 2>&1; then
  PATH="$(dirname "$(rustup which cargo)"):$PATH"
  rustup target list --installed | grep -qx "$triple" || rustup target add "$triple"
fi
command -v cargo >/dev/null 2>&1 || { echo "error: cargo not found; install Rust with rustup" >&2; exit 1; }

# Xcode exports SDKROOT and friends for the iOS SDK. Build scripts and proc
# macros compile for the Mac, so give cargo a clean environment.
env -i \
  HOME="$HOME" PATH="$PATH" \
  IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-26.0}" \
  CARGO_TERM_COLOR=never \
  cargo rustc \
    --manifest-path "$repo/src-tauri/Cargo.toml" \
    --lib --crate-type staticlib --release --locked \
    --no-default-features --features mobile \
    --target "$triple"

lib="$repo/src-tauri/target/$triple/release/libfileporter_lib.a"
test -f "$lib" || { echo "error: $lib was not produced" >&2; exit 1; }
