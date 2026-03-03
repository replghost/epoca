# Epoca Browser — Product Backlog & Strategy

## Product Vision

Epoca is a **programmable, privacy-first browser** built on open, auditable infrastructure.
Where Chrome is a distribution channel for Google's data business, Epoca is a workbench
for people who want to own their browsing — power users, developers, and privacy-conscious
professionals who are underserved by every mainstream browser.

---

## Positioning & Differentiation

### The Problem with the Status Quo
| Browser | Core tension |
|---------|-------------|
| Chrome | The browser IS the product — your attention is the business model |
| Safari | Locked to Apple ecosystem; no programmability |
| Firefox | Open but slow to innovate; extension model is a security liability |
| Arc | Beautiful UX but still Chromium-based (same telemetry risks) |
| Brave | Privacy-focused but ships a crypto ad network as the alternative |

### Epoca's Wedge
**"The browser that does what you tell it, not what Google tells it."**

Three interlocking advantages no other browser can replicate:

1. **Process-level privacy by default** — WKWebKit content blocking runs *before* the
   network stack, not in a JavaScript extension that can be removed by Manifest V4.
   Blocks ads, trackers, fingerprinting at the OS compositor level, zero JS overhead.

2. **Programmable tabs** — PolkaVM sandboxed guest apps + ZML declarative UI means
   users can write tiny, auditable tab-replacement apps. A custom Reddit reader, a
   stripped-down Gmail, a focused Notion view — all as first-class browser tabs with
   no extension marketplace to trust.

3. **Cross-platform with native rendering** — GPUI on macOS/Linux/Windows, wgpu on
   Android. One codebase, native performance, no Electron overhead.

### Target Niche (Year 1)
- Developers who already use Arc but want open-source / auditable underpinnings
- Privacy researchers and journalists who need verifiable content blocking
- Power users building personal automation (custom tab apps, local AI side panels)
- Teams who want a company browser with auditable policy enforcement via the capability broker

---

## P0 — Critical bugs / must-fix before sharing

- [ ] **Sidebar layout bug**: content not extending to left edge when sidebar
  collapses (`_inset_subscription` fix landed — needs validation)
- [ ] **Omnibox focus**: ensure omnibox input auto-focuses when opened
- [ ] **WelcomeTab startup**: app should open omnibox immediately on launch,
  not an empty grey screen
- [ ] **Crash on fast sidebar toggle**: rapid toggle can leave animation task
  in inconsistent state

---

## P1 — Privacy & Content Blocking (deeper than Brave)

**Architecture:** Six-layer pipeline in new `epoca-shield` crate.
See `docs/content-blocking.md` for the full design document.

**Why this beats Chrome uBlock Origin:**
WKContentRuleList rules run in the OS network process — isolated from page JS,
undetectable by pages. Chrome Manifest V3 extensions run in the renderer. Brave
uses the same WKContentRuleList on macOS but Epoca adds CNAME uncloaking and
video-overlay sweeping which Brave does not have.

### P0 — Infrastructure (must land first)
- [ ] Create `epoca-shield` crate with `ShieldConfig`, `CompiledRuleSet`, `ShieldManager`
- [ ] HTTP list fetcher with ETag caching to `~/.epoca/content-rules/lists/`
- [ ] ABP/uBlock filter parser → WKContentRuleList JSON compiler (45k-rule bucket splitting)
- [ ] Install compiled rule lists on `WebViewTab` construction via objc2 bridge
- [ ] Register `epocaShield` `WKScriptMessageHandler` for blocked-count telemetry
- [ ] `ShieldManager` GPUI global in `epoca-core/src/shield.rs`

