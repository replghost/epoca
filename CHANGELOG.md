# Changelog

All notable changes to Epoca are recorded here.
Format loosely follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

---

## [Unreleased] — ongoing

### Added
- **Media API Phase A (functional getUserMedia + attachTrack)** — `window.epoca.media.getUserMedia()`
  now allocates opaque track IDs via `media_api.rs`, resolves the JS promise with `{audioTrackId, videoTrackId}`,
  then evaluates getUserMedia JS in the WKWebView (browser native stack, no ObjC/AVFoundation).
  `window.epoca.media.attachTrack(trackId, elementId)` wires a track to a DOM element via `srcObject`.
  `RTCPeerConnection`/`RTCSessionDescription`/`RTCIceCandidate` remain frozen; `getUserMedia` is
  intentionally left unblocked (harmless without PeerConnection). `cleanup_for_webview()` added to
  `SpaTab::drop`. Push events `mediaTrackReady`, `mediaConnected`, `mediaRemoteTrack`, `mediaClosed`,
  `mediaError` drain loop added to workbench render path. New `BridgeAsyncAction::MediaGetUserMedia`
  and `MediaAttachTrack` variants. `media_api.rs`, `js_bridge.rs`, `spa.rs`, `tabs.rs`, `workbench.rs`. (2026-03-07)

- **Media API scaffold (Phase A)** — `window.epoca.media` namespace injected into SPA WebViews
  with `getUserMedia`, `connect`, `close`, and `attachTrack` methods. Native WebRTC globals
  (`RTCPeerConnection`, `RTCSessionDescription`, `RTCIceCandidate`, `navigator.mediaDevices.getUserMedia`)
  are nulled at document_start so apps must go through the host API. Bridge variants
  `MediaGetUserMedia`, `MediaConnect`, `MediaClose`, `MediaAttachTrack` added to `BridgeRequest`.
  Permission checks enforce `media = ["camera"]`/`["audio"]` from `PermissionsMeta`. Push events
  `mediaTrackReady`, `mediaConnected`, `mediaRemoteTrack`, `mediaClosed`, `mediaError` wired into
  `__epocaPush`. TypeScript types in `types/epoca-host-api.d.ts` updated. `js_bridge.rs`, `spa.rs`,
  `tabs.rs`, `workbench.rs`. (2026-03-07)

- **Bundle signature verification** — `.prod` bundles can now include an optional `signature.toml`
  with an ed25519 public key and signature over `sha256(manifest.toml) || sha256(app.polkavm)`.
  SPA bundles use `sha256("")` for program hash. Self-signed model (integrity, not authenticity).
  ZIP and CAR parsers extract and verify the signature when present. Deps: `ed25519-zebra`,
  `hex`. `bundle.rs`, `SignatureFile`, `verify_bundle_signature()`. 12 unit tests. (2026-03-06)

- **Bookmarks** — Local bookmark storage backed by `~/.epoca/bookmarks.json`. In-memory cache
  avoids disk reads on the 30fps render path. URL normalization (lowercase host, strip fragment,
  trailing-slash handling) prevents duplicate entries. Atomic `toggle()` for star icon. Star icon
  in URL bar toggles bookmark for the active page (amber = bookmarked). Bookmarks panel accessible
  via File > Bookmarks (⌘⇧B), Add Bookmark (⌘D). Click to open, X to remove. Session-restorable.
  Platform-aware path (macOS `~/Library/Application Support/Epoca/`). `bookmarks.rs`, `BookmarksTab`
  in `tabs.rs`, `AddBookmark`/`OpenBookmarks` actions. 11 unit tests. (2026-03-06)

- **CARv1 .prod bundle format** — `.prod` bundles now support CARv1 (IPFS-native) in addition
  to ZIP. `ProdBundle::from_file()` and `from_bytes()` auto-detect format via magic bytes.
  Shared CAR/UnixFS parser extracted to `epoca-sandbox/src/car.rs` (~370 lines, zero external
  deps). Walks dag-pb directory trees, reassembles multi-chunk files, handles raw leaves.
  `epoca-chain/src/dotns.rs` deduplicated to use the shared parser. (2026-03-06)

