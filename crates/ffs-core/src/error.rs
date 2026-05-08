//! Typed errors for atom signing, verification, and timestamp validation.
//!
//! `VerifyError` distinguishes the three failure modes a verifier needs to
//! report distinctly: a signature failure (key/sig mismatch or tampering),
//! a content-hash mismatch (envelope changed since hash was recorded), and
//! a malformed envelope (parse, encoding, or timestamp issue).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum VerifyError {
    #[error("signature does not match author public key")]
    Signature,
    #[error("content hash does not match envelope")]
    HashMismatch,
    #[error("envelope is malformed: {0}")]
    Malformed(String),
}

#[derive(Debug, Error)]
pub enum SignError {
    #[error("envelope serialization failed: {0}")]
    Serialization(String),
    #[error("invalid timestamp in template: {0}")]
    BadTimestamp(#[from] BadTimestampError),
}

#[derive(Debug, Error)]
pub enum BadTimestampError {
    #[error("timestamp parse error: {0}")]
    Parse(String),
    #[error("timestamp is not UTC")]
    NonUtc,
}
