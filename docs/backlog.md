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
- [x] ~~**Omnibox focus**: ensure omnibox input auto-focuses when opened~~ (done — `new_tab()` calls `window.focus(&focus_handle)`)
- [x] ~~**WelcomeTab startup**: app should open omnibox immediately on launch~~ (done — `new_tab(window, cx)` called on startup)
- [ ] **Crash on fast sidebar toggle**: rapid toggle can leave animation task
  in inconsistent state

---

## P1 — Privacy & Content Blocking (deeper than Brave)

**Architecture:** Six-layer pipeline in `epoca-shield` crate.
See `docs/content-blocking.md` for the full design document.

**Why this beats Chrome uBlock Origin:**
WKContentRuleList rules run in the OS network process — isolated from page JS,
undetectable by pages. Chrome Manifest V3 extensions run in the renderer. Brave
uses the same WKContentRuleList on macOS but Epoca adds CNAME uncloaking and
video-overlay sweeping which Brave does not have.

### P0 — Infrastructure ✅ DONE
- [x] Create `epoca-shield` crate with `ShieldConfig`, `CompiledRuleSet`, `ShieldManager`
- [x] HTTP list fetcher with ETag caching to `~/.epoca/content-rules/lists/`
- [x] ABP/uBlock filter parser → WKContentRuleList JSON compiler (45k-rule bucket splitting)
- [x] Install compiled rule lists on `WebViewTab` construction via objc2 bridge
- [x] Register `epocaShield` `WKScriptMessageHandler` for blocked-count telemetry
- [x] `ShieldManager` GPUI global in `epoca-core/src/shield.rs`

### P1 — Core blocking (partially done)
- [x] EasyList + EasyPrivacy + AdGuard Base → `epoca-rules-*` lists
- [x] Cosmetic compiler: `##` rules → CSS injection via `WKUserScript` (document_end)
- [ ] `window.open` block: `createWebViewWith` delegate denial + document_start override
- [x] **Shield status UI** — Eye/EyeOff icon, per-site toggle popover, blocked count badge
- [x] Settings page: toggle shield on/off
- [x] Background update loop (6-hour interval)
- [ ] Per-domain exception list management in Settings (currently only toggle from URL bar)
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

---

## P1 — First-Class Browser Features

### Navigation & History
- [ ] Persist browsing history to SQLite (`~/.epoca/history.db`)
- [ ] Omnibox autocomplete from history + open tabs
- [ ] Back/forward swipe gestures (macOS trackpad)
- [ ] Reading list / bookmarks (local, no sync account required)

### Tab Management
- [x] ~~Session contexts~~ (experimental — named contexts to share cookies across tab groups)
- [ ] Session restore on launch (persist open tabs to disk, reopen on next launch)
- [ ] Duplicate tab
- [ ] Drag-to-reorder in sidebar
- [ ] Pin/unpin tab (UI wired but persistence not implemented)
- [ ] Tab search (filter sidebar by title/URL — omnibox partially does this)
- [ ] Mute tab audio

### UI / UX
- [x] ~~**Crash reporting**~~ — Sentry integrated with compile-time `SENTRY_DSN` env var
- [x] ~~**Keyboard shortcut system**~~ — ⌘T, ⌘W, ⌘L, ⌘R, ⌘⇧R, ⌘Q, ⌘N, ⌘, all wired
- [x] ~~**Multi-window support**~~ — ⌘N opens new window with cascading offset
- [x] ~~**Per-tab favicon fetched and displayed**~~ — FAVICON_SCRIPT + epocaFavicon handler
- [ ] Dark/light mode toggle (system follow already works via WKWebView theme)
- [x] ~~**Page title propagated**~~ — TITLE_TRACKER_SCRIPT + epocaMeta handler updates sidebar
- [ ] Find-in-page (⌘F)
- [ ] Full-screen mode (hide sidebar, maximize content)

### Testing
- [ ] GPUI `#[gpui::test]` — headless unit/integration tests for workbench logic via `TestAppContext`
- [ ] Appium Mac2Driver — E2E UI testing via macOS Accessibility API

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
- [x] **PolkaVM gas limit** — implemented in `SandboxConfig`
- [x] **App ID collision via filename** — uses canonical path
- [ ] **ZML actions not broker-checked at execution time** — `exec_actions` runs actions without consulting the broker. Add per-action broker checks for fetch/storage/clipboard.
- [ ] **Network fetch is fully stubbed** — broker allows fetch but nothing executes. When implementing: run on background thread, cap response size (10 MB), reject redirect chains outside declared domain.
- [ ] **Permission store in cwd** — `epoca_permissions.json` lives in the working directory. Move to `~/Library/Application Support/Epoca/` on macOS.

