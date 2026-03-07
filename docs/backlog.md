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

- [x] ~~**Sidebar layout bug**: content not extending to left edge when sidebar collapses~~ (resolved — `_inset_subscription` fix + CALayer mask working correctly)
- [x] ~~**Omnibox focus**: ensure omnibox input auto-focuses when opened~~ (done — `new_tab()` calls `window.focus(&focus_handle)`)
- [x] ~~**WelcomeTab startup**: app should open omnibox immediately on launch~~ (done — `new_tab(window, cx)` called on startup)
- [x] ~~**Crash on fast sidebar toggle**~~ (resolved — generation counter `sidebar_anim_gen` prevents concurrent animation loops)

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
- [x] ~~`window.open` block: overridden at document_start in HOST_API_SCRIPT~~ (2026-03-06)
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
- [x] ~~Persist browsing history to SQLite (`~/.epoca/history.db`)~~ (done — `history.rs`, configurable TTL, frecency search)
- [x] ~~Omnibox autocomplete from history + open tabs~~ (done — cached history results in omnibox, Globe icon, two-line layout)
- [x] ~~Back/forward swipe gestures (macOS trackpad)~~ (2026-03-07)
- [x] ~~Reading list / bookmarks (local, no sync account required)~~ (2026-03-06)

### Tab Management
- [x] ~~Session contexts~~ (experimental — named contexts to share cookies across tab groups)
- [x] ~~Session restore on launch (persist open tabs to disk, reopen on next launch)~~ (2026-03-04)
- [ ] Duplicate tab
- [x] ~~Drag-to-reorder in sidebar~~ (2026-03-07)
- [ ] Pin/unpin tab (UI wired but persistence not implemented)
- [ ] Tab search (filter sidebar by title/URL — omnibox partially does this)
- [ ] Mute tab audio

### Credential Autofill (1Password integration)
- [ ] **System credential autofill via WKWebView** — Verify `WKWebViewConfiguration` allows macOS `ASCredentialProviderExtension` autofill (1Password, iCloud Keychain). Check that sandboxed and `nonPersistentDataStore` WebViews don't opt out. No Epoca code needed if WKWebView config is compatible; fix config if it blocks autofill.

### UI / UX
- [x] ~~**Crash reporting**~~ — Sentry integrated with compile-time `SENTRY_DSN` env var
- [x] ~~**Keyboard shortcut system**~~ — ⌘T, ⌘W, ⌘L, ⌘R, ⌘⇧R, ⌘Q, ⌘N, ⌘, all wired
- [x] ~~**Multi-window support**~~ — ⌘N opens new window with cascading offset
- [x] ~~**Per-tab favicon fetched and displayed**~~ — FAVICON_SCRIPT + epocaFavicon handler
- [ ] Dark/light mode toggle (system follow already works via WKWebView theme)
- [x] ~~**Page title propagated**~~ — TITLE_TRACKER_SCRIPT + epocaMeta handler updates sidebar
- [x] ~~Find-in-page (⌘F)~~
- [ ] Full-screen mode (hide sidebar, maximize content)

### Wallet
- [ ] **ETH wallet bridge (`window.ethereum` EIP-1193)** — Expose Epoca wallet to Ethereum dapps via a `window.ethereum` provider object. Relay `eth_requestAccounts`, `eth_sendTransaction`, `personal_sign`, `eth_signTypedData_v4` to the native wallet layer. Mirrors existing BTC wallet bridge pattern (JS init script → WKScriptMessageHandler → mpsc channel → workbench drain). Requires Helios light client for `eth_call` / `eth_getBalance` verification.
- [ ] Multi-account wallet selection — "Connect wallet?" prompt should let user pick which account to share (currently single root keypair, no choice). Requires: multiple mnemonics or HD-derived accounts, account picker UI in the connect banner, per-tab tracking of which account is connected.
- [ ] Per-app derived accounts in dapp flow — `app_keypair(app_id)` infra exists but `enable()` always exposes root address. Option to use derived per-origin accounts for isolation.

