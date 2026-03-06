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
pub mod chain;
pub mod wallet;
pub mod app_library;

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
