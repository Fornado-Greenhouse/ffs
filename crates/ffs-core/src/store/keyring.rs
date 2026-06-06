//! OS-keychain integration for FFS's two long-lived secrets:
//!
//! - The **SQLCipher DEK** (32 bytes) that encrypts `atoms.db`.
//! - The **owner signing-key seed** (32 bytes) that's expanded into
//!   the daemon's Ed25519 identity.
//!
//! Both helpers wrap the cross-platform `keyring` crate (macOS
//! Keychain, Linux Secret Service, Windows Credential Manager).
//! On first call they generate 32 random bytes via `OsRng`, base64-
//! encode them, persist to the OS keychain under the supplied
//! `(service, account)` pair, and return the raw bytes. On
//! subsequent calls they read the existing entry and decode it.
//!
//! Pure helpers (`encode_key`, `decode_key`) sit underneath both
//! `dek_from_keyring` and `owner_key_from_keyring` and carry the
//! validation logic — these are the unit-testable surface.

use ::keyring::{Entry, Error as KeyringError};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;

use super::StoreError;

/// Service name for the owner signing-key seed entry.
pub const OWNER_KEY_SERVICE: &str = "ffs-owner-key";
/// Service name for the SQLCipher DEK entry.
pub const DEK_SERVICE: &str = "ffs-dek";

/// Look up or create the substrate's SQLCipher DEK in the OS
/// keychain. The returned 32 bytes are passed to
/// [`super::SqliteAtomStore::open_with_key`].
///
/// `account` is a per-substrate identifier such as the substrate's
/// owner public-key in multibase form.
pub fn dek_from_keyring(service: &str, account: &str) -> Result<[u8; 32], StoreError> {
    let entry = Entry::new(service, account).map_err(|e| StoreError::Keyring(e.to_string()))?;
    key_from_entry_or_generate(&entry)
}

/// Look up or create the substrate's owner Ed25519 signing-key seed
/// in the OS keychain. The returned 32 bytes are used as the seed
/// for [`ed25519_dalek::SigningKey::from_bytes`].
///
/// `account` is typically the OS username — there's one signing
/// identity per substrate, and substrates are one-per-user at MVP.
/// Multi-substrate-per-user (cohorts) can override the account
/// when the cohort design lands.
pub fn owner_key_from_keyring(service: &str, account: &str) -> Result<[u8; 32], StoreError> {
    let entry = Entry::new(service, account).map_err(|e| StoreError::Keyring(e.to_string()))?;
    key_from_entry_or_generate(&entry)
}

/// Persist a 32-byte key into the OS keychain under `(service,
/// account)`, replacing any existing entry. Used by the daemon's
/// env-var → keychain migration path so a user who boots with
/// `FFS_OWNER_KEY_HEX` set finds the value already in the keychain
/// on the next boot and can drop the env var.
pub fn save_key_to_keychain(
    service: &str,
    account: &str,
    key: &[u8; 32],
) -> Result<(), StoreError> {
    let entry = Entry::new(service, account).map_err(|e| StoreError::Keyring(e.to_string()))?;
    entry
        .set_password(&encode_key(key))
        .map_err(|e| StoreError::Keyring(e.to_string()))
}

/// Shared lookup-or-generate logic for both keys. On `NoEntry`
/// generates 32 fresh bytes from `OsRng`, base64-encodes them,
/// persists, and returns. On a hit decodes and length-validates.
fn key_from_entry_or_generate(entry: &Entry) -> Result<[u8; 32], StoreError> {
    match entry.get_password() {
        Ok(s) => decode_key(&s).map_err(StoreError::Keyring),
        Err(KeyringError::NoEntry) => {
            let key = generate_fresh_key();
            entry
                .set_password(&encode_key(&key))
                .map_err(|e| StoreError::Keyring(e.to_string()))?;
            Ok(key)
        }
        Err(e) => Err(StoreError::Keyring(e.to_string())),
    }
}

/// Base64-encode 32 bytes into the form persisted in the keychain.
pub(crate) fn encode_key(key: &[u8; 32]) -> String {
    B64.encode(key)
}

/// Decode a base64 string into 32 bytes. Returns a descriptive
/// error string when the input isn't valid base64 OR when the
/// decoded payload has the wrong length (corruption guard against
/// a tampered keychain entry).
pub(crate) fn decode_key(s: &str) -> Result<[u8; 32], String> {
    let bytes = B64
        .decode(s.trim())
        .map_err(|e| format!("base64 decode failed: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!(
            "key in keyring has wrong length: {} (expected 32)",
            bytes.len()
        ));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn generate_fresh_key() -> [u8; 32] {
    use rand::RngCore;
    let mut key = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut key);
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_round_trips_for_32_bytes() {
        let key = [0xAAu8; 32];
        let encoded = encode_key(&key);
        let decoded = decode_key(&encoded).expect("round-trip ok");
        assert_eq!(decoded, key);
    }

    #[test]
    fn decode_tolerates_surrounding_whitespace() {
        let key = [0x42u8; 32];
        let encoded = format!("  {}\n", encode_key(&key));
        let decoded = decode_key(&encoded).expect("whitespace tolerated");
        assert_eq!(decoded, key);
    }

    #[test]
    fn decode_rejects_wrong_byte_length() {
        // 16 bytes base64-encoded.
        let short = B64.encode([0u8; 16]);
        let err = decode_key(&short).expect_err("short keys must fail");
        assert!(err.contains("wrong length"), "error should explain: {err}");
    }

    #[test]
    fn decode_rejects_non_base64() {
        let err = decode_key("!!! not base64 !!!").expect_err("garbage must fail");
        assert!(err.contains("base64"), "error should name the codec: {err}");
    }

    #[test]
    fn generate_fresh_key_produces_distinct_keys_across_calls() {
        // OsRng should not produce duplicates within a single process.
        let a = generate_fresh_key();
        let b = generate_fresh_key();
        assert_ne!(a, b, "two fresh keys should differ");
    }

    #[test]
    fn service_constants_match_documented_values() {
        assert_eq!(OWNER_KEY_SERVICE, "ffs-owner-key");
        assert_eq!(DEK_SERVICE, "ffs-dek");
    }
}