### Testing
- [ ] GPUI `#[gpui::test]` — headless unit/integration tests for workbench logic via `TestAppContext`
- [ ] Appium Mac2Driver — E2E UI testing via macOS Accessibility API

---

## P1 — PolkaVM App Platform (Epoca's unique value)

### `.prod` Bundle Format
Sandboxed PolkaVM apps are packaged as `.prod` files — CARv1 (IPFS-native) or ZIP archives:
```
my-app.prod (CARv1 or ZIP)
├── manifest.toml       # required — declares type, permissions, metadata
├── app.polkavm          # required for non-SPA — compiled guest binary
├── icon.png             # optional — app icon (256x256)
├── assets/              # optional — images, data files
│   └── ...
└── signature.toml       # optional — ed25519 signature over manifest + binary hash
```
- [x] **`ProdBundle` loader** — `epoca-sandbox/src/bundle.rs`: auto-detects ZIP or CARv1 format, parses manifest, loads `app.polkavm` + `assets/`
- [x] **CARv1 parser** — `epoca-sandbox/src/car.rs`: walks dag-pb/UnixFS directory trees, reassembles multi-chunk files, handles raw leaves
- [x] **`prod-pack` CLI** — `tools/prod-pack`: converts a bundle directory into a CARv1 `.prod` file with SHA-256 CIDv1 blocks
- [x] **CID verification** — recompute SHA-256 per block during CAR parse and compare against embedded CID
- [x] **Lazy IPFS asset loading** — DOTNS-resolved SPA tabs fetch assets on-demand from IPFS gateway instead of downloading everything upfront. `AssetSource::Lazy` in `spa.rs`, `dotns::resolve_lazy()`, `ProdBundle.ipfs_cid`
- [ ] **`host_asset_read(name_ptr, name_len, offset, dst_ptr, max_len) -> u32`** — guest reads files from the `.prod` bundle's `assets/` directory
- [x] ~~**Bundle signature verification** — ed25519 over `sha256(manifest.toml) + sha256(app.polkavm)`; optional but checked if present~~ (2026-03-06)
- [ ] **`open_prod_bundle(path)`** in Workbench — reads `.prod`, dispatches to correct tab type based on `manifest.toml` type field

### Manifest Types
Four app archetypes, each with different surface area, lifecycle, and host API:

**`type = "application"` — Full Tab App** (extends current `SandboxAppTab`)
```toml
[app]
type = "application"
id = "com.example.notes"
name = "Quick Notes"
version = "1.0.0"
[permissions]
network = ["api.example.com"]
storage = "2mb"
clipboard = "write"
```
- Owns entire tab surface, all 13 node kinds available
- Lifecycle: `init()` once, `update()` at 30fps while tab visible, pauses when backgrounded
- [ ] Implement `type = "application"` manifest parsing in `ProdBundle`

**`type = "extension"` — Chat/Context Extension**
```toml
[app]
type = "extension"
id = "com.example.translate"
name = "Translate"
version = "1.0.0"
[extension]
surfaces = ["chat", "context-menu"]
triggers = ["translate", "tr"]
[permissions]
network = ["api.deepl.com"]
```
- Doesn't own a tab — contributes to host surfaces (chat panel, context menu, command palette)
- Event-driven lifecycle: `on_invoke(trigger, context) → ViewTree`
- Restricted node kinds (no full layout freedom, rendered within host chrome)
- [ ] Design extension host API: `on_invoke`, `on_message`, `InvokeContext`, `Surface` enum
- [ ] `ExtensionHost` in `epoca-core` — manages loaded extensions, routes triggers
- [ ] Extension surface integration (chat panel, context menu contributions)