### P1 — Core blocking
- [ ] EasyList + EasyPrivacy + AdGuard Base → `epoca-rules-*` lists
- [ ] Cosmetic compiler: `##` rules → CSS injection via `WKUserScript` (document_end)
- [ ] `window.open` block: `createWebViewWith` delegate denial + document_start override
- [ ] Shield icon: network-blocked + popup-blocked counts per tab
- [ ] Settings page: toggle lists, per-domain exceptions
- [ ] Background update loop (6-hour interval)
- [ ] uBlock Annoyances + Fanboy Annoyance lists (cookie banners, overlay ads)
- [ ] User TOML rule format: `~/.epoca/content-rules/user-rules.toml`

### P1 — Video site deep blocking (surpasses Brave)
- [ ] YouTube DASH ad segment URL pattern block + JS skip script
- [ ] Overlay sweeper: MutationObserver + periodic scan for `z-index>999, position:fixed`
  elements matching ad class/id heuristics → auto-remove
- [ ] Cookie consent auto-dismiss JS (document_end, reject-only)
- [ ] Cookie consent CDN script block (OneTrust, Cookiebot, CookiePro domains)
- [ ] Exit-intent popup suppression (`mouseleave`/`beforeunload` capture on video sites)

### P1 — Fingerprinting protection
- [ ] Canvas noise (seeded per origin per session — consistent within page, differs across origins)
- [ ] WebGL vendor/renderer normalization
- [ ] Audio oscillator noise
- [ ] Font enumeration reduction (allowlist ~9 common fonts)
- [ ] Screen size rounding (nearest 100px)
- [ ] `navigator.hardwareConcurrency` + `navigator.deviceMemory` normalization

### P2 — CNAME uncloaking and deep tracker isolation (no browser does this natively)
- [ ] Integrate `hickory-dns` for async CNAME chain resolution at Rust layer
- [ ] AdGuard CNAME tracker list integration
- [ ] Block on CNAME match via `decidePolicyForNavigationAction`
- [ ] First-party storage partitioning (localStorage namespace in iframe contexts)
- [ ] Request header sanitization: Referer trim, UA normalization, Accept-Language reduction

### P2 — Anti-detection
- [ ] Anti-anti-adblock: `googletag`/`adsbygoogle` stub injection
- [ ] `getComputedStyle` proxy to spoof ad element dimensions
- [ ] Twitch ad stream restore after segment block
- [ ] Learned CNAME map persistence across sessions

**Epoca vs Brave comparison:**

| Capability | Brave | Epoca (planned) |
|---|---|---|
| Network blocking | EasyList/EasyPrivacy | + AdGuard + video CDN lists |
| Cosmetic filtering | Yes | Yes + overlay sweeper |
| CNAME uncloaking | No | Yes (DNS layer) |
| Video ad skipping | Basic DASH | DASH + JS skip + overlay removal |
| Cookie consent | Manual | Auto-dismiss (reject-only) |
| Fingerprint protection | Canvas/WebGL/audio | + font limits + screen rounding |
| Anti-anti-adblock | No | Stub injection + style proxy |

---

## P1 — First-Class Browser Features

### Navigation & History
- [ ] Persist browsing history to SQLite (`~/.epoca/history.db`)
- [ ] Omnibox autocomplete from history + open tabs
- [ ] Back/forward swipe gestures (macOS trackpad)
- [ ] Reading list / bookmarks (local, no sync account required)

