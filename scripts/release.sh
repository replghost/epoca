#!/usr/bin/env bash
set -euo pipefail

# ─── Epoca Release Script ───────────────────────────────────────────
#
# Usage:
#   ./scripts/release.sh 0.1.0           # full release
#   ./scripts/release.sh 0.1.0 --dry-run # show what would happen
#
# What it does:
#   1. Bumps version in Cargo.toml
#   2. Moves [Unreleased] changelog entries to a versioned section
#   3. Builds the release binary + .app bundle
#   4. Creates a tarball with the icon
#   5. Commits, tags, pushes
#   6. Creates a draft GitHub Release with the tarball
#   7. Prints the Homebrew cask update instructions
#
# Prerequisites:
#   - gh CLI authenticated (gh auth login)
#   - Clean working tree (uncommitted changes will be rejected)
# ─────────────────────────────────────────────────────────────────────

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

if [ $# -lt 1 ]; then
    echo "Usage: $0 <version> [--dry-run]"
    echo "Example: $0 0.1.0"
    exit 1
fi

VERSION="$1"
DRY_RUN="${2:-}"
ARCH="$(uname -m)"
TAG="v${VERSION}"
TARBALL="Epoca-${VERSION}-macos-${ARCH}.tar.gz"

echo "==> Releasing Epoca ${VERSION} (${ARCH})"
echo ""

# ── Preflight checks ────────────────────────────────────────────────

if ! gh auth status &>/dev/null; then
    echo "ERROR: gh CLI not authenticated. Run: gh auth login"
    exit 1
fi

if [ -n "$(git status --porcelain)" ]; then
    echo "ERROR: Working tree is dirty. Commit or stash changes first."
    git status --short
    exit 1
fi

if git tag -l "$TAG" | grep -q "$TAG"; then
    echo "ERROR: Tag $TAG already exists."
    exit 1
fi

# ── Step 1: Bump version ────────────────────────────────────────────

echo "==> Bumping version to ${VERSION}..."

CURRENT=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
if [ "$CURRENT" = "$VERSION" ]; then
    echo "    Already at ${VERSION}"
else
    sed -i '' "s/^version = \"${CURRENT}\"/version = \"${VERSION}\"/" Cargo.toml
    # Also bump dev-info.plist
    sed -i '' "s/<string>${CURRENT}<\/string>/<string>${VERSION}<\/string>/" dev-info.plist
    echo "    ${CURRENT} → ${VERSION}"
fi

# ── Step 2: Finalize changelog ──────────────────────────────────────

echo "==> Finalizing changelog..."

TODAY=$(date +%Y-%m-%d)
if grep -q '^\#\# \[Unreleased\]' CHANGELOG.md; then
    sed -i '' "s/^## \[Unreleased\] — ongoing/## [Unreleased] — ongoing\n\n---\n\n## [${VERSION}] — ${TODAY}/" CHANGELOG.md
    echo "    Created section [${VERSION}] — ${TODAY}"
else
    echo "    WARNING: No [Unreleased] section found"
fi

# ── Step 3: Extract release notes ───────────────────────────────────

echo "==> Extracting release notes..."

# Pull everything between ## [VERSION] and the next ## [
NOTES=$(awk "/^## \[${VERSION}\]/{found=1; next} /^## \[/{if(found) exit} found{print}" CHANGELOG.md)
NOTES_FILE=$(mktemp)
echo "$NOTES" > "$NOTES_FILE"
NOTES_LINES=$(echo "$NOTES" | wc -l | tr -d ' ')
echo "    ${NOTES_LINES} lines of release notes"

# ── Dry run stops here ──────────────────────────────────────────────

if [ "$DRY_RUN" = "--dry-run" ]; then
    echo ""
    echo "==> DRY RUN — would do the following:"
    echo "    - Commit version bump + changelog"
    echo "    - Tag: ${TAG}"
    echo "    - Build: cargo build --release -p epoca"
    echo "    - Package: dist/${TARBALL}"
    echo "    - Create draft GitHub Release"
    echo ""
    echo "Release notes preview:"
    echo "─────────────────────"
    head -20 "$NOTES_FILE"
    echo "─────────────────────"
    # Revert changes
    git checkout -- Cargo.toml dev-info.plist CHANGELOG.md 2>/dev/null || true
    rm -f "$NOTES_FILE"
    exit 0
fi

# ── Step 4: Build ────────────────────────────────────────────────────

echo "==> Building release binary..."
cargo build --release -p epoca

APP_SRC="target/release/Epoca.app"
if [ ! -d "$APP_SRC" ]; then
    echo "ERROR: ${APP_SRC} not found"
    exit 1
fi

# ── Step 5: Package ─────────────────────────────────────────────────

echo "==> Packaging .app bundle..."
mkdir -p dist
STAGE=$(mktemp -d)
cp -R "$APP_SRC" "$STAGE/Epoca.app"

# Add icon
if [ -f "resources/Epoca.icns" ]; then
    mkdir -p "$STAGE/Epoca.app/Contents/Resources"
    cp "resources/Epoca.icns" "$STAGE/Epoca.app/Contents/Resources/Epoca.icns"
    # Ensure Info.plist references the icon
    /usr/libexec/PlistBuddy -c "Add :CFBundleIconFile string Epoca" \
        "$STAGE/Epoca.app/Contents/Info.plist" 2>/dev/null || \
    /usr/libexec/PlistBuddy -c "Set :CFBundleIconFile Epoca" \
        "$STAGE/Epoca.app/Contents/Info.plist"
fi

strip "$STAGE/Epoca.app/Contents/MacOS/epoca" 2>/dev/null || true
tar -czf "dist/${TARBALL}" -C "$STAGE" Epoca.app
rm -rf "$STAGE"

SHA=$(shasum -a 256 "dist/${TARBALL}" | cut -d' ' -f1)
SIZE=$(du -h "dist/${TARBALL}" | cut -f1)
echo "    dist/${TARBALL} (${SIZE}, sha256: ${SHA})"

# ── Step 6: Commit, tag, push ───────────────────────────────────────

echo "==> Committing and tagging..."
git add Cargo.toml Cargo.lock dev-info.plist CHANGELOG.md
git commit -m "$(cat <<EOF
release: v${VERSION}
EOF
)"
git tag -a "$TAG" -m "Epoca v${VERSION}"
git push origin main
git push origin "$TAG"

# ── Step 7: Create GitHub Release ────────────────────────────────────

echo "==> Creating draft GitHub Release..."
gh release create "$TAG" \
    "dist/${TARBALL}" \
    --title "Epoca v${VERSION}" \
    --notes-file "$NOTES_FILE" \
    --draft

rm -f "$NOTES_FILE"

# ── Done ─────────────────────────────────────────────────────────────

echo ""
echo "============================================"
echo "  Epoca v${VERSION} release created (DRAFT)"
echo "============================================"
echo ""
echo "Next steps:"
echo "  1. Review the draft at: https://github.com/replghost/epoca/releases"
echo "  2. Publish when ready"
echo "  3. Update the Homebrew cask:"
echo ""
echo "     In homebrew-epoca/Casks/epoca.rb, set:"
echo "       version \"${VERSION}\""
echo "       sha256 \"${SHA}\"  # ${ARCH}"
echo ""
