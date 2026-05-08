//! mTLS HTTPS server and client for FFS-to-FFS federation.
//!
//! Bridge handshake, capability-filtered serving, and pull sync are introduced
//! by tasks 14 and 15.

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
