//! `ffs-daemon`: the long-running per-user FFS process.
//!
//! Subsequent tasks introduce the JSON-RPC dispatcher, filesystem watchers,
//! federation server, and skill subprocess host. This skeleton exists so the
//! workspace builds and CI can run smoke tests across the binary.

fn main() {
    println!("ffs-daemon scaffold");
}

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        // Smoke test: confirms ffs-daemon links against ffs-core + child crates.
        assert_eq!(ffs_core::CRATE_NAME, "ffs-core");
    }
}