### Tab Management
- [ ] Tab groups / workspaces (like Arc's spaces, but stored as ZML files — shareable)
- [ ] Session restore on launch
- [ ] Duplicate tab
- [ ] Drag-to-reorder in sidebar
- [ ] Pin/unpin tab (UI wired but persistence not implemented)
- [ ] Tab search (filter sidebar by title/URL)
- [ ] Mute tab audio

### UI / UX
- [ ] Crash reporting: wire up Sentry DSN (SDK already integrated — set `SENTRY_DSN` env var to activate)
- [ ] Keyboard shortcut system (⌘T new tab → omnibox, ⌘W close tab, ⌘L focus URL)
- [ ] Per-tab favicon fetched and displayed (replace static IconName::Globe)
- [ ] Dark/light mode toggle (system follow already works via WKWebView theme)
- [ ] Page title propagated from WKWebView navigation delegate to sidebar tab entry
- [ ] Find-in-page (⌘F)
- [ ] Full-screen mode (hide sidebar, maximize content)

### Testing
- [ ] GPUI `#[gpui::test]` — headless unit/integration tests for workbench logic via `TestAppContext`; no display needed, runs in CI
- [ ] Appium Mac2Driver — E2E UI testing via macOS Accessibility API (Playwright-like for native Mac apps); covers sidebar, omnibox, tab lifecycle

---

## P2 — Programmable Workbench (Epoca's unique value)

### ZML / PolkaVM Guest Apps
- [ ] ZML hot-reload in dev mode (already partly working)
- [ ] ZML standard library: fetch(), localStorage, clipboard
- [ ] ZML layout: flex wrap, grid support
- [ ] ZML components: Table, Chart, DatePicker, Modal
- [ ] Guest app marketplace (local directory of `.zml` apps, no central server)
- [ ] PolkaVM guest: async fetch capability (currently stubbed)
- [ ] ZML ↔ WebView bridge: guest app can open a WebView pane in split view

### Split View / Panels
- [ ] Vertical split: two tabs side-by-side
- [ ] Side panel: ZML app alongside a WebView (e.g., AI chat + web)
- [ ] Picture-in-picture: floating mini-webview

### Local AI Integration
- [ ] Side panel: local LLM (llama.cpp via FFI) for page summarization
- [ ] Page-aware context: selected text → LLM prompt
- [ ] ZML app: `@llm` binding for AI-powered tab apps

---

## P2 — Platform

### Android
- [ ] Android renderer integration with real ZML apps
- [ ] Touch-optimized sidebar (bottom sheet instead of left panel)
- [ ] Android WebView bridge (WebKit → Android WebView or GeckoView)
- [ ] Play Store / F-Droid packaging

### Sync (opt-in, E2E encrypted)
- [ ] History, bookmarks, tab groups sync via user-owned key
- [ ] No central server required: sync over iCloud Drive / local network / custom S3
- [ ] Open sync protocol — any client can implement it

### Enterprise / Team Features
- [ ] Capability broker policies pushed from a config file (already architected)
- [ ] Network policy: block categories of sites per workspace
- [ ] Audit log: which apps accessed which capabilities

---

## P1 — Security & Sandboxing (QA/Architect review findings)

### Critical security gaps
- [ ] **PolkaVM gas limit** — `SandboxConfig` has no `max_gas_per_update`; a looping guest `update()` blocks the main thread forever. Add `max_gas_per_update: u64` (default 50M) and call `instance.set_gas()` before each `call_update`. `CallError::NotEnoughGas` → show "app timed out" error to user.
- [ ] **App ID collision via filename** — two `counter.zml` files in different directories share the same broker permission set. Use canonical path (or Blake3 hash of path + content) as app_id in `DeclarativeAppTab` and `SandboxAppTab`.
- [ ] **ZML actions not broker-checked at execution time** — `exec_actions` runs actions without consulting the broker. Add per-action broker checks for fetch/storage/clipboard before the capability is implemented to avoid accidental escalation.
- [ ] **Network fetch is fully stubbed** — broker allows fetch but nothing executes. When implementing: run on background thread (GPUI background executor), cap response size (10 MB), reject redirect chains outside declared domain.
- [ ] **Permission store in cwd** — `epoca_permissions.json` lives in the working directory; any local process can escalate permissions by editing it. Move to `~/Library/Application Support/Epoca/` on macOS; add note about future read-only admin policy layer at `/Library/Managed Preferences/`.

### QA findings (from automated review)
- [ ] **Broker lock poisoning ignored** — all `broker.lock()` calls silently discard poisoning. Recover with `poisoned.into_inner()` and log the error so permission failures are visible.
- [ ] **ZML state reset heuristic too coarse** — state is fully reset if state-block *count* changes. Should compare variable names instead, so adding a new `@state` preserves existing values.
- [ ] **`find_node_by_callback` unbounded recursion** — malformed ZML with deeply nested views could stack-overflow. Add a depth limit (e.g. 1000).

---

## P2 — Architecture (Architect review findings)

### Tab system
- [ ] **`TabBehavior` / `NavHandler` trait** — `Workbench` calls `entity.downcast::<WebViewTab>()` in every navigation method. Add a `NavHandler` trait (or vtable struct) stored in `TabEntry` so adding a new navigable tab type requires zero changes in `workbench.rs`. (Already partially implemented — eliminates all downcast call sites.)
- [ ] **`TabKind` closed enum** — adding split-view, PiP, WASM, or AI tabs requires a new variant and a new match arm everywhere. Long-term, migrate to a trait-based or capability-flag model.
- [ ] **Pause PolkaVM poll for inactive tabs** — each `SandboxAppTab` spawns an unconditional 33 ms timer. With 20 open tabs = 600 ms of wakeups/sec. Skip `call_update` when the tab is not the active one.

### Platform abstraction
- [ ] **macOS ObjC code inlined in `tabs.rs` / `workbench.rs`** — all `#[cfg(target_os = "macos")]` HAL functions should move to a `platform/macos.rs` module (or a future `epoca-platform-hal` crate) behind a `PlatformHal` trait. Enables Linux/Windows porting without auditing workbench logic.
- [ ] **`sidebar_blocker_ptr: u64` unsound** — raw `*mut AnyObject` stored as integer. Wrap in `struct SidebarBlocker(*mut AnyObject)` with `unsafe impl Send` and a documented safety invariant.
- [ ] **`CHROME: f32 = 10.0` duplicated** — `update_sidebar_blocker` duplicates the chrome inset from `workbench.rs`. Extract to a shared constant.

### State management
- [ ] **GPUI globals not scalable** — `OverlayLeftInset` and `OmniboxOpen` cause O(n tabs) ObjC calls per animation frame. Migrate to a `TabCommand` enum that `Workbench` fans out to each tab entity via `entity.update(cx, ...)`.

---

## P2 — Distribution & Auto-Update

- [ ] **macOS .app packaging** — use `cargo-bundle` (or custom `build.sh`) to produce a properly structured `.app` bundle; wire `Info.plist`, icon set, and entitlements.
- [ ] **Code signing + notarization** — `codesign --deep --timestamp` with an Apple Developer certificate; `xcrun notarytool` for Gatekeeper clearance. Required before any public distribution.
- [ ] **Sparkle auto-updater** — integrate Sparkle 2 via objc2 bindings (`SparkleUpdater` wrapper crate); host a signed `appcast.xml` on a CDN (e.g. `updates.getepoca.com`); call `checkForUpdatesInBackground` at launch. Sparkle handles delta updates, Ed25519 signature verification, and the native macOS update UI.
- [ ] **Linux: AppImage + self-update** — produce `.AppImage` with `appimage-update` for delta updates.
- [ ] **Windows: signed MSI** — use `cargo-wix` or `msiexec` with a signed MSI installer.
- [ ] **GitHub Releases** (interim) — `self_update` crate for in-app update check pointing at GitHub Releases API; zero infrastructure needed.

---

## P3 — Moonshots

- [ ] **WASM guest apps**: compile Rust/TS/Python to WASM, run as sandboxed tabs —
  a superset of PolkaVM
- [ ] **Decentralized content**: IPFS/Arweave tab renderer, ENS domain support
- [ ] **Hardware attestation**: verify page JS hasn't been tampered with using
  reproducible builds + WASM attestation
- [ ] **Browser-as-IDE**: CodeEditorTab with LSP support, run local dev servers as tabs
- [ ] **Physical-world tabs**: NFC/QR scanner as a tab type (mobile)

---

## Tracking

This backlog lives in `docs/backlog.md` and is the source of truth for product priorities.
For implementation details on locked-in design decisions, see `docs/design.md`.
Update this file in the same commit as any feature work so it stays current.