**`type = "widget"` — Dashboard Widget**
```toml
[app]
type = "widget"
id = "com.example.weather"
name = "Weather"
version = "1.0.0"
[widget]
sizes = ["small", "medium"]
refresh = "30m"
default_size = "small"
[permissions]
network = ["api.openweathermap.org"]
geolocation = "coarse"
```
- Fixed-size card on a widget board/dashboard (like macOS Dashboard / iOS widgets)
- Sizes: `small` (160×160), `medium` (320×160), `large` (320×320)
- Lifecycle: `init()` once, `refresh()` on interval — NOT continuous 30fps
- Between refreshes, last ViewTree is cached and rendered statically
- Limited node kinds: Text, HStack, VStack, Container, Image, Chart, Spacer, Divider (no Input/Button, or tap-to-open-app only)
- [ ] `WidgetBoard` panel — grid layout of widget cards with host-controlled chrome
- [ ] `WidgetHost` — manages widget lifecycle, refresh timers, size negotiation
- [ ] Widget size negotiation protocol (`widget.sizes` in manifest ↔ host available space)

**`type = "spa"` — Sandboxed Single-Page App** (NEW — `SpaTab`)
```toml
[app]
type = "spa"
id = "com.example.meet"
name = "SS Meet"
version = "1.0.0"
[webapp]
entry = "index.html"
sandbox = "strict"
[permissions]
statements = true
sign = true
chain = true
data = true
media = ["camera", "audio"]
```
- Hosts a bundled client-side SPA (HTML/JS/CSS) in a WKWebView, loaded via `epocaapp://` custom URL scheme
- **No network access** — `WKURLSchemeHandler` serves assets from bundle; `block-all` WKContentRuleList prevents outbound HTTP
- **Host API injection** — `window.epoca` injected at `documentStart` provides signing, Statement Store, and WebSocket proxy
- **Per-request signing confirmation** — user sees payload summary + approve/reject dialog (reuses `PendingPermission` pattern)
- **Isolated data store** — each SpaTab uses `nonPersistentDataStore`, never shared default store
- Use cases: video conferencing (ss-meet), chat, collaborative editors, dashboards — any client-side web app that talks to Substrate
- [x] **`SpaTab` struct + `TabKind::Spa`** — placeholder UI, bundle loading, session restore support (2026-03-05)
- [x] **`ProdBundle` extended** — `program_bytes` optional for `type = "spa"`, `WebAppMeta` parsed from `[webapp]` section
- [x] **`open_webapp()` in Workbench** — dispatches `.prod` bundles with `type = "spa"` to `SpaTab`
- [x] **`WKURLSchemeHandler` for `epocaapp://`** — `with_custom_protocol` serves assets from `SPA_ASSETS` registry (eager or lazy IPFS)
- [x] **`window.epoca` injection** — `HOST_API_SCRIPT` at `documentStart`; Promise-based API with correlation IDs via `__epocaResolve`
- [x] **Signing relay** — `epocaHost` WKScriptMessageHandler → `PendingSpaSign` → GPUI confirmation dialog → host signs → JS Promise resolves
- [x] ~~**Statements pub/sub** — `epoca.statements.write/subscribe`, local in-memory pub/sub, namespaced by app_id, drain loop pushes events to webviews~~ (2026-03-06)
- [x] ~~**Chain query/submit** — `epoca.chain.query` (method allowlist) + `epoca.chain.submit` (approval dialog), chain read from manifest~~ (2026-03-06)
- [x] ~~**Data connections scaffold** — `epoca.data.connect/send/close`, connection state management, approval dialog, events wired (needs str0m WebRTC transport)~~ (2026-03-06)
- [x] ~~**Manifest permission enforcement** — chain/statements/data APIs require declared permissions in manifest~~ (2026-03-06)
- [ ] **Cross-host statement delivery** — statements currently local-only; needs gossip/networking for multi-node pub/sub
- [ ] **str0m WebRTC transport** — P2P data connections via str0m, SDP signaling over statements
- [x] **`block-all` WKContentRuleList** — `install_block_all_rule()` in `spa.rs` (skipped at runtime due to startup race; CSP headers enforce instead)
- [x] ~~**Bundle signature verification** — required for apps requesting `sign = true`~~ (2026-03-06)
- [x] ~~**TypeScript type definitions** — `types/epoca-host-api.d.ts` with full `window.epoca` types~~ (2026-03-06)