### QA findings (from automated review)
- [ ] **Broker lock poisoning ignored** — all `broker.lock()` calls silently discard poisoning. Recover with `poisoned.into_inner()` and log the error.
- [ ] **ZML state reset heuristic too coarse** — state is fully reset if state-block *count* changes. Should compare variable names instead.
- [ ] **`find_node_by_callback` unbounded recursion** — malformed ZML with deeply nested views could stack-overflow. Add a depth limit (e.g. 1000).

---

## P2 — Architecture (Architect review findings)

### Tab system
- [x] ~~**`NavHandler` trait**~~ — implemented, eliminates all downcast call sites
- [ ] **`TabKind` closed enum** — adding split-view, PiP, WASM, or AI tabs requires a new variant. Long-term, migrate to trait-based or capability-flag model.
- [ ] **Pause PolkaVM poll for inactive tabs** — each `SandboxAppTab` spawns an unconditional 33 ms timer. Skip `call_update` when the tab is not active.

### Platform abstraction
- [ ] **macOS ObjC code inlined in `tabs.rs` / `workbench.rs`** — move to `platform/macos.rs` module behind a `PlatformHal` trait for Linux/Windows porting.
- [ ] **`sidebar_blocker_ptr: u64` unsound** — raw `*mut AnyObject` stored as integer. Wrap in `struct SidebarBlocker(*mut AnyObject)` with `unsafe impl Send`.
- [ ] **`CHROME: f32 = 10.0` duplicated** — extract to a shared constant.

### State management
- [ ] **GPUI globals not scalable** — `OverlayLeftInset` and `OmniboxOpen` cause O(n tabs) ObjC calls per animation frame. Migrate to a `TabCommand` enum.

---

## P2 — Distribution & Auto-Update

- [ ] **macOS .app packaging** — `cargo-bundle` or custom `build.sh` for `.app` bundle with `Info.plist`, icon set, entitlements.
- [ ] **Code signing + notarization** — `codesign --deep --timestamp` + `xcrun notarytool` for Gatekeeper.
- [ ] **Sparkle auto-updater** — Sparkle 2 via objc2; host signed `appcast.xml` on CDN.
- [ ] **Linux: AppImage + self-update**
- [ ] **Windows: signed MSI**
- [ ] **GitHub Releases** (interim) — `self_update` crate for in-app update check.

---

## P3 — Moonshots

- [ ] **WASM guest apps**: compile Rust/TS/Python to WASM, run as sandboxed tabs
- [ ] **Decentralized content**: IPFS/Arweave tab renderer, ENS domain support
- [ ] **Hardware attestation**: verify page JS via reproducible builds + WASM attestation
- [ ] **Browser-as-IDE**: CodeEditorTab with LSP support, run local dev servers as tabs
- [ ] **Physical-world tabs**: NFC/QR scanner as a tab type (mobile)

---

## What's Next — Product Thinking

### Immediate (next 1-2 sessions)
The biggest gap between "project" and "product" is **session restore** + **history**. Without
these, closing the app loses all state. Users won't adopt a browser that forgets everything.

1. **Session restore on launch** — serialize open tabs (URLs + context IDs) to
   `~/.epoca/session.json` on quit/crash; reopen on next launch. Low effort, high impact.
2. **Find-in-page (⌘F)** — table-stakes browser feature. WKWebView exposes
   `evaluateJavaScript("window.find(...)")` or the native `WKWebView._findString:` SPI.
3. **Sidebar bug validation** — the P0 sidebar layout bug needs a definitive test.

### Short-term (next 2-4 sessions)
4. **Browsing history** — SQLite-backed history with omnibox autocomplete. This is
   foundational for bookmarks, reading list, and sync later.
5. **Bookmarks / reading list** — local JSON or SQLite store, no account required.
6. **macOS .app packaging** — makes it shareable. Currently requires `cargo run`.

### Medium-term
7. **Find-in-page** and **back/forward swipe gestures** round out the browser basics.
8. **Tab drag-to-reorder** and **pin persistence** improve daily usability.
9. **Distribution** — code signing + notarization + Sparkle for auto-update.

### What to deprioritize
- **Fingerprinting protection** and **CNAME uncloaking** are impressive technically but
  won't be noticed by early adopters who are already on macOS (Safari's ITP handles most
  tracking). Ship these after the browser basics are solid.
- **Android** — park until macOS is feature-complete enough to share.
- **Local AI** — exciting but premature until the core browser loop is polished.

---

## Tracking

This backlog lives in `docs/backlog.md` and is the source of truth for product priorities.
For implementation details on locked-in design decisions, see `docs/design.md`.
Update this file in the same commit as any feature work so it stays current.
