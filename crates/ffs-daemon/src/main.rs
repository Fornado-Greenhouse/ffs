//! `ffs-daemon` binary entrypoint.
//!
//! For MVP this is a minimal wrapper: the dispatcher library + transport
//! lands in this task (07), but the full binary wiring (config file,
//! owner-identity provisioning, SQLCipher DEK from keychain, predicate-
//! specs dir, templates dir, SIGTERM handler) gets stitched together by
//! the onboarding scripts in task 22. Until then this `main` simply
//! prints a banner; the library API (`ffs_daemon::*`) is fully usable
//! from tests and downstream crates.

fn main() {
    println!("ffs-daemon scaffold — library ready (task 07); binary wiring lands with task 22");
}

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        // Confirms the binary still links against the lib + ffs-core chain.
        assert_eq!(ffs_core::CRATE_NAME, "ffs-core");
    }
}
