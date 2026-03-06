#!/usr/bin/env bash
# Re-sign the Epoca binary with a stable identifier before running.
#
# Used as a Cargo custom runner (.cargo/config.toml) so that every
# `cargo run` produces a binary macOS recognises as the same app across
# builds.  Without this, the linker embeds a random hash in the identifier
# (e.g. "epoca-725c8b98a71875df"), causing WKWebView's WebCrypto key to be
# treated as a new Keychain entry every build → errSecDuplicateItem (-25299)
# → "wants to access your keychain" dialog on every launch.
#
# Ad-hoc signing (no Apple Developer account required) is sufficient for
# local development.  The identifier must match what's in dev.entitlements.

set -euo pipefail

BINARY="$1"; shift

codesign \
  --force \
  --sign - \
  --identifier "com.replghost.epoca" \
  "$BINARY" 2>/dev/null

exec "$BINARY" "$@"
