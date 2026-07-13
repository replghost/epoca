#!/usr/bin/env bash
# postpackage hook: stably sign the freshly-packaged Epoca.app, then refresh
# the .dmg so a drag-install carries the signed app.
#
# `mach package` (via `surfer package`) produces an ad-hoc, linker-signed app
# and a .dmg built from it. Ad-hoc signatures have no stable identity, so the
# macOS keychain ACL that guards the host wallet ("Zen Encrypted Storage")
# re-prompts on every rebuild and blocks NeedsAccountGet — see scripts/sign-dev.sh
# for the full story. This re-signs the packaged app with the stable dev
# identity and rebuilds the dmg around it so what you install is already stable.
#
# Runs automatically after `npm run package`. Safe to run standalone too.
# Set EPOCA_SIGN_ID to pick a specific identity (see sign-dev.sh).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIST=$(ls -d "$ROOT"/engine/obj-*/dist 2>/dev/null | head -1)
if [ -z "${DIST:-}" ] || [ ! -d "$DIST" ]; then
  echo "postpackage: no engine/obj-*/dist directory — did 'surfer package' run? Skipping." >&2
  exit 0
fi

# The packaged app lives at <dist>/<binaryName>/<brandFullName>.app (binaryName
# is "zen"). Discover it rather than hardcoding the brand name.
APP=$(ls -d "$DIST"/zen/*.app 2>/dev/null | head -1)
if [ -z "${APP:-}" ] || [ ! -d "$APP" ]; then
  echo "postpackage: no packaged .app under $DIST/zen (non-macOS build?) — skipping." >&2
  exit 0
fi

# Never fail the package for a contributor/CI without a signing identity: skip
# with a note instead. The app stays ad-hoc (the pre-existing behavior).
# A real identity prints a numbered "  1) <40-hex> "name"" line; zero identities
# print only "0 valid identities found", so match the hash line specifically.
if [ -z "${EPOCA_SIGN_ID:-}" ] && \
   ! security find-identity -v -p codesigning 2>/dev/null | grep -qE '[0-9]+\) [0-9A-F]{40}'; then
  echo "postpackage: no codesigning identity — leaving ad-hoc signature." >&2
  echo "  (set up a dev cert to keep the wallet keychain stable across rebuilds;" >&2
  echo "   see scripts/sign-dev.sh)" >&2
  exit 0
fi

echo "postpackage: signing packaged app"
bash "$ROOT/scripts/sign-dev.sh" "$APP"

# Refresh the dmg from the signed app. Best-effort: if this fails, the signed
# app under dist/ is still usable (copy it to /Applications directly).
DMG=$(ls -t "$DIST"/*.dmg 2>/dev/null | head -1)
if [ -z "${DMG:-}" ]; then
  echo "postpackage: no .dmg to refresh (app is signed regardless)."
  exit 0
fi

echo "postpackage: rebuilding $(basename "$DMG") around the signed app"
STAGE=$(mktemp -d)
# ditto (not cp) preserves the code signature and bundle metadata intact.
ditto "$APP" "$STAGE/$(basename "$APP")"
ln -s /Applications "$STAGE/Applications"
VOL="$(basename "${APP%.app}")"
if hdiutil create -volname "$VOL" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null; then
  echo "postpackage: dmg refreshed"
else
  echo "postpackage: WARNING dmg rebuild failed — install the signed app from $APP directly" >&2
fi
rm -rf "$STAGE"
