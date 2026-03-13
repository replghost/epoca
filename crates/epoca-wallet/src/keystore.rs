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
        delete_generic_password, generic_password, set_generic_password,
        set_generic_password_options, AccessControlOptions, PasswordOptions,
    };
    use security_framework_sys::base::errSecItemNotFound;
    use security_framework_sys::item;
    use security_framework_sys::keychain_item::SecItemCopyMatching;
    use hex;

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
    /// Falls back to the legacy keychain if the Data Protection Keychain
    /// is unavailable (e.g. ad-hoc signed binary without entitlements).
    pub fn store_mnemonic(mnemonic: &str) -> Result<()> {
        // Delete legacy entry if present
        let _ = delete_generic_password(SERVICE, ACCOUNT);
        // Delete Data Protection entry if present
        delete_biometric_entry();
        match set_generic_password_options(mnemonic.as_bytes(), biometric_options()) {
            Ok(()) => Ok(()),
            Err(e) => {
                log::warn!("Data Protection Keychain unavailable ({e}), falling back to legacy keychain");
                set_generic_password(SERVICE, ACCOUNT, mnemonic.as_bytes())
                    .map_err(|e| anyhow!("Keychain store failed: {e}"))
            }
        }
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
        delete_biometric_entry_for(ACCOUNT);
    }

    /// Delete the biometric-protected entry for the given account from the
    /// Data Protection Keychain. Shared by `delete_mnemonic` and
    /// `delete_paired_data`.
    fn delete_biometric_entry_for(account: &str) {
        use core_foundation::dictionary::CFDictionary;
        use security_framework_sys::keychain_item::SecItemDelete;
        unsafe {
            let k_class = CFString::wrap_under_get_rule(item::kSecClass);
            let v_generic = CFType::wrap_under_get_rule(item::kSecClassGenericPassword as _);
            let k_service = CFString::wrap_under_get_rule(item::kSecAttrService);
            let v_service = CFString::new(SERVICE);
            let k_account = CFString::wrap_under_get_rule(item::kSecAttrAccount);
            let v_account = CFString::new(account);
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

    /// Build a `PasswordOptions` for the Data Protection Keychain with
    /// biometric access control, using the given account name.
    fn biometric_options_for(account: &str) -> PasswordOptions {
        let mut opts = PasswordOptions::new_generic_password(SERVICE, account);
        opts.use_protected_keychain();
        let acl = SecAccessControl::create_with_protection(
            Some(ProtectionMode::AccessibleWhenPasscodeSetThisDeviceOnly),
            (AccessControlOptions::BIOMETRY_ANY | AccessControlOptions::DEVICE_PASSCODE | AccessControlOptions::OR).bits(),
        )
        .expect("Failed to create SecAccessControl");
        opts.set_access_control(acl);
        opts
    }

    // ── Paired wallet accounts ────────────────────────────────────────────────

    const ACCOUNT_PAIRED_ADDRESS: &str = "paired-address";
    const ACCOUNT_PAIRED_SESSION_KEY: &str = "paired-session-key";
    const ACCOUNT_PAIRED_RENDEZVOUS: &str = "paired-rendezvous";

    /// Store paired wallet data in the Data Protection Keychain with biometric
    /// access control. Each of the three values is stored under its own account
    /// name. The 32-byte arrays are hex-encoded before storage.
    pub fn store_paired_data(address: &str, session_key: &[u8; 32], rendezvous: &[u8; 32]) -> Result<()> {
        // Clear any existing entries first.
        delete_paired_data();

        let session_key_hex = hex::encode(session_key);
        let rendezvous_hex = hex::encode(rendezvous);

        set_generic_password_options(address.as_bytes(), biometric_options_for(ACCOUNT_PAIRED_ADDRESS))
            .map_err(|e| anyhow!("Keychain store failed for paired-address: {e}"))?;
        set_generic_password_options(session_key_hex.as_bytes(), biometric_options_for(ACCOUNT_PAIRED_SESSION_KEY))
            .map_err(|e| anyhow!("Keychain store failed for paired-session-key: {e}"))?;
        set_generic_password_options(rendezvous_hex.as_bytes(), biometric_options_for(ACCOUNT_PAIRED_RENDEZVOUS))
            .map_err(|e| anyhow!("Keychain store failed for paired-rendezvous: {e}"))?;

        Ok(())
    }

    /// Load paired wallet data from the Data Protection Keychain.
    /// Triggers Touch ID / passcode prompt via the biometric access control.
    /// Returns `None` if no paired wallet is stored.
    pub fn load_paired_data() -> Option<(String, [u8; 32], [u8; 32])> {
        let mut opts = PasswordOptions::new_generic_password(SERVICE, ACCOUNT_PAIRED_ADDRESS);
        opts.use_protected_keychain();
        let address_bytes = match generic_password(opts) {
            Ok(b) => b,
            Err(e) => {
                if e.code() == errSecItemNotFound {
                    return None;
                }
                log::warn!("Keychain auth failed for paired-address: {e}");
                return None;
            }
        };
        let address = match String::from_utf8(address_bytes) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("paired-address is not valid UTF-8: {e}");
                return None;
            }
        };

        let mut opts = PasswordOptions::new_generic_password(SERVICE, ACCOUNT_PAIRED_SESSION_KEY);
        opts.use_protected_keychain();
        let session_key_hex = match generic_password(opts) {
            Ok(b) => String::from_utf8(b).ok()?,
            Err(e) => {
                log::warn!("Keychain load failed for paired-session-key: {e}");
                return None;
            }
        };

        let mut opts = PasswordOptions::new_generic_password(SERVICE, ACCOUNT_PAIRED_RENDEZVOUS);
        opts.use_protected_keychain();
        let rendezvous_hex = match generic_password(opts) {
            Ok(b) => String::from_utf8(b).ok()?,
            Err(e) => {
                log::warn!("Keychain load failed for paired-rendezvous: {e}");
                return None;
            }
        };

        let session_key = decode_hex32(&session_key_hex)?;
        let rendezvous = decode_hex32(&rendezvous_hex)?;

        Some((address, session_key, rendezvous))
    }

    /// Check whether a paired wallet exists in the Data Protection Keychain
    /// without reading the secret data (no biometric prompt).
    pub fn has_paired_wallet() -> bool {
        unsafe {
            let k_class = CFString::wrap_under_get_rule(item::kSecClass);
            let v_generic = CFType::wrap_under_get_rule(item::kSecClassGenericPassword as _);
            let k_service = CFString::wrap_under_get_rule(item::kSecAttrService);
            let v_service = CFString::new(SERVICE);
            let k_account = CFString::wrap_under_get_rule(item::kSecAttrAccount);
            let v_account = CFString::new(ACCOUNT_PAIRED_ADDRESS);
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
            status == 0
        }
    }

    /// Delete all three paired wallet entries from the Data Protection Keychain.
    pub fn delete_paired_data() {
        delete_biometric_entry_for(ACCOUNT_PAIRED_ADDRESS);
        delete_biometric_entry_for(ACCOUNT_PAIRED_SESSION_KEY);
        delete_biometric_entry_for(ACCOUNT_PAIRED_RENDEZVOUS);
    }

    /// Decode a 64-character hex string into a 32-byte array.
    fn decode_hex32(hex_str: &str) -> Option<[u8; 32]> {
        let bytes = hex::decode(hex_str).ok()?;
        let arr: [u8; 32] = bytes.try_into().ok()?;
        Some(arr)
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

#[cfg(not(target_os = "macos"))]
pub fn store_paired_data(_address: &str, _session_key: &[u8; 32], _rendezvous: &[u8; 32]) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("Wallet storage not yet implemented on this platform"))
}

#[cfg(not(target_os = "macos"))]
pub fn load_paired_data() -> Option<(String, [u8; 32], [u8; 32])> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn has_paired_wallet() -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
pub fn delete_paired_data() {}
