# Epoca — distribution

How Epoca is built, signed, and shipped. Epoca inherits Zen's release pipeline
(`.github/workflows/*-release-build.yml`), so most machinery already exists —
this doc records the epoca-specific decisions and the handoff for publishing
from **paritytech**.

## Decisions (current)

- **Publish from:** `paritytech/epoca` (uses Parity's Apple Developer ID + CI
  secrets). `replghost/epoca` remains the working fork.
- **First targets:** macOS + Linux. Windows deferred (needs an Authenticode
  cert Parity may lack).
- **Auto-update:** **off for v1.** Distribute via **GitHub Releases**
  (manual download). No update host to stand up yet. Auto-update can be added
  later without re-architecting (see "Auto-update, later").

## Two signings — keep them separate

| Signing | Purpose | Source | Status |
| --- | --- | --- | --- |
| **OS code signing** | So users can *run* the app (Gatekeeper / SmartScreen) | macOS: Apple Developer ID + notarization; Windows: Authenticode | **macOS: net-new, needs Parity's Apple account** |
| **MAR update signing** | Tamper-proof auto-updates | Self-generated RSA keypair (`scripts/mar_sign.sh`) | Self-serve; only needed once auto-update is on |

The inherited macOS workflow does **not** codesign or notarize today (it runs
`mach package` → `npm run package` → `.dmg` → upload). Notarization is the one
net-new piece for a double-click-runnable mac build.

## Repo move (manual)

Two ways to get the code under `paritytech`:

**A. GitHub transfer** (cleanest — preserves issues/PRs/history; old URL
redirects). Repo Settings → *Transfer ownership* → `paritytech`. Requires admin
on the repo + rights to create repos in the org. Then locally:
```
git remote set-url origin https://github.com/paritytech/epoca.git
```

**B. New repo + push** (keeps `replghost/epoca` too — mirror model):
```
# create an EMPTY paritytech/epoca on GitHub (no README/license)
git remote rename origin replghost              # keep the old remote
git remote add origin https://github.com/paritytech/epoca.git
git push -u origin --all
git push origin --tags
```

Remotes after the move (recommended):
- `origin` → `paritytech/epoca` (publish target)
- `replghost` → `replghost/epoca` (personal working fork, optional)
- `zen` → `zen-browser/desktop` (upstream, for Firefox/Zen sync — unchanged)

**CLA note:** paritytech repos run a CLA bot that blocks PRs whose commits carry
a `Co-Authored-By: Claude` trailer. Epoca commits already omit it — keep it that
way.

## CI secrets to configure in `paritytech/epoca`

Referenced by the existing release workflows:

- `DEPLOY_KEY` — release push token.
- `SURFER_CERT_PATCH_ISSUER`, `SURFER_CERT_PATCH_NAME` — Surfer build-cert patch.
- `ZEN_SIGNING_CERT_PEM_BASE64`, `ZEN_SIGNING_PRIVATE_KEY_PEM_BASE64`,
  `ZEN_MAR_SIGNING_PASSWORD` — MAR update signing. Only needed when auto-update
  is turned on. Generate with `bash scripts/mar_sign.sh` (self-signed keypair).
- Optional (features degrade if empty, build still works):
  `ZEN_SAFEBROWSING_API_KEY`, `ZEN_MOZILLA_API_KEY`,
  `ZEN_GOOGLE_LOCATION_SERVICE_API_KEY`.

**Net-new for notarized macOS** (Parity's Apple account) — plus codesign +
`xcrun notarytool` + `stapler` steps added to `macos-release-build.yml`:
- Developer ID Application cert (`.p12`) + its password.
- Notarization creds: App Store Connect API key (issuer id + key id + `.p8`) —
  or Apple ID + app-specific password + Team ID.

## Building a release

Workflows are brand-parameterized — invoke with the **`epoca`** brand:
```
npm run surfer -- ci --brand epoca --display-version <version>
./mach build        # full release build
npm run package     # -> dist/*.dmg (mac) / tarball (linux)
```
Locally, `npm run package` on the current objdir produces a standalone `.dmg`
(chrome jarred into `omni.ja`, so no dev symlinks — dot:// works with the
sandbox on, no `MOZ_DISABLE_CONTENT_SANDBOX` needed).

## Config that must change before any public build

- `surfer.json` `updateHostname` (currently `updates.zen-browser.app`) — a
  shipped build would phone **Zen's** update server. With auto-update off, point
  the updater at an inert epoca value / disable it.
- `surfer.json` `brands.epoca.github.repo` → `paritytech/epoca`.

## Auto-update, later

When ready: generate the MAR keypair (above), stand up a static host (GitHub
Pages / S3 / Cloudflare) serving `update.xml` + `.mar` files under a domain you
control (e.g. `updates.epoca.<tld>`), set `updateHostname` to it, and re-enable
the updater. MARs are verified against the embedded public key, so a plain
static host is safe.
