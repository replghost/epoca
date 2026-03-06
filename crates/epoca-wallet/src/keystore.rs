//! macOS Keychain integration for wallet mnemonic storage.
//!
//! The mnemonic is stored in the **Data Protection Keychain** with biometric
//! access control (Touch ID / Face ID, passcode fallback). Reading the secret
//! triggers a native biometric prompt — no ugly "allow access?" dialog.
//!
//! Legacy Keychain items (created before biometric support) are automatically
//! migrated to the Data Protection Keychain on the next successful `load_mnemonic`.

#[cfg(target_os = "macos")]
mod macos {
    use anyhow::{anyhow, Result};
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::string::CFString;
    use security_framework::access_control::{ProtectionMode, SecAccessControl};
    use security_framework::passwords::{
        delete_generic_password, generic_password, set_generic_password_options,
        AccessControlOptions, PasswordOptions,
    };
    use security_framework_sys::base::errSecItemNotFound;
    use security_framework_sys::item;
    use security_framework_sys::keychain_item::SecItemCopyMatching;

    const SERVICE: &str = "com.replghost.epoca.wallet";
    const ACCOUNT: &str = "master-mnemonic";

    /// Check whether a mnemonic exists in the Keychain without reading the
    /// secret data. Uses `SecItemCopyMatching` with `kSecReturnAttributes`
    /// only — this avoids triggering the macOS authentication prompt.
    pub fn has_mnemonic() -> bool {
        unsafe {
            let k_class = CFString::wrap_under_get_rule(item::kSecClass);
            let v_generic = CFType::wrap_under_get_rule(item::kSecClassGenericPassword as _);
            let k_service = CFString::wrap_under_get_rule(item::kSecAttrService);
            let v_service = CFString::new(SERVICE);
            let k_account = CFString::wrap_under_get_rule(item::kSecAttrAccount);
            let v_account = CFString::new(ACCOUNT);
            let k_return = CFString::wrap_under_get_rule(item::kSecReturnAttributes);
            let v_true = CFBoolean::true_value();
            let k_dpk = CFString::wrap_under_get_rule(item::kSecUseDataProtectionKeychain);
            let query = core_foundation::dictionary::CFDictionary::from_CFType_pairs(&[
                (k_class, v_generic),
                (k_service, v_service.as_CFType()),
                (k_account, v_account.as_CFType()),
                (k_return, v_true.as_CFType()),
                (k_dpk, v_true.as_CFType()),
            ]);
            let mut result = std::ptr::null();
            let status = SecItemCopyMatching(query.as_concrete_TypeRef(), &mut result);
            if !result.is_null() {
                core_foundation::base::CFRelease(result);
            }
            if status == 0 {
                return true;
            }
        }
        // Also check legacy keychain (pre-biometric items)
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
            status == 0
        }
    }

    /// Build a `PasswordOptions` for the Data Protection Keychain with
    /// biometric access control (Touch ID + passcode fallback).
    fn biometric_options() -> PasswordOptions {
        let mut opts = PasswordOptions::new_generic_password(SERVICE, ACCOUNT);
        opts.use_protected_keychain();
        let acl = SecAccessControl::create_with_protection(
            Some(ProtectionMode::AccessibleWhenPasscodeSetThisDeviceOnly),
            (AccessControlOptions::BIOMETRY_ANY | AccessControlOptions::DEVICE_PASSCODE | AccessControlOptions::OR).bits(),
        )
        .expect("Failed to create SecAccessControl");
        opts.set_access_control(acl);
        opts
    }

    /// Store the mnemonic in the Keychain with biometric access control.
    /// Deletes any existing entry first (both legacy and Data Protection).
    pub fn store_mnemonic(mnemonic: &str) -> Result<()> {
        // Delete legacy entry if present
        let _ = delete_generic_password(SERVICE, ACCOUNT);
        // Delete Data Protection entry if present
        delete_biometric_entry();
        set_generic_password_options(mnemonic.as_bytes(), biometric_options())
            .map_err(|e| anyhow!("Keychain store failed: {e}"))
    }

    /// Load the mnemonic from the Keychain.
    /// Triggers Touch ID / passcode prompt via the biometric access control.
    ///
    /// If a legacy (non-biometric) item is found, it is automatically migrated
    /// to the Data Protection Keychain with biometric access control.
    pub fn load_mnemonic() -> Result<String> {
        // Try Data Protection Keychain first (biometric item)
        let mut opts = PasswordOptions::new_generic_password(SERVICE, ACCOUNT);
        opts.use_protected_keychain();
        match generic_password(opts) {
            Ok(bytes) => {
                return String::from_utf8(bytes)
                    .map_err(|e| anyhow!("Mnemonic not valid UTF-8: {e}"));
            }
            Err(e) => {
                // Only fall through to legacy if the biometric item doesn't exist.
                // If the user cancelled Touch ID or auth failed, propagate the error.
                if e.code() != errSecItemNotFound {
                    return Err(anyhow!("Keychain authentication failed: {e}"));
                }
            }
        }
        // Fallback: try legacy keychain (pre-biometric item).
        let bytes = security_framework::passwords::get_generic_password(SERVICE, ACCOUNT)
            .map_err(|e| anyhow!("Keychain load failed: {e}"))?;
        let phrase = String::from_utf8(bytes)
            .map_err(|e| anyhow!("Mnemonic not valid UTF-8: {e}"))?;

        // Migrate: re-store with biometric access control, delete legacy item.
        if store_mnemonic(&phrase).is_ok() {
            log::info!("Migrated wallet mnemonic to biometric-protected Keychain");
        }
        Ok(phrase)
    }

    /// Delete the mnemonic from both legacy and Data Protection Keychains.
    pub fn delete_mnemonic() -> Result<()> {
        let _ = delete_generic_password(SERVICE, ACCOUNT);
        delete_biometric_entry();
        Ok(())
    }

    /// Delete the biometric-protected entry from the Data Protection Keychain.
    fn delete_biometric_entry() {
        use core_foundation::dictionary::CFDictionary;
        use security_framework_sys::keychain_item::SecItemDelete;
        unsafe {
            let k_class = CFString::wrap_under_get_rule(item::kSecClass);
            let v_generic = CFType::wrap_under_get_rule(item::kSecClassGenericPassword as _);
            let k_service = CFString::wrap_under_get_rule(item::kSecAttrService);
            let v_service = CFString::new(SERVICE);
            let k_account = CFString::wrap_under_get_rule(item::kSecAttrAccount);
            let v_account = CFString::new(ACCOUNT);
            let k_dpk = CFString::wrap_under_get_rule(item::kSecUseDataProtectionKeychain);
            let v_true = CFBoolean::true_value();
            let query = CFDictionary::from_CFType_pairs(&[
                (k_class, v_generic),
                (k_service, v_service.as_CFType()),
                (k_account, v_account.as_CFType()),
                (k_dpk, v_true.as_CFType()),
            ]);
            SecItemDelete(query.as_concrete_TypeRef());
        }
    }
}

#[cfg(target_os = "macos")]
pub use macos::*;

// Fallback for non-macOS platforms: not yet implemented.
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
