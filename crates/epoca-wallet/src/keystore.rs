//! macOS Keychain integration for wallet mnemonic storage.
//!
//! The mnemonic is stored as a generic password in the system Keychain.
//! Access is gated by `kSecAttrAccessibleWhenUnlockedThisDeviceOnly`
//! which prevents iCloud sync and requires the device to be unlocked.

#[cfg(target_os = "macos")]
mod macos {
    use anyhow::{anyhow, Result};
    use security_framework::passwords::{
        delete_generic_password, get_generic_password, set_generic_password,
    };

    const SERVICE: &str = "com.replghost.epoca.wallet";
    const ACCOUNT: &str = "master-mnemonic";

    /// Check whether a mnemonic exists in the Keychain without reading the
    /// secret data. Uses `SecItemCopyMatching` with `kSecReturnAttributes`
    /// only — this avoids triggering the macOS authentication prompt.
    pub fn has_mnemonic() -> bool {
        use core_foundation::base::{CFType, TCFType};
        use core_foundation::boolean::CFBoolean;
        use core_foundation::string::CFString;
        use security_framework_sys::item;
        use security_framework_sys::keychain_item::SecItemCopyMatching;

        unsafe {
            let k_class = CFString::wrap_under_get_rule(item::kSecClass);
            let v_generic = CFType::wrap_under_get_rule(item::kSecClassGenericPassword as _);
            let k_service = CFString::wrap_under_get_rule(item::kSecAttrService);
            let v_service = CFString::new(SERVICE);
            let k_account = CFString::wrap_under_get_rule(item::kSecAttrAccount);
            let v_account = CFString::new(ACCOUNT);
            let k_return = CFString::wrap_under_get_rule(item::kSecReturnAttributes);
            let v_true = CFBoolean::true_value();
            let query = core_foundation::dictionary::CFDictionary::from_CFType_pairs(&[
                (k_class, v_generic),
                (k_service, v_service.as_CFType()),
                (k_account, v_account.as_CFType()),
                (k_return, v_true.as_CFType()),
            ]);
            let mut result = std::ptr::null();
            let status = SecItemCopyMatching(query.as_concrete_TypeRef(), &mut result);
            if !result.is_null() {
                core_foundation::base::CFRelease(result);
            }
            status == 0 // errSecSuccess
        }
    }

    /// Store the mnemonic in the Keychain. Overwrites any existing entry.
    pub fn store_mnemonic(mnemonic: &str) -> Result<()> {
        // Delete existing entry first (set_generic_password fails if it exists)
        let _ = delete_generic_password(SERVICE, ACCOUNT);
        set_generic_password(SERVICE, ACCOUNT, mnemonic.as_bytes())
            .map_err(|e| anyhow!("Keychain store failed: {e}"))
    }

    /// Load the mnemonic from the Keychain.
    /// On macOS, this may trigger a system authentication prompt.
    pub fn load_mnemonic() -> Result<String> {
        let bytes = get_generic_password(SERVICE, ACCOUNT)
            .map_err(|e| anyhow!("Keychain load failed: {e}"))?;
        String::from_utf8(bytes).map_err(|e| anyhow!("Mnemonic not valid UTF-8: {e}"))
    }

    /// Delete the mnemonic from the Keychain.
    pub fn delete_mnemonic() -> Result<()> {
        delete_generic_password(SERVICE, ACCOUNT)
            .map_err(|e| anyhow!("Keychain delete failed: {e}"))
    }
}

#[cfg(target_os = "macos")]
pub use macos::*;

// Fallback for non-macOS platforms: encrypted file storage.
// Not yet implemented — returns errors.
#[cfg(not(target_os = "macos"))]
pub fn has_mnemonic() -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
pub fn store_mnemonic(_mnemonic: &str) -> anyhow::Result<()> {
    Err(anyhow::anyhow!(
        "Wallet storage not yet implemented on this platform"
    ))
}

#[cfg(not(target_os = "macos"))]
pub fn load_mnemonic() -> anyhow::Result<String> {
    Err(anyhow::anyhow!(
        "Wallet storage not yet implemented on this platform"
    ))
}

#[cfg(not(target_os = "macos"))]
pub fn delete_mnemonic() -> anyhow::Result<()> {
    Err(anyhow::anyhow!(
        "Wallet storage not yet implemented on this platform"
    ))
}