- **`prod-pack` CLI tool** — `tools/prod-pack`: converts a bundle directory into a CARv1
  `.prod` file. Builds UnixFS directory DAG with raw leaf blocks (CIDv1 codec 0x55) and
  dag-pb directory blocks (CIDv1 codec 0x70), SHA-256 multihash. Hand-encoded CBOR header
  and protobuf. Round-trip verified against `epoca-sandbox` parser. Usage:
  `prod-pack <directory> [output.prod]`. (2026-03-06)

- **CID integrity verification** — CAR parser now recomputes SHA-256 for every block and
  compares against the digest embedded in the CID. Tampered or corrupted blocks are rejected
  with an error before any content is extracted. `epoca-sandbox/src/car.rs`. (2026-03-06)

- **Lazy IPFS asset loading for dot apps** — DOTNS-resolved SPA tabs no longer download all
  assets before opening. Only `manifest.toml` is fetched during resolution; remaining assets
  are fetched on-demand from the IPFS gateway when the WKWebView requests them via `epocaapp://`.
  `AssetSource` enum in `spa.rs` supports both `Eager` (local bundles) and `Lazy` (IPFS-backed
  with caching) modes. `ProdBundle.ipfs_cid` field enables lazy mode. `dotns::resolve_lazy()`
  fetches only manifest + CID. `dotns::ipfs_gateway()` exposes the gateway URL. (2026-03-06)

- **Approved dot apps persistence** — `approved_dot_apps` HashMap (app name → CID) now persists
  to `approved_apps.json` alongside `session.json`. Loaded on startup, saved on each approval.
  Returning to a previously-approved dot app with the same CID skips the permission prompt.
  `session.rs`: `load_approved_apps()`, `save_approved_apps()`. (2026-03-06)

- **DotLoadingTab + permission approval flow** — Navigating to `dot://name.dot` opens an
  animated loading tab with phases (Resolving → Fetching → Permission Review). Permission
  card shows actual entitlements from bundle `PermissionsMeta` (network, signing, statement
  store, media). Allow/Deny buttons with async-safe generation counter. `DotLoadingTab`,
  `DotLoadingEvent`, `PendingDotLoad` in `tabs.rs`/`workbench.rs`. (2026-03-06)

- **Ethereum Helios light client (Phase 4)** — `ConnectionBackend::Helios` variant added.
  Helios verifies beacon chain consensus and execution state locally — no trusted RPC servers.
  Public endpoints: consensus `ethereum.operationsolarstorm.org` (a16z), execution `eth.llamarpc.com`.
  Dedicated std::thread with tokio current-thread runtime. `FileDB` checkpoint persistence at
  `~/Library/Application Support/Epoca/chain-db/ethereum/` for fast restarts (<5s after first sync).
  Polls block number + gas price every 12s (one Ethereum slot). Reports `ChainExtra::Eth { finalized_block,
  gas_price_gwei }`. Gated behind `experimental_eth` setting. New dep: `helios-ethereum` (git, rev 204c998a).
  `eth.rs`, `client.rs`, `lib.rs`. (2026-03-06)

- **BTC wallet bridge (Phase 3)** — `window.bitcoin` Unisat-compatible JavaScript API injected into
  every WebView when `experimental_wallet` + `experimental_btc` are enabled. Methods: `requestAccounts`,
  `getAccounts` (non-prompting), `signMessage` (BIP-137 with SHA256d, recoverable ECDSA, native SegWit
  header 39+recid), `getNetwork` ("livenet"), `getBalance` (stub zeros — real UTXO scanning in Phase 3.5),
  `on`/`removeListener` event emitter. User approval required for address exposure (shared `connected_sites`
  with Polkadot wallet) and per-request sign confirmation dialog. `WalletChannel` enum dispatches shared
  connect banner. Private keys never cross JS boundary. `btc_sign_message()` in `WalletManager`,
  `BTC_WALLET_INJECT_SCRIPT`, `EpocaBtcWalletHandler` ObjC class, `BTC_WALLET_CHANNEL`. 5 new tests
  (varint, base64 output, recovery, locked, oversized). `wallet.rs`, `workbench.rs`, `tabs.rs`,
  `lib.rs`. (2026-03-06)

