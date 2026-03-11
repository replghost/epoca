#![recursion_limit = "1024"]

pub mod workbench;
pub mod tabs;
pub mod view_bridge;
pub mod declarative;
pub mod shield;
pub mod webauthn;
pub mod settings;
pub mod session;
pub mod history;
pub mod spa;
pub mod js_bridge;
pub mod chain;
pub mod wallet;
pub mod host;
pub mod app_library;
pub mod reader;
pub mod chain_api;
pub mod statements_api;
pub mod data_api;
pub mod media_api;
pub mod bookmarks;
pub mod extrinsic;
pub mod app_storage;

#[cfg(feature = "test-server")]
pub mod test_server;

/// GPUI global that tells any `WebViewTab` how far to shift its WKWebView
/// bounds to the right, so the overlay sidebar can occupy that vacated zone
/// without being obscured by the native view.
#[derive(Clone, Default)]
pub struct OverlayLeftInset(pub f32);
impl gpui::Global for OverlayLeftInset {}

/// Published by `Workbench` whenever the omnibox overlay is shown or hidden.
/// `WebViewTab` observes this to hide the native WKWebView while the modal
/// is open — otherwise the NSView z-order puts web content on top of GPUI.
#[derive(Clone, Default)]
pub struct OmniboxOpen(pub bool);
impl gpui::Global for OmniboxOpen {}

/// Published by `Workbench` when an approval dialog is pending.
/// `WebViewTab` observes this to dim the WKWebView (alpha) and block clicks.
#[derive(Clone, Default)]
pub struct WebViewDimmed(pub bool);
impl gpui::Global for WebViewDimmed {}
