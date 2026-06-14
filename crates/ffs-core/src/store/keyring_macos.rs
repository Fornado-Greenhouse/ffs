//! macOS-specific keychain helpers that set `kSecAttrAccessGroup`
//! explicitly. The `keyring` crate (v3, `apple-native`) calls
//! `find_generic_password` / `set_generic_password` without an
//! access-group attribute, which means Keychain Services partitions
//! entries by the writing binary's code-signing identity AND launch
//! context — so an entry the interactive CLI wrote is invisible to
//! the launchd-spawned daemon, even though they're "the same" user.
//!
//! Setting an explicit access group puts every FFS binary in one
//! logical keychain bucket. Per ADR-023, all three FFS binaries
//! (ffs, ffs-daemon, ffs-mcp) are codesigned with the
//! `entitlements/ffs.entitlements.plist` declaring
//! `keychain-access-groups = [3S9R9K2L38.com.ffs.shared]` — that's
//! the precondition for this module's writes/reads to succeed at
//! runtime.
//!
//! When the running binary lacks the entitlement (unsigned dev
//! build, missing Developer ID), the SecItem* calls fail with
//! `errSecMissingEntitlement` and we surface that as
//! [`StoreError::Keyring`] — the daemon main detects the unsigned
//! state earlier via `is_signed_with_keychain_entitlement` and
//! never reaches this module. This module is the
//! `signed-and-running` path.

use security_framework::passwords::{generic_password, set_generic_password_options};
use security_framework::passwords_options::PasswordOptions;

use super::StoreError;
use super::keyring::{decode_key, encode_key, generate_fresh_key};

/// `errSecItemNotFound` from `<Security/SecBase.h>`. The
/// `security-framework` crate doesn't re-export it and the `-sys`
/// crate isn't in our direct dep graph; hardcoding the constant
/// (with this comment locking down its source) keeps us off the
/// `-sys` dep just for one number. Apple-stable across every
/// macOS / iOS / iPadOS release.
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

/// Shared access group for every FFS binary. Mirrors the
/// `keychain-access-groups` value in
/// `entitlements/ffs.entitlements.plist`. `3S9R9K2L38` is the
/// project's Apple Developer Team ID; `com.ffs.shared` is the
/// per-project namespace.
pub const FFS_ACCESS_GROUP: &str = "3S9R9K2L38.com.ffs.shared";

/// macOS analog of [`super::keyring::dek_from_keyring`] +
/// [`super::keyring::owner_key_from_keyring`]: look up the entry,
/// create-and-persist on `NoEntry`. Both helpers in the parent
/// module funnel through here on macOS.
pub(super) fn key_from_access_group_or_generate(
    service: &str,
    account: &str,
) -> Result<[u8; 32], StoreError> {
    let mut opts = PasswordOptions::new_generic_password(service, account);
    opts.set_access_group(FFS_ACCESS_GROUP);
    match generic_password(opts) {
        Ok(bytes) => {
            // The stored payload is base64 text (mirrors the
            // keyring-crate path so the same encoding helpers
            // validate length). `from_utf8_lossy` is fine because
            // base64 is ASCII; if the payload is somehow non-UTF8
            // the decode_key call below catches it as a length
            // mismatch.
            let payload = String::from_utf8_lossy(&bytes);
            decode_key(&payload).map_err(StoreError::Keyring)
        }
        Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => {
            let key = generate_fresh_key();
            persist(service, account, &key)?;
            Ok(key)
        }
        Err(e) => Err(StoreError::Keyring(format!(
            "SecItemCopyMatching ({service},{account}) failed: {e}"
        ))),
    }
}

/// macOS analog of [`super::keyring::save_key_to_keychain`]. Sets
/// `kSecAttrAccessGroup` so the entry lands in the shared FFS
/// bucket. Replaces any existing entry under the same
/// `(service, account, access_group)` tuple.
pub(super) fn persist(service: &str, account: &str, key: &[u8; 32]) -> Result<(), StoreError> {
    let mut opts = PasswordOptions::new_generic_password(service, account);
    opts.set_access_group(FFS_ACCESS_GROUP);
    let payload = encode_key(key);
    set_generic_password_options(payload.as_bytes(), opts).map_err(|e| {
        StoreError::Keyring(format!(
            "SecItemAdd ({service},{account},group={FFS_ACCESS_GROUP}) failed: {e}"
        ))
    })
}
