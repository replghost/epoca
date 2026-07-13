# Epoca — working TODO

_Living checklist. Kept in the repo so it's visible/durable; not necessarily committed._

## ✅ Done
- **t3ams auto-sign, end-to-end** — RemotePermission wire fix, `NeedsPermissionPrompt` grant+replay binding, statement proof (`payload.subarray(4)`), submit-result parsing. epoca commit `c1eb24d77` (pushed to `origin/dev`).
- **Cross-client user discovery** — people-chain now proxied via the vendored useragent-kit environment bundle (`paseo-next-v2.json`); `UsernameOwnerOf` resolves (william.99 found).
- **useragent-kit PR #1552 merged** (RemotePermission wire + `storeRemotePermissionDecision`), plus **#1553** (changeset fix). Bot release PRs #1549/#1554 merged.

## 🔜 Active (this batch → one clean rebuild)
- **[B] Context-menu / l10n bug** — REAL bug (ruled out dev-run, clobber build, fresh profile). L10nRegistry returns `null` for chrome messages → menu labels empty → thin-bar menu. Find root cause.
- **[A] Rebrand → Epoca (ADDITIVE)** — add `brands.epoca` in `surfer.json` + `configs/branding/epoca` (do NOT overwrite Zen's `release`); set app-name/basename to epoca/Epoca; build `--brand epoca`. Additive keeps Zen sync clean.
- **[C] Sync Zen `dev`** — clean merge (16 commits; only `src/zen/moz.build` overlaps, auto-merges). Do as a reviewed local merge (NOT the GitHub button). May fix [B] if it's an upstream bug.
- **[Accounts + pink dot]** — remove the urlbar pink-dot (`EpocaIdentityPanel`); design the account/identity UX (Arc-style profile card + a settings section).

## 📦 Distribution
- Plan + handoff: see `DISTRIBUTION.md`. Decisions: publish from `paritytech/epoca`, target macOS+Linux first, auto-update OFF for v1 (ship via GitHub Releases).
- `npm run package` → standalone `.dmg` verified working (omni.ja, dot:// with sandbox on, l10n baked in).
- Remaining gate: **macOS notarization is net-new** (not in the inherited workflow) — needs Parity's Apple Developer ID + added codesign/notarize/staple steps. Until then the `.dmg` is unsigned (recipients right-click→Open).

## 🧵 Follow-ups
- Send the drafted **message to useragent-kit maintainers** re: the Swift-dist release failure.

## ✅ Recently done
- Visible-text rebrand → Epoca (`-brand-product-name` via per-brand `brandingGenericName`; chrome strings de-Zen'd). Internals stay `zen`.
- Re-vendored official `@useragent-kit/wasm@0.4.50` (byte-for-byte, bridge+ss58 12/12).
- useragent-kit changeset guardrail merged (PR #1557 — name-check in `verify-changeset.mjs`).

## 🛠 Workflow notes
- `dev` = epoca integration branch. Sync Zen via **reviewed local merges**, not the fork "Sync" button (diverged fork → Discard trap).
- Keep changes in `src/zen/epoca/**` + **additive** brand to minimize sync conflicts. `surfer.json` is the only real recurring conflict (Zen bumps its `version` each Firefox sync — keep our brand lines, take their version).
- Build: Node 22 (`/opt/homebrew/opt/node@22/bin`); `npm run surfer -- ci --brand epoca`; run via `cd engine && python3 ./mach run`. SSH/port-22 is blocked here → push over HTTPS with `gh auth setup-git`.
- Branding assets under `engine/browser/branding/*` are generated + gitignored (no asset diffs to merge).