- **ETH + BTC key derivation (Phase 1)** — secp256k1 key derivation from the shared BIP-39
  mnemonic. ETH: BIP-44 `m/44'/60'/0'/0/0` with EIP-55 checksummed addresses. BTC: BIP-84
  `m/84'/0'/0'/0/0` with P2WPKH bech32 addresses. `WalletManager` extended with `eth_key`/`btc_key`
  fields, `eth_address()`, `btc_address()`, `eth_sign_personal()` (EIP-191 with signature recovery),
  `btc_sign_raw()` (DER ECDSA). Keys derived on unlock/create/import, cleared on lock. Seed arrays
  and mnemonic strings zeroized after use. New deps: `k256`, `bip32`, `sha2`, `sha3`, `ripemd`,
  `bech32`. 20 tests including BIP-84 spec vector and ETH address recovery. `derive.rs`, `lib.rs`.
  (2026-03-05)

- **Bitcoin Kyoto light client (Phase 2/2.5)** — `ChainId::Ethereum` and `ChainId::Bitcoin` added to
  `epoca-chain`. Bitcoin backend uses Kyoto BIP-157/158 compact block filter light client — connects
  directly to Bitcoin P2P network, downloads block headers and compact filters, verifies proof-of-work
  locally. No trusted servers. `ConnectionBackend::Kyoto` variant. Tokio current-thread runtime on
  dedicated std::thread. Block header/filter data persisted to `~/Library/Application Support/Epoca/
  chain-db/bitcoin/`. `ChainExtra` enum surfaces chain-specific data (ETH finalized block + gas, BTC
  tip height + fee rate). Settings flags `experimental_eth` and `experimental_btc`. New deps: `bip157`,
  `tokio`. `btc.rs`, `client.rs`, `settings.rs`. (2026-03-05)

- **SPA Tab (sandboxed single-page app)** — New `SpaTab` tab type and `TabKind::Spa` for hosting
  bundled client-side web apps in a sandboxed WKWebView. `.prod` bundle format extended: `type = "spa"`
  in manifest, `[webapp]` section with `entry` + `sandbox`, `program_bytes` now optional. Broker
  `Permissions` extended with `sign`, `statement_store`, `media`. `open_webapp()` dispatches SPA
  bundles. Placeholder UI renders while WKURLSchemeHandler integration is pending. `bundle.rs`,
  `tabs.rs`, `workbench.rs`, `session.rs`, `broker/lib.rs`. (2026-03-05)

- **Content Shield description updated** — Settings subtitle now accurately lists all 9 filter lists
  (EasyList, AdGuard, uBlock Origin, Fanboy, Peter Lowe) instead of just "EasyList + EasyPrivacy". (2026-03-05)

- **Browsing history (ephemeral with TTL)** — SQLite-backed browsing history at
  `~/.epoca/history.db`. Configurable retention: Session Only / 8h / 24h (default) / 7d / 30d.
  Hard DELETE on expiry, incremental vacuum. Frecency-ordered search in omnibox (⌘T) with
  "History" divider between tab matches and history results. File permissions 0600. Corrupt DB
  auto-recreated. `history.rs`, `HistoryRetention` enum in settings, retention selector in
  Settings UI. 29 unit tests. (2026-03-05)

- **Session restore** — Open tabs are saved to `session.json` every 30s and on quit (⌘Q). On next
  launch (no CLI arg), tabs are restored with their URLs, titles, favicons, pinned state, and
  session contexts. Atomic writes via `.tmp` + rename. Skips non-restorable tab types (SandboxApp,
  FramebufferApp, Welcome). `session.rs`, `WorkbenchRef` global. (2026-03-04)

- **Find-in-page (⌘F)** — Opens a find bar between chrome and content area. Live search on typing,
  Enter for next match, Shift+Enter/↑ for previous, Escape to close. Uses `window.find()` JS API
  on the active WebViewTab. No-op on non-WebView tabs. `FindInPage`, `FindPrev`, `CloseFindBar`
  actions. Edit menu with Find entry. (2026-03-04)

- **Embedded HTTP test server** — `localhost:9223` test API behind `#[cfg(feature = "test-server")]` +
  `EPOCA_TEST=1`. Endpoints: `GET /state` (full app snapshot), `POST /action` (navigate, new_tab,
  close_tab, etc.), `GET /webview/eval?js=X` (JS eval with correlation IDs). Channel-drain pattern
  matches existing shield.rs architecture. `test_server.rs`, `tools/test_cursor.sh`. (2026-03-04)

- **Session contexts** — Named browsing sessions with separate WKWebView data stores. Each context
  shares cookies/storage across its tabs; tabs without a context are fully private. Context picker
  dropdown in URL bar (colored dot prefix). "Open in Context" submenu in right-click menu.
  `experimental_contexts` setting toggle. `SessionContext` type in settings.rs. (2026-03-04)

