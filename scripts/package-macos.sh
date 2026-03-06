#!/usr/bin/env bash
set -euo pipefail

# Package Epoca as a macOS .app bundle in a tarball.
# Usage: ./scripts/package-macos.sh [--arch arm64|x86_64]
#
# Outputs: dist/Epoca-<version>-macos-<arch>.tar.gz

ARCH="${2:-$(uname -m)}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION=$(grep '^version' "$REPO_ROOT/Cargo.toml" | head -1 | sed 's/.*"\(.*\)"/\1/')

echo "==> Building epoca (release, $ARCH)..."
cd "$REPO_ROOT"

if [ "$ARCH" = "x86_64" ] && [ "$(uname -m)" = "arm64" ]; then
    # Cross-compile for Intel on Apple Silicon
    cargo build --release -p epoca --target x86_64-apple-darwin
    APP_SRC="target/x86_64-apple-darwin/release/Epoca.app"
else
    cargo build --release -p epoca
    APP_SRC="target/release/Epoca.app"
fi

if [ ! -d "$APP_SRC" ]; then
    echo "ERROR: $APP_SRC not found. GPUI should generate the .app bundle automatically."
    exit 1
fi

echo "==> Packaging..."
DIST="$REPO_ROOT/dist"
mkdir -p "$DIST"
TARBALL="Epoca-${VERSION}-macos-${ARCH}.tar.gz"

# Create a clean staging area
STAGE=$(mktemp -d)
cp -R "$APP_SRC" "$STAGE/Epoca.app"

# Add app icon
ICON="$REPO_ROOT/resources/Epoca.icns"
if [ -f "$ICON" ]; then
    mkdir -p "$STAGE/Epoca.app/Contents/Resources"
    cp "$ICON" "$STAGE/Epoca.app/Contents/Resources/Epoca.icns"
    echo "    Icon added"
fi

# Strip debug symbols to reduce size
strip "$STAGE/Epoca.app/Contents/MacOS/epoca" 2>/dev/null || true

tar -czf "$DIST/$TARBALL" -C "$STAGE" Epoca.app
rm -rf "$STAGE"

SIZE=$(du -h "$DIST/$TARBALL" | cut -f1)
SHA=$(shasum -a 256 "$DIST/$TARBALL" | cut -d' ' -f1)

echo ""
echo "==> Done!"
echo "    File:    dist/$TARBALL"
echo "    Size:    $SIZE"
echo "    SHA-256: $SHA"
echo ""
echo "Upload to GitHub Releases, then update the Homebrew cask with:"
echo "    sha256 \"$SHA\""
