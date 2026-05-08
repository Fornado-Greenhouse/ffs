//! Foundational types and logic shared across every FFS binary.
//!
//! The substantive modules (atom envelope, predicate spec, store, capability,
//! projection) are introduced by subsequent tasks. This crate is the
//! single dependency point those binaries import.

/// Workspace marker exposed so smoke tests can confirm the crate links.
pub const CRATE_NAME: &str = "ffs-core";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_set() {
        assert_eq!(CRATE_NAME, "ffs-core");
    }
}
