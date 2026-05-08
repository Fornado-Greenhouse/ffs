//! Subprocess host for Python skills (scribe, librarian, auditor).
//! Spawning, supervision, and stdio bridging are introduced by task 10.

pub const CRATE_NAME: &str = "ffs-skills-host";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke() {
        assert_eq!(CRATE_NAME, "ffs-skills-host");
        assert_eq!(ffs_core::CRATE_NAME, "ffs-core");
    }
}
