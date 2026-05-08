//! Multihash content addressing. FFS uses the BLAKE3 codec (0x1e) with a
//! 32-byte digest, wrapped in the standard multihash framing
//! `varint(codec) || varint(length) || digest`. Both 0x1e and 32 fit in a
//! single byte so the multihash is exactly 34 bytes.
//!
//! The on-the-wire form is the base58btc multibase string of those 34
//! bytes (per ADR-017).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::multibase::{MultibaseError, decode_base58btc, encode_base58btc};

const BLAKE3_CODEC: u8 = 0x1e;
const DIGEST_LEN: u8 = 32;
const MULTIHASH_LEN: usize = 2 + DIGEST_LEN as usize;

#[derive(Debug, Error)]
pub enum MultihashError {
    #[error("multibase: {0}")]
    Multibase(#[from] MultibaseError),
    #[error("expected BLAKE3 codec (0x1e), got 0x{0:02x}")]
    UnexpectedCodec(u8),
    #[error("expected digest length 32, got {0}")]
    BadLength(u8),
    #[error("multihash has unexpected total length {0} (want {MULTIHASH_LEN})")]
    BadTotalLength(usize),
}

/// A BLAKE3-32 multihash. Internally stored as the 34-byte
/// `varint(codec) || varint(length) || digest` form.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Multihash([u8; MULTIHASH_LEN]);

impl Multihash {
    /// Wrap a raw 32-byte BLAKE3 digest as a multihash.
    pub fn from_blake3(digest: &[u8; 32]) -> Self {
        let mut bytes = [0u8; MULTIHASH_LEN];
        bytes[0] = BLAKE3_CODEC;
        bytes[1] = DIGEST_LEN;
        bytes[2..].copy_from_slice(digest);
        Self(bytes)
    }

    /// Compute the BLAKE3 multihash of `data`.
    pub fn blake3_of(data: &[u8]) -> Self {
        let digest = blake3::hash(data);
        Self::from_blake3(digest.as_bytes())
    }

    /// Build a multihash from raw multihash bytes. Validates the BLAKE3
    /// codec and 32-byte digest length per FFS conventions.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, MultihashError> {
        if bytes.len() != MULTIHASH_LEN {
            return Err(MultihashError::BadTotalLength(bytes.len()));
        }
        if bytes[0] != BLAKE3_CODEC {
            return Err(MultihashError::UnexpectedCodec(bytes[0]));
        }
        if bytes[1] != DIGEST_LEN {
            return Err(MultihashError::BadLength(bytes[1]));
        }
        let mut arr = [0u8; MULTIHASH_LEN];
        arr.copy_from_slice(bytes);
        Ok(Self(arr))
    }

    /// The full 34-byte multihash (codec + length + digest).
    pub fn as_bytes(&self) -> &[u8; MULTIHASH_LEN] {
        &self.0
    }

    /// The 32-byte BLAKE3 digest portion.
    pub fn digest(&self) -> &[u8] {
        &self.0[2..]
    }

    pub fn to_multibase(&self) -> String {
        encode_base58btc(&self.0)
    }

    pub fn from_multibase(s: &str) -> Result<Self, MultihashError> {
        let bytes = decode_base58btc(s)?;
        Self::from_bytes(&bytes)
    }
}

impl Serialize for Multihash {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&self.to_multibase())
    }
}

impl<'de> Deserialize<'de> for Multihash {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let s = String::deserialize(de)?;
        Self::from_multibase(&s).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blake3_of_known_input() {
        let mh = Multihash::blake3_of(b"hello");
        assert_eq!(mh.as_bytes()[0], BLAKE3_CODEC);
        assert_eq!(mh.as_bytes()[1], DIGEST_LEN);
        assert_eq!(mh.digest().len(), 32);
        // Stable across calls.
        let mh2 = Multihash::blake3_of(b"hello");
        assert_eq!(mh, mh2);
    }

    #[test]
    fn multibase_roundtrip() {
        let mh = Multihash::blake3_of(b"sample");
        let s = mh.to_multibase();
        assert!(s.starts_with('z'));
        let back = Multihash::from_multibase(&s).unwrap();
        assert_eq!(mh, back);
    }

    #[test]
    fn from_bytes_rejects_bad_codec() {
        let mut bad = [0u8; MULTIHASH_LEN];
        bad[0] = 0x12; // SHA-256 codec, not BLAKE3
        bad[1] = DIGEST_LEN;
        let err = Multihash::from_bytes(&bad).unwrap_err();
        assert!(matches!(err, MultihashError::UnexpectedCodec(0x12)));
    }

    #[test]
    fn from_bytes_rejects_bad_length() {
        let mut bad = [0u8; MULTIHASH_LEN];
        bad[0] = BLAKE3_CODEC;
        bad[1] = 0x10; // claims 16-byte digest
        let err = Multihash::from_bytes(&bad).unwrap_err();
        assert!(matches!(err, MultihashError::BadLength(0x10)));
    }

    #[test]
    fn from_bytes_rejects_short_input() {
        let err = Multihash::from_bytes(&[0u8; 10]).unwrap_err();
        assert!(matches!(err, MultihashError::BadTotalLength(10)));
    }

    #[test]
    fn serde_json_roundtrip() {
        let mh = Multihash::blake3_of(b"json roundtrip");
        let json = serde_json::to_string(&mh).unwrap();
        // Should be a JSON string containing the multibase encoding.
        assert!(json.starts_with("\"z"));
        let back: Multihash = serde_json::from_str(&json).unwrap();
        assert_eq!(mh, back);
    }
}