- **"+ New Context" in dropdown** — Context picker dropdown includes a "+ New Context" row at the
  bottom, so users can create contexts without going to Settings. Auto-picks first unused color
  from preset palette. (2026-03-04)

- **"Private Tab" in context menu** — "Open in Context" right-click submenu always shows "Private Tab"
  option (opens link with no context/isolated data store). `MenuAction::OpenPrivate`. (2026-03-04)

- **URL bar triple-click select-all** — Triple-clicking the URL bar selects the entire URL, matching
  standard browser behavior. Uses `on_mouse_down` with `click_count >= 3` to dispatch
  `gpui_component::input::SelectAll`. (2026-03-04)

- **Back/forward swipe gestures** — Two-finger trackpad swipe navigates back/forward in
  WebView tabs. Custom JS-based implementation with rubber-band resistance physics. Edge
  shadow + frosted chevron arrow slides in from the swipe edge; arrow brightens near threshold.
  Triggers after 150px accumulated delta. Springs back on cancel. `SWIPE_NAV_SCRIPT` in
  `tabs.rs`, `NavEvent` enum in `shield.rs`. (2026-03-07)

- **Tab drag-to-reorder** — Tabs in the sidebar can be reordered by dragging. Mouse-down starts
  a potential drag; 4px movement threshold activates it. Dragged tab shows at 50% opacity.
  Reordering respects pinned/regular group boundaries. Uses manual `on_mouse_down` /
  `on_mouse_move` / `on_mouse_up` pattern (GPUI has no built-in drag API). `DragState`,
  `reorder_tab()` in `workbench.rs`. (2026-03-07)

### Fixed
- **blake2 import in statement_store.rs** — `blake2::Blake2b256` doesn't exist in blake2 0.10.
  Fixed with `type Blake2b256 = Blake2b<U32>;` using `digest::consts::U32`. (2026-03-06)

- **Borrowed variable escaping thread::spawn in statements_api.rs** — `channel` (a `&str`) was
  captured by a `thread::spawn` closure. Added `let channel_owned = channel.to_string();` before
  the closure. (2026-03-06)

- **Statement store initialization** — `init()` now generates an ephemeral sr25519 keypair
  internally (no wallet dependency). Network submit uses a bounded `mpsc::sync_channel(32)`
  worker thread instead of unbounded thread spawning per write. `statement_store.rs`,
  `statements_api.rs`. (2026-03-06)

- **Mutex poison recovery in tabs.rs** — `PENDING_BOOKMARK_OPEN` and `PENDING_LAUNCH` static
  mutex writers changed from `.unwrap()` to `if let Ok(mut guard)` to prevent UI-thread panics
  if a previous thread panicked while holding the lock. (2026-03-06)

- **Cursor pointer (hand icon) on links** — GPUI's `reset_cursor_style()` overrode WKWebView's CSS
  cursor every paint frame. Fix: `canvas` paint closure calls `window.set_window_cursor_style(
  CursorStyle::PointingHand)` with `hitbox_id: None`, bypassing hit-test. (2026-03-04)

- **URL bar navigates in-place** — Entering a URL or search query in the URL bar now navigates the
  current WebView tab instead of always opening a new tab. New tabs only opened from omnibox (⌘T)
  or when no navigable tab exists. (2026-03-04)

- **Context indicator syncs on tab switch** — `active_context` now updates when switching tabs, but
  only from WebView tabs. Non-WebView tabs (Settings, Welcome) don't reset the context indicator.
  (2026-03-04)

- **Context switch reopens with live URL** — `switch_context()` reads the URL bar input (live URL)
  instead of the stale `TabKind::WebView { url }` that may reflect the original navigation. (2026-03-04)

- **Context dropdown backdrop covers full window** — Backdrop div now renders at the root Workbench
  level so clicks anywhere outside the dropdown dismiss it (previously only covered sidebar area).
  Dropdown also auto-closes when sidebar hides in overlay mode. (2026-03-04)

- **Orphaned tabs cleaned up on context delete** — When a context is deleted in Settings, tabs that
  referenced it have their `context_id` set to `None` (private). `active_context` also resets if it
  referenced the deleted context. Checked in `process_pending_nav`. (2026-03-04)

