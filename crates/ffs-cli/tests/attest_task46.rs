//! `ffs attest` (ADR-034): a bad basis is a usage error before any RPC.

use std::path::Path;

#[tokio::test]
async fn attest_with_unknown_basis_is_a_usage_error_before_any_rpc() {
    let o = ffs_cli::commands::attest(
        Path::new("/nonexistent/ffs.sock"),
        "zSomeHash",
        "vibes",
        None,
        None,
        None,
        false,
    )
    .await;
    assert_eq!(o.code, ffs_cli::commands::EXIT_USAGE);
    assert!(o.stderr.contains("--basis must be one of"), "{}", o.stderr);
}
