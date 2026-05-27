//! Ed25519-derived TLS certificate generation.
//!
//! Per ADR-020, each substrate's federation endpoint presents a self-
//! signed X.509 certificate whose Ed25519 key matches the substrate's
//! signing key. The subject CN is the multibase-encoded public key
//! so peers see a stable identity in the cert. Trust is established
//! by certificate-fingerprint pinning during the bridge handshake;
//! the fingerprint is BLAKE3-of-DER.
//!
//! TLS 1.3 supports Ed25519 certificate signatures directly, so the
//! same key acts as both the FFS-layer atom signer and the TLS-layer
//! peer identity. rcgen builds the X.509; ed25519-dalek owns the key.

use ed25519_dalek::SigningKey;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ED25519, SerialNumber};
use rustls_pki_types::PrivatePkcs8KeyDer;
use time::OffsetDateTime;

use ffs_core::{Multihash, PublicKey};

#[derive(Debug, thiserror::Error)]
pub enum CertError {
    #[error("rcgen: {0}")]
    Rcgen(#[from] rcgen::Error),
    #[error("key conversion: {0}")]
    Key(String),
}

/// Output of certificate generation: the DER-encoded certificate
/// bytes (for distribution / TLS handshake), the PEM-encoded form
/// (for `~/.ffs/run/cert.pem` persistence), and the BLAKE3
/// fingerprint of the DER bytes (the value peers pin).
pub struct SubstrateCertificate {
    pub der: Vec<u8>,
    pub pem: String,
    pub fingerprint: Multihash,
}

/// Generate a self-signed X.509 certificate from the substrate's
/// Ed25519 signing key.
///
/// The cert's subject CN is the multibase-encoded public key — a
/// stable identity that the peer can verify matches the
/// FFS-layer author key on any atom they receive.
///
/// Validity is 10 years from the supplied `now`. Rotation happens
/// via `bridge.rotate` long before expiry, so the long lifetime
/// is a convenience that avoids forcing re-issuance for active
/// deployments. Production wiring can override `now` for
/// reproducible cert generation; tests fix it for stability.
pub fn generate_from_signing_key(
    key: &SigningKey,
    now: OffsetDateTime,
) -> Result<SubstrateCertificate, CertError> {
    let pubkey = PublicKey::from_verifying(&key.verifying_key());
    let cn = pubkey.to_multibase();

    // rcgen accepts a pre-existing keypair via PKCS#8. Encode our
    // Ed25519 signing key as a PKCS#8 v2 document and hand it over;
    // rcgen will reuse it for the cert's subject public key + signature.
    let pkcs8 = ed25519_pkcs8_v2(key);
    let pkcs8_der = PrivatePkcs8KeyDer::from(pkcs8);
    let keypair = KeyPair::from_pkcs8_der_and_sign_algo(&pkcs8_der, &PKCS_ED25519)
        .map_err(|e| CertError::Key(e.to_string()))?;

    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, &cn);
    params.distinguished_name = dn;
    // Deterministic-ish serial: hash of the public key, truncated.
    // Avoids randomness so the same key always produces the same
    // serial on regeneration (helps reproducibility in tests).
    let serial_hash = Multihash::blake3_of(pubkey.as_bytes());
    let serial_bytes = serial_hash.digest()[..16].to_vec();
    params.serial_number = Some(SerialNumber::from(serial_bytes));
    params.not_before = now;
    params.not_after = now + time::Duration::days(10 * 365);

    let cert = params.self_signed(&keypair)?;
    let der = cert.der().to_vec();
    let pem = cert.pem();
    let fingerprint = Multihash::blake3_of(&der);
    Ok(SubstrateCertificate {
        der,
        pem,
        fingerprint,
    })
}

/// Compute the BLAKE3 fingerprint of an arbitrary DER-encoded
/// certificate. Used on inbound mTLS to look the cert up in the
/// pinned-fingerprint set.
pub fn fingerprint_der(der: &[u8]) -> Multihash {
    Multihash::blake3_of(der)
}

