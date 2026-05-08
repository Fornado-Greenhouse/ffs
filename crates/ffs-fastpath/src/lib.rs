//! Filesystem watcher + reverse-map diff classifier + supersession-or-route
//! decision. Introduced by task 09.

pub const CRATE_NAME: &str = "ffs-fastpath";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke() {
        assert_eq!(CRATE_NAME, "ffs-fastpath");
        assert_eq!(ffs_core::CRATE_NAME, "ffs-core");
    }
}
