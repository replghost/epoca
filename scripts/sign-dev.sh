#!/usr/bin/env bash
# Sign a built Epoca.app with a STABLE local code-signing identity.
#
# Why: dev builds are ad-hoc signed by the linker (Signature=adhoc, no
# TeamIdentifier). A macOS keychain ACL binds to an app's *designated
# requirement*; for an ad-hoc signature that is the exact cdhash, which
# changes on every rebuild. So after each rebuild, OSKeyStore.decrypt (used to
# unlock the host wallet) can no longer read "Zen Encrypted Storage" without a
# fresh keychain prompt — which blocks NeedsAccountGet and makes account-using
# dot:// products (e.g. t3ams-spa) boot then blank.
#
# Signing with a real identity (Apple Development is fine — hardened runtime is
# NOT used, so no entitlements are required and JIT keeps working) makes the
# designated requirement identity-based (Team + identifier "zen"), so it stays
# constant across rebuilds. Approve the keychain prompt once with "Always
# Allow" and it sticks for every future build signed with the same cert.
#
# Usage:
#   scripts/sign-dev.sh [/path/to/Epoca.app]      # default: /Applications/Epoca.app
#   EPOCA_SIGN_ID="Apple Development: ..." scripts/sign-dev.sh
#
# Pick the identity automatically if EPOCA_SIGN_ID is unset (first valid
# codesigning identity). Set EPOCA_SIGN_ID explicitly if you have more than one.
set -euo pipefail

APP="${1:-/Applications/Epoca.app}"
if [ ! -d "$APP" ]; then
  echo "error: app bundle not found: $APP" >&2
  exit 1
fi

ID="${EPOCA_SIGN_ID:-}"
if [ -z "$ID" ]; then
  ID=$(security find-identity -v -p codesigning | awk -F'"' 'NR==1{print $2}')
fi
if [ -z "$ID" ]; then
  echo "error: no codesigning identity found (security find-identity -v -p codesigning)" >&2
  exit 1
fi
echo "Signing $APP"
echo "  identity: $ID"

# Refuse to sign a running bundle: codesign rewrites the on-disk Mach-O
# signatures, which can crash the live process.
if pgrep -f "$APP/Contents/MacOS/" >/dev/null 2>&1; then
  echo "error: Epoca appears to be running — quit it first, then re-run." >&2
  exit 1
fi

# Strip quarantine/Finder xattrs first — codesign rejects bundles carrying
# "resource fork, Finder information, or similar detritus".
xattr -cr "$APP" 2>/dev/null || true

# Sign inside-out. --deep covers nested helper apps and frameworks; no hardened
# runtime (dev), no entitlements (matches the current ad-hoc build).
codesign --force --deep --sign "$ID" "$APP"

echo "=== verify ==="
codesign -dvv "$APP" 2>&1 | grep -iE "Authority|TeamIdentifier|Signature|flags"
codesign --verify --deep --strict --verbose=2 "$APP" && echo "signature OK"