/// Encode an ed25519-dalek `SigningKey` as PKCS#8 v2 DER.
///
/// `ed25519-dalek` v2 exposes `to_pkcs8_der` via the `pkcs8` feature
/// only. To avoid adding the feature flag (and an extra dep), build
/// the small fixed envelope ourselves: PKCS#8 v2 for Ed25519 has a
/// fixed prefix (OneAsymmetricKey: version, AlgorithmIdentifier, the
/// CurvePrivateKey octet string, then the 32 private + 32 public
/// bytes). RFC 8410 § 7 gives the exact structure.
fn ed25519_pkcs8_v2(key: &SigningKey) -> Vec<u8> {
    // PKCS#8 v2 prefix for an Ed25519 key, ending right before the
    // 32-byte seed. Source: RFC 8410 § 10.3.
    //
    // SEQUENCE (PrivateKeyInfo / OneAsymmetricKey, version=1)
    //   INTEGER 1                                  -- v2
    //   SEQUENCE (AlgorithmIdentifier)
    //     OID 1.3.101.112                         -- id-Ed25519
    //   OCTET STRING (CurvePrivateKey, 34 bytes:
    //     04 20 <32 seed bytes>)
    //   [1] IMPLICIT OCTET STRING (PublicKey, 32 bytes)
    let seed = key.to_bytes();
    let public = key.verifying_key().to_bytes();
    let mut out = Vec::with_capacity(85);
    // SEQUENCE, length 83 = 5 + 7 + 34 + 35
    out.extend_from_slice(&[0x30, 0x53]);
    // INTEGER 1
    out.extend_from_slice(&[0x02, 0x01, 0x01]);
    // SEQUENCE (AlgorithmIdentifier), length 5
    //   OID 1.3.101.112
    out.extend_from_slice(&[0x30, 0x05, 0x06, 0x03, 0x2B, 0x65, 0x70]);
    // OCTET STRING (CurvePrivateKey: OCTET STRING wrapper around seed)
    out.extend_from_slice(&[0x04, 0x22, 0x04, 0x20]);
    out.extend_from_slice(&seed);
    // [1] IMPLICIT OCTET STRING (PublicKey), 33 bytes payload?
    // RFC 8410 says context-tag [1] with the raw public key bytes.
    out.extend_from_slice(&[0xA1, 0x23, 0x03, 0x21, 0x00]);
    out.extend_from_slice(&public);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_key() -> SigningKey {
        SigningKey::from_bytes(&[42u8; 32])
    }

    fn fixed_now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_780_000_000).unwrap()
    }

    #[test]
    fn cert_subject_cn_matches_multibase_public_key() {
        let key = fixed_key();
        let cert = generate_from_signing_key(&key, fixed_now()).unwrap();
        // The expected CN is the multibase-encoded public key.
        let expected_cn = PublicKey::from_verifying(&key.verifying_key()).to_multibase();
        // PEM should round-trip; the body contains a BEGIN
        // CERTIFICATE banner.
        assert!(
            cert.pem.contains("BEGIN CERTIFICATE"),
            "pem looks well-formed"
        );
        // The multibase CN string is ASCII; X.509 PrintableString /
        // UTF8String encodings contain the bytes verbatim, so a
        // window-search over the DER finds the CN body. Avoids
        // pulling in a full X.509 parser just for the assertion.
        let cn_bytes = expected_cn.as_bytes();
        assert!(
            cert.der.windows(cn_bytes.len()).any(|w| w == cn_bytes),
            "DER should contain the multibase CN bytes ({expected_cn})"
        );
    }

    #[test]
    fn cert_generation_is_deterministic_for_a_fixed_key_and_time() {
        let key = fixed_key();
        let now = fixed_now();
        let a = generate_from_signing_key(&key, now).unwrap();
        let b = generate_from_signing_key(&key, now).unwrap();
        assert_eq!(a.der, b.der, "cert DER should be stable across runs");
        assert_eq!(a.fingerprint, b.fingerprint);
    }

    #[test]
    fn fingerprint_der_matches_generate_output() {
        let key = fixed_key();
        let cert = generate_from_signing_key(&key, fixed_now()).unwrap();
        assert_eq!(cert.fingerprint, fingerprint_der(&cert.der));
    }

    #[test]
    fn different_keys_produce_different_fingerprints() {
        let a =
            generate_from_signing_key(&SigningKey::from_bytes(&[1u8; 32]), fixed_now()).unwrap();
        let b =
            generate_from_signing_key(&SigningKey::from_bytes(&[2u8; 32]), fixed_now()).unwrap();
        assert_ne!(a.fingerprint, b.fingerprint);
    }
}