- **Duplicate context colors** — Both the dropdown "+ New Context" and Settings "Add Context" now
  pick the first unused color from `DEFAULT_CONTEXT_COLORS` instead of cycling by index, preventing
  duplicates after delete+create cycles. (2026-03-04)

- **Context dot sizes standardized** — URL bar dot 6px, tab sidebar dot 5px, dropdown dot 6px
  (previously inconsistent 4–7px). (2026-03-04)

- **Right-click link context menu** — Right-clicking a link in any WebView shows a native NSMenu with:
  Open in New Tab, Open in New Window, Open in Context (submenu, when experimental contexts enabled),
  and Copy Link Address. `CONTEXT_MENU_SCRIPT` (init script) intercepts `contextmenu` events on `<a>` tags,
  posts via `epocaContextMenu` WKScriptMessageHandler (`EpocaContextMenuHandler` ObjC class,
  `CONTEXT_MENU_CHANNEL`). NSMenu actions route back via `MENU_ACTION_CHANNEL` → `drain_menu_actions()`
  in `process_pending_nav`. `EpocaMenuTarget` ObjC class handles selector callbacks. (2026-03-04)

- **Favicon in tab list** — Sidebar tab rows show the site's favicon instead of the generic
  globe icon. `FAVICON_SCRIPT` finds the best `<link rel="icon">` URL or falls back to
  `/favicon.ico`, posts via `epocaFavicon` WKScriptMessageHandler (`EpocaFaviconHandler`
  ObjC class, `FAVICON_CHANNEL`, `drain_favicon_events()`). Stored in `TabEntry.favicon_url`;
  rendered with `img()` (GPUI URI image loading); falls back to `IconName::Globe`. (2026-03-03)

- **URL bar padding tightened** — Switched Input to `Size::Small` (8px horizontal padding,
  down from 12px) and `.appearance(false)` to avoid double bg/border. (2026-03-03)

- **Live page titles in tab list** — Sidebar shows the actual page title instead of the URL slug.
  `TITLE_TRACKER_SCRIPT` monitors `document.title` via MutationObserver + SPA navigation hooks,
  posts via `epocaMeta` WKScriptMessageHandler; `EpocaMetaHandler` ObjC class routes events by
  WKWebView pointer (`WebViewTab.webview_ptr`); drained each frame in `process_pending_nav`. (2026-03-03)

- **Link status bar** — Arc-style frosted-glass pill fixed at bottom-left of every WebView.
  Shows hovered link URL; updates to "Open in new tab: [url]" while ⌘ held and
  "Open in new tab → switch: [url]" while ⌘⇧ held. Fades in/out smoothly.
  `LINK_STATUS_SCRIPT` init script, idempotent. (2026-03-03)

- **Tab isolation ("Fresh Tab" mode)** — `Workbench.isolated_tabs: bool`. When enabled,
  every new WebView tab uses `WKWebsiteDataStore.nonPersistentDataStore()` (macOS) via
  wry's `.with_incognito(true)`. No cookies, localStorage, or cache shared between tabs or
  sessions. Defeats metered paywalls and cross-tab tracking. Toggle via
  `Workbench::set_isolated_tabs(bool)`. (2026-03-03)

- **Keyboard shortcuts & native menu bar** — ⌘T (New Tab), ⌘W (Close Tab), ⌘R (Reload),
  ⌘⇧R (Hard Reload), ⌘L (Focus URL Bar), ⌘Q (Quit). macOS menu bar: Epoca, File, View.
  Hard reload uses `reloadFromOrigin` (bypasses cache). (2026-03-03)

- **Cmd-click → open in background tab** — `⌘-click` any link opens it as a background tab
  (active tab unchanged). `⌘⇧-click` opens with focus. Implemented via `CMD_CLICK_SCRIPT`
  init script + `EpocaNavHandler` ObjC `WKScriptMessageHandler` + `NAV_CHANNEL` drain in
  Workbench. `Workbench.open_links_in_background: bool` (default `true`). (2026-03-03)

- **Cmd-click ripple** — Green expanding ring (`#44bb66`) radiates from the click point when
  a background tab is opened via ⌘-click. 400ms, non-blocking. `RIPPLE_SCRIPT` init script,
  idempotent via `window.__epocaRipple` guard. (2026-03-03)

- **Shield badge on globe icon** — Globe icon in URL bar turns green (`#44bb6699`) when the
  shield is active, red (`#cc444499`) when the site is excepted, muted when no shield is
  loaded. Color derived from `ShieldGlobal` state + hostname lookup each render. (2026-03-03)

