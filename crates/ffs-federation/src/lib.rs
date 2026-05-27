//! `ffs-federation` — mTLS HTTPS transport for FFS-to-FFS bridges.
//!
//! Per ADR-020, federation runs over HTTPS with mutual TLS, pull-
//! based. Identity is anchored in each substrate's Ed25519 signing
//! key: the TLS certificate is generated from that key (rcgen),
//! peers exchange BLAKE3 fingerprints out-of-band, and the in-band
//! handshake exchanges capability atom hashes plus the supported
//! predicate vocabulary.
//!
//! Module map:
//!
//! - `cert`: Ed25519 → X.509 certificate generation.
//! - `handshake`: bridge state machine + wire types.
//! - `client`: `FederationClient` trait + `InMemoryFederationClient`
//!   for tests; the production reqwest+rustls binding is wired by
//!   task_22's onboarding scripts (deferred per TechSpec § Unit
//!   Tests, which calls for trait-mocked transport in unit tests).
//! - `server`: pure endpoint handler functions; the axum binding
//!   that calls them lives in the daemon binary (deferred).

pub mod cert;
pub mod client;
pub mod handshake;
pub mod server;

pub use cert::{CertError, SubstrateCertificate, fingerprint_der, generate_from_signing_key};
pub use client::{FederationClient, FederationClientError, InMemoryFederationClient};
pub use handshake::{
    HandshakeError, HandshakeRequest, HandshakeResponse, RotateRequest, RotateResponse,
};
pub use server::{FederationContext, ServerError, handle_handshake, handle_rotate};

/// Workspace marker exposed so smoke tests can confirm the crate links.
pub const CRATE_NAME: &str = "ffs-federation";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke() {
        assert_eq!(CRATE_NAME, "ffs-federation");
        assert_eq!(ffs_core::CRATE_NAME, "ffs-core");
    }
}
