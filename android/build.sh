#!/bin/bash
set -euo pipefail

# Build zhost-android for ARM Android targets using cargo-ndk.
# Requires: cargo-ndk (`cargo install cargo-ndk`), Android NDK.
#
# Usage:
#   ./android/build.sh          # Build debug APK
#   ./android/build.sh release  # Build release APK

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
JNILIBS_DIR="$SCRIPT_DIR/app/src/main/jniLibs"

BUILD_TYPE="${1:-debug}"

echo "==> Building zhost-android native libraries ($BUILD_TYPE)"

CARGO_NDK_ARGS="-t arm64-v8a -t armeabi-v7a -o $JNILIBS_DIR"

if [ "$BUILD_TYPE" = "release" ]; then
    cargo ndk $CARGO_NDK_ARGS build --release -p zhost-android --features android
else
    cargo ndk $CARGO_NDK_ARGS build -p zhost-android --features android
fi

echo "==> Building APK"

cd "$SCRIPT_DIR"

if [ "$BUILD_TYPE" = "release" ]; then
    ./gradlew assembleRelease
    echo "==> APK: $SCRIPT_DIR/app/build/outputs/apk/release/app-release.apk"
else
    ./gradlew assembleDebug
    echo "==> APK: $SCRIPT_DIR/app/build/outputs/apk/debug/app-debug.apk"
fi
