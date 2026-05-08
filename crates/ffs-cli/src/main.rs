//! `ffs`: command-line client that resolves `ffs://` URLs by talking to the
//! local daemon. The argv parser, URL resolver, and JSON-RPC client are
//! introduced by task 08.

fn main() {
    println!("ffs scaffold");
}

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        assert_eq!(ffs_core::CRATE_NAME, "ffs-core");
    }
}