### Guest Host API (all app types)
Expand the host function surface beyond current `host_set_view`/`host_poll_event`/`host_fetch`/`host_log`:
- [ ] **Complete `host_fetch`** — currently broker-checked but actual HTTP request is `// TODO` in `tabs.rs:721`. Implement on background thread, cap response 10MB, reject redirect chains outside declared domains
- [ ] **`host_kv_get(key_ptr, key_len, dst_ptr, max_len) -> u32`** — scoped persistent key-value storage per app_id, backed by `~/.epoca/app-storage/<app_id>/`
- [ ] **`host_kv_set(key_ptr, key_len, val_ptr, val_len) -> u32`** — write, with broker-checked size limits from `permissions.storage`
- [ ] **`host_clipboard_write(ptr, len)`** / **`host_clipboard_read(dst_ptr, max_len) -> u32`** — with broker permission check
- [ ] **`host_time_ms() -> u64`** — monotonic milliseconds since sandbox init
- [ ] **`host_asset_read`** — read files from `.prod` bundle (see above)

### Framebuffer API (games, emulators, creative tools)
For guests that do software rendering instead of ViewTree UI:
```toml
[app]
type = "application"
id = "com.example.doom"
name = "DOOM"
[permissions]
gpu = "2d"
[sandbox]
framebuffer = true
max_gas_per_update = 2000000000
```
- [x] **`host_present_frame(ptr, width, height, stride)`** — guest hands ARGB pixel buffer to host; host converts to BGRA, uploads via GPUI `paint_image`
- [x] **`host_poll_input(buf_ptr, buf_len) -> u32`** — keyboard/mouse events as `InputEvent { type: u8, key_code: u8 }` structs
- [x] **`FramebufferSandboxInstance`** — variant of `SandboxInstance` with framebuffer host functions instead of ViewTree functions
- [x] **`FramebufferAppTab`** — new tab type using `paint_image` to blit pixels, captures GPUI key events → input queue, scales framebuffer to fill bounds
- [ ] **Gas metering for framebuffer apps** — configurable via `manifest.toml [sandbox]` section; `GasMeteringKind::Async` preferred for perf