- **Cosmetic blocked count in URL bar** — Small green number next to globe shows how many
  elements the shield hid on the current page. Sourced from `epocaShield` WKScriptMessageHandler
  (`EpocaShieldHandler` ObjC class, `SHIELD_CHANNEL`, `drain_shield_events()`), stored in
  `WebViewTab.blocked_count`. (2026-03-03)

- **Per-site shield exception toggle** — Eye/EyeOff button in URL bar suffix toggles the
  shield exception for the active tab's hostname. Calls `ShieldManager::toggle_site_exception()`
  via `ShieldGlobal`. Globe turns red when site is excepted. `ToggleSiteShield` GPUI action. (2026-03-03)

- **6-hour background filter list refresh** — `init_shield` bootstrap thread now loops,
  sleeping 6 hours between re-fetches of EasyList/EasyPrivacy, keeping rules fresh without
  a restart. Updates `COMPILED_CONFIG` static; applies to newly opened tabs. (2026-03-03)

---

## [0.0.1] — 2026-02-12

### Added
- **Workbench shell** — GPUI-based browser chrome with pinned and overlay sidebar modes.
  Sidebar animates in/out; traffic lights positioned inside the sidebar panel.
  Fullscreen-safe: sidebar hides cleanly, mini toolbar with pin button shown in fullscreen.

- **WebView tabs** — WKWebView-backed browser tabs via wry. CALayer mask clips rendering
  to sidebar-free region without changing the page viewport (no content reflow).
  `EpocaSidebarBlocker` NSView intercepts hit-testing in the sidebar overlay region.

- **Content blocking (epoca-shield)** — Four-layer pipeline:
  WKContentRuleList (EasyList + EasyPrivacy, 45k-rule bucket splitting),
  document_start JS (fingerprint countermeasures), document_end JS (overlay sweeper, cookie
  consent auto-dismiss), cosmetic count reporting via `epocaShield` WKScriptMessageHandler.
  _Not yet implemented: DNS CNAME uncloaking (hickory-dns), WKNavigationDelegate layer._

- **Omnibox** — ⌘T opens floating search/URL input. Accepts URLs, local file paths
  (`.toml`, `.zml`, `.polkavm`), and search queries (DuckDuckGo fallback).

- **Tab list with page titles** — Sidebar shows live page titles (tracked via
  `TITLE_TRACKER_SCRIPT` + `epocaMeta` WKScriptMessageHandler + `TITLE_CHANNEL` drain).
  Falls back to hostname while page loads.

- **Arc-style URL status bar** — Fixed bottom-left pill showing hovered link URL.
  Shows "Open in new tab: [url]" hint while ⌘ is held. Frosted-glass dark pill,
  monospace text, fades in/out smoothly. (`LINK_STATUS_SCRIPT` init script.)

- **Cmd-click ripple** — Green expanding ring (`#44bb66`) animates from the click point
  when a background tab is opened. 400ms, non-blocking. (`RIPPLE_SCRIPT` init script.)

- **Scrollbar styling** — Custom dark scrollbar matching browser chrome via
  `SCROLLBAR_CSS_SCRIPT`. Pointer cursor forced on all `<a href>` elements.

- **PolkaVM guest tabs** — Sandboxed PolkaVM guest applications run as first-class tabs.
  ZML declarative UI pipeline: `.zml` → Parser → Evaluator → ViewTree → GPUI.

- **Declarative app tabs** — `.toml` and `.zml` files open as tab apps. Dev mode
  (`--dev` flag) hot-reloads on file change.

- **Capability broker** — `epoca-broker` crate. App manifests declare permissions;
  broker enforces at runtime.

- **Android renderer** — `epoca-android` crate: winit 0.30 + wgpu 23 + cosmic-text 0.12.
  Custom 2D flexbox layout. Renders ViewTree from ZML pipeline. Desktop preview binary.

- **Crash reporting** — Sentry integration (opt-in via `SENTRY_DSN` env var).

- **CI** — GitHub Actions on `macos-latest`: test → build → fmt → clippy.

---

## Format notes
- Dates are when the feature landed in the working codebase, not when it was planned.
- Entries use past tense ("Added X") not present tense.
- Implementation detail (crate names, key types, ObjC class names) is included so release
  notes can be written from this file without re-reading source.
