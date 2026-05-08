//! Thin wrapper around the `multibase` crate that fixes FFS to base58btc
//! ('z' prefix) for keys, signatures, and multihashes per ADR-017 / ADR-018.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MultibaseError {
    #[error("multibase decode error: {0}")]
    Decode(String),
    #[error("expected base58btc encoding (prefix 'z'), got prefix {0:?}")]
    UnexpectedBase(char),
    #[error("empty multibase string")]
    Empty,
}

/// Encode bytes as a base58btc multibase string (prefix `z`).
pub fn encode_base58btc(bytes: &[u8]) -> String {
    multibase::encode(multibase::Base::Base58Btc, bytes)
}

/// Decode a base58btc multibase string. Rejects any other base.
pub fn decode_base58btc(s: &str) -> Result<Vec<u8>, MultibaseError> {
    if s.is_empty() {
        return Err(MultibaseError::Empty);
    }
    let prefix = s.chars().next().expect("non-empty");
    if prefix != 'z' {
        return Err(MultibaseError::UnexpectedBase(prefix));
    }
    let (_, bytes) = multibase::decode(s).map_err(|e| MultibaseError::Decode(e.to_string()))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let payload = b"hello, ffs";
        let encoded = encode_base58btc(payload);
        assert!(
            encoded.starts_with('z'),
            "expected 'z' prefix, got {encoded}"
        );
        let decoded = decode_base58btc(&encoded).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn empty_string_rejected() {
        let err = decode_base58btc("").unwrap_err();
        assert!(matches!(err, MultibaseError::Empty));
    }

    #[test]
    fn non_base58btc_rejected() {
        // 'f' is base16 in multibase.
        let result = decode_base58btc("f68656c6c6f");
        assert!(matches!(result, Err(MultibaseError::UnexpectedBase('f'))));
    }

    #[test]
    fn empty_payload_roundtrips() {
        let encoded = encode_base58btc(&[]);
        assert!(encoded.starts_with('z'));
        let decoded = decode_base58btc(&encoded).unwrap();
        assert_eq!(decoded, &[] as &[u8]);
    }
}
