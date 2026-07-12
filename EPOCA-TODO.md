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

## 📦 Distribution (later)
- Package: `surfer package` → DMG (mac) / installer (win) / tar (linux).
- Signing: macOS Developer ID + notarize (Parity has certs), Windows Authenticode (needs a cert), MAR signing + an update host. Unsigned works only for you/technical testers.
- Consider moving repo to `paritytech/epoca` for signing certs + org/CI.

## 🧵 Follow-ups
- Re-vendor **official** `@useragent-kit/wasm@0.4.50` once useragent-kit publishes (blocked on their release infra: `release.yml` Swift-dist step fails + no `v0.4.50` tag). epoca currently ships a local 0.4.50 build.
- Open the **changeset-status CI guardrail** PR for useragent-kit (`pnpm exec changeset status` in the PR check).
- Send the drafted **message to useragent-kit maintainers** re: the Swift-dist release failure.

## 🛠 Workflow notes
- `dev` = epoca integration branch. Sync Zen via **reviewed local merges**, not the fork "Sync" button (diverged fork → Discard trap).
- Keep changes in `src/zen/epoca/**` + **additive** brand to minimize sync conflicts. `surfer.json` is the only real recurring conflict (Zen bumps its `version` each Firefox sync — keep our brand lines, take their version).
- Build: Node 22 (`/opt/homebrew/opt/node@22/bin`); `npm run surfer -- ci --brand epoca`; run via `cd engine && python3 ./mach run`. SSH/port-22 is blocked here → push over HTTPS with `gh auth setup-git`.
- Branding assets under `engine/browser/branding/*` are generated + gitignored (no asset diffs to merge).
