//! `ffs` binary entrypoint. Parses argv via clap and dispatches into the
//! library's `run` function.

use std::io::Write;
use std::process::ExitCode;

use clap::Parser;
use ffs_cli::{Args, run};

#[tokio::main]
async fn main() -> ExitCode {
    // Map clap-parse-failure to EXIT_USAGE (64) rather than clap's default 2.
    let args = match Args::try_parse() {
        Ok(a) => a,
        Err(e) => {
            let _ = e.print();
            return ExitCode::from(ffs_cli::EXIT_USAGE);
        }
    };
    let outcome = run(args).await;
    if !outcome.stdout.is_empty() {
        let _ = std::io::stdout().write_all(outcome.stdout.as_bytes());
    }
    if !outcome.stderr.is_empty() {
        let _ = std::io::stderr().write_all(outcome.stderr.as_bytes());
    }
    ExitCode::from(outcome.code)
}

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        assert_eq!(ffs_core::CRATE_NAME, "ffs-core");
    }
}