### DOOM on PolkaVM (proof of concept)
Target: doomgeneric (minimal C port, 5 platform callbacks) running in PolkaVM, packaged as `.prod`.
Source lives in the separate [epoca-games](https://github.com/replghost/epoca-games) repo (GPL-2.0).
- [x] Create doom guest with Rust shim + `build.rs` for C cross-compilation
- [x] Patch doomgeneric WAD I/O (`w_wad.c`) to use `host_asset_read` instead of `fopen`/`fread`
- [x] Implement libc shim: `malloc`/`free` (8MB static arena bump allocator), `memcpy`/`memset`, `printf`→`host_log`, `exit`→`unimp`
- [x] Build pipeline: `cargo +nightly build -Z build-std=core,alloc --target $(polkatool get-target-json-path --bitness 32) --release` then `polkatool link`
- [x] ~~Validate 35fps at 320×200 in `FramebufferAppTab` with `doom1.wad` (shareware, ~4MB)~~ (50+ fps after JIT, 2026-03-07)
- [x] ~~Soft-float: verify clang `-march=rv32emac -mabi=ilp32e` soft-float works for Doom's trig (`cos`/`sin`/`atan2`)~~ (confirmed working via gameplay, 2026-03-07)

### Scene Graph API (3D, medium-term)
For GPU-accelerated 3D rendering — guest describes scene, host renders with Metal:
```rust
enum SceneNode {
    Mesh { vertices: AssetRef, indices: AssetRef, material: MaterialId, transform: Mat4 },
    Camera { fov: f32, near: f32, far: f32, transform: Mat4 },
    Light { kind: LightKind, color: Color, intensity: f32, transform: Mat4 },
    Group { children: Vec<SceneNode>, transform: Mat4 },
}
```
- [ ] Design `SceneTree` protocol in `epoca-protocol` (3D equivalent of `ViewTree`)
- [ ] `host_set_scene(ptr, len)` host function
- [ ] Metal scene renderer in `epoca-core` (or separate `epoca-3d` crate)
- [ ] `gpu = "3d"` permission level in broker

### Guest UI Framework (`epoca-guest-ui` evolution)
The guest UI toolkit is the `no_std` declarative layer guests write against. It produces ViewTree nodes;
the host renders them natively (GPUI on desktop, wgpu on Android, future: web/iOS).

**Investigate prior art:**
- [ ] **egui feasibility** — egui is immediate-mode and `std`-dependent; likely not suitable for `no_std` PolkaVM guests, but investigate `egui-miniquad` or `egui` core without backends. Could the retained-mode output (tessellated meshes) be sent over the protocol boundary?
- [ ] **SwiftUI-like declarative model** — current `epoca-guest-ui` builder API is closest to this. Investigate formalizing: `View` trait, `@State` equivalent via `use_state<T>()`, `ViewModifier` chains, conditional views, `ForEach` for lists
- [ ] **Iced `widget` core** — Iced separates widget logic from rendering; its `iced_core` is relatively clean. Could its `Widget` trait + `Layout` engine be adapted for `no_std`?
- [ ] **Xilem/Masonry patterns** — Xilem uses a functional reactive model with tree diffing. Similar to what we do. Study their `View` trait and diff algorithm.

**Framework expansion (regardless of base):**
- [ ] **Remaining node kinds**: render `List` (scrollable, recycled), `Image` (from assets or URL), `ZStack`, `Table`, `Chart` in `view_bridge.rs` (currently placeholder)
- [ ] **Semantic styles**: `.caption()`, `.title()`, `.destructive()`, `.secondary()` — host maps to theme
- [ ] **Layout hints**: `.padding(px)`, `.frame(min_w, max_w)`, `.alignment()`, `.spacing()`
- [ ] **Navigation**: `push_screen(ViewTree)`, `pop_screen()` for multi-screen apps (host manages a nav stack per tab)
- [ ] **State management**: `use_state<T>()` or `@State` equivalent — framework handles diffing so guests don't manually track changes
- [ ] **`ForEach` / list builder** — efficient list construction with stable IDs for diffing
- [ ] Apply diff patches from `diff_trees()` instead of full re-render (diffing code exists in `epoca-protocol`, currently unused in `ViewBridge::update_tree`)

### ZML / Declarative Apps
- [ ] ZML hot-reload in dev mode (already partly working)
- [ ] ZML standard library: fetch(), localStorage, clipboard
- [ ] ZML layout: flex wrap, grid support
- [ ] ZML components: Table, Chart, DatePicker, Modal
- [ ] Guest app marketplace (local directory of `.zml` / `.prod` apps, no central server)
- [ ] ZML ↔ PolkaVM bridge: ZML app delegates compute to a `.polkavm` module (`call(fn_name, args) → result`, not full UI takeover)
- [ ] ZML ↔ WebView bridge: guest app can open a WebView pane in split view

### App Installation & Library
App identity = `app.id` from manifest. Two `.prod` files with the same `id` are the same app (newer replaces older).

**Phase 1 — MVP (done)**
- [x] **File → Open App** — file picker filtered to `.prod`, opens and installs (2026-03-06)
- [x] **Install to `~/.epoca/apps/`** — on first open, extract `.prod` to `~/.epoca/apps/{app_id}/`; subsequent launches load from disk (2026-03-06)
- [x] **App Library tab** — built-in tab showing installed apps as a grid of cards (icon, name, last launched); click to launch (2026-03-06)
- [ ] **Session restore for FramebufferApp tabs** — re-open from installed path on restore
- [x] **Controls hint overlay** — `controls_hint` field in manifest, shown on first launch, dismissed on keypress (2026-03-06)

**Phase 2 — Polish**
- [ ] Register `.prod` UTI — Open With / double-click from Finder
- [ ] Drag & drop `.prod` onto window to install
- [ ] Omnibox integration — type app name, installed apps show in results
- [ ] Sidebar pinning for favorite apps

**Phase 3 — Ecosystem**
- [ ] App catalog — browse/install from hosted directory
- [ ] Auto-update — manifest `update_url`, periodic check
- [ ] Save data — `~/.epoca/apps/{id}/save/`, `host_save_read`/`host_save_write` host APIs
- [ ] Open-from-URL: download `.prod` from HTTPS, verify signature, prompt to install
- [ ] App registry protocol (simple JSON index over HTTPS, no central server required)
- [ ] `cargo-epoca` CLI: scaffolds guest projects, handles cross-compile + `polkatool link` + `.prod` packaging

### TCP Socket Host API (enables VNC, SSH, networked apps)
- [ ] `host_tcp_connect(addr_ptr, addr_len, port) -> handle`
- [ ] `host_tcp_read(handle, buf_ptr, buf_len) -> bytes_read`
- [ ] `host_tcp_write(handle, buf_ptr, buf_len) -> bytes_written`
- [ ] `host_tcp_close(handle)`
- [ ] Broker network permission gating for TCP connections
- [ ] Mouse input events for framebuffer apps (`host_poll_mouse`)

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

### Hardware Security Key Authentication (YubiKey / FIDO2)
Allow users to authenticate to websites using USB security keys (YubiKey, SoloKeys, etc.)
via the WebAuthn / FIDO2 standard. This is table-stakes for security-conscious users.

- [x] ~~**WebAuthn API relay**~~ — JS polyfill intercepts `navigator.credentials.create()` / `.get()`, relays via `epocaWebAuthn` WKScriptMessageHandler → mpsc channel → `evaluate_script` to resolve promises (`webauthn.rs`)
- [x] ~~**macOS AuthenticationServices integration**~~ — `ASAuthorizationController` with both `ASAuthorizationPlatformPublicKeyCredentialProvider` and `ASAuthorizationSecurityKeyPublicKeyCredentialProvider`, delegate + presentation anchor, full create/get ceremony flow (`webauthn.rs`)
- [ ] **Fallback: direct USB HID** — if AuthenticationServices doesn't cover the flow, use `IOKit` HID manager to talk CTAP2 directly to FIDO2 keys
- [x] ~~**Passkey support**~~ — platform passkey provider (`ASAuthorizationPlatformPublicKeyCredentialProvider`) included alongside security key provider in both create and get ceremonies
- [ ] **UI chrome** — security key prompt overlay (tap your key animation) when WebAuthn ceremony is in progress
- [ ] **Attestation policy** — broker-level setting to allow/deny attestation conveyance per origin

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
- [x] ~~**Permission store in cwd** — moved to `~/Library/Application Support/Epoca/permissions.json` on macOS, `~/.epoca/permissions.json` elsewhere~~ (2026-03-06)

### QA findings (from automated review)
- [x] ~~**Broker lock poisoning** — all `broker.lock()` calls now recover via `into_inner()` with error logging~~ (2026-03-06)
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

- [ ] **OpenTTD AI opponents**: re-enable Squirrel VM scripting via `setjmp`/`longjmp` exception shim, or write a simple hardcoded AI in C
- [ ] **Retro game/emulator ecosystem**: NES, GB, CHIP-8 emulators as `.prod` bundles using framebuffer API — "the app store for sandboxed retro gaming"
- [ ] **GPU-accelerated 3D apps**: scene graph protocol + Metal renderer for real 3D games in sandboxed tabs
- [ ] **WASM guest apps**: compile Rust/TS/Python to WASM, run as sandboxed tabs (alternative to PolkaVM for web-origin code)
- [ ] **Decentralized content**: IPFS/Arweave tab renderer, ENS domain support
- [ ] **Hardware attestation**: verify page JS via reproducible builds + WASM attestation
- [ ] **Browser-as-IDE**: CodeEditorTab with LSP support, run local dev servers as tabs
- [ ] **Physical-world tabs**: NFC/QR scanner as a tab type (mobile)
- [ ] **Guest-to-guest messaging**: broker-mediated channels between running `.prod` apps

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
