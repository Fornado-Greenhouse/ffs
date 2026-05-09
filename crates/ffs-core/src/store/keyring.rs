//! OS-keychain integration for SQLCipher data-encryption keys.
//!
//! Generates a 32-byte DEK on first use, persists it to the platform
//! keychain (macOS Keychain, Windows Credential Store, Linux Secret
//! Service), and returns it on subsequent calls. Encoding is base64
//! since the `keyring` crate stores strings.

use ::keyring::{Entry, Error as KeyringError};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;

use super::StoreError;

/// Look up or create the substrate's DEK in the OS keychain. The returned
/// 32 bytes are passed to [`super::SqliteAtomStore::open_with_key`].
///
/// `service` is typically `"ffs-dek"`; `account` is a per-substrate
/// identifier such as the cohort name or the substrate's identity public
/// key in multibase form.
pub fn dek_from_keyring(service: &str, account: &str) -> Result<[u8; 32], StoreError> {
    let entry = Entry::new(service, account).map_err(|e| StoreError::Keyring(e.to_string()))?;
    match entry.get_password() {
        Ok(s) => {
            let bytes = B64
                .decode(s)
                .map_err(|e| StoreError::Keyring(e.to_string()))?;
            if bytes.len() != 32 {
                return Err(StoreError::Keyring(format!(
                    "DEK in keyring has wrong length: {} (expected 32)",
                    bytes.len()
                )));
            }
            let mut out = [0u8; 32];
            out.copy_from_slice(&bytes);
            Ok(out)
        }
        Err(KeyringError::NoEntry) => {
            // First use — generate a fresh DEK and persist it.
            let mut key = [0u8; 32];
            use rand::RngCore;
            rand::rngs::OsRng.fill_bytes(&mut key);
            entry
                .set_password(&B64.encode(key))
                .map_err(|e| StoreError::Keyring(e.to_string()))?;
            Ok(key)
        }
        Err(e) => Err(StoreError::Keyring(e.to_string())),
    }
}
