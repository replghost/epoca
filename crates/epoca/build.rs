fn main() {
    // On macOS, embed a stable CFBundleIdentifier in the binary at link time.
    //
    // Without this, the linker generates a code-signing identifier from a hash
    // of the binary content (e.g. "epoca-725c8b98a71875df"), which changes on
    // every build.  WKWebView keys its WebCrypto master key in the macOS
    // Keychain to this identifier, so every new build looks like a new app →
    // errSecDuplicateItem (-25299) → "wants to access your keychain" dialog.
    //
    // Embedding an Info.plist with CFBundleIdentifier = "com.replghost.epoca"
    // gives the linker a stable identity that doesn't change between builds.
    #[cfg(target_os = "macos")]
    {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        // dev-info.plist lives two levels up from crates/epoca/ at the repo root.
        let plist = format!("{manifest_dir}/../../dev-info.plist");
        println!(
            "cargo:rustc-link-arg=-Wl,-sectcreate,__TEXT,__info_plist,{plist}"
        );
        // Re-run if the plist changes.
        println!("cargo:rerun-if-changed={plist}");
    }
}
