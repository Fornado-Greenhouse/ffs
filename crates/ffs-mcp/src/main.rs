//! `ffs-mcp` binary entrypoint.
//!
//! The MCP server is a separate process from the daemon (per
//! ADR-013); an MCP-aware agent spawns it as a subprocess and
//! speaks line-delimited JSON-RPC over the subprocess's stdio. This
//! binary reads stdin, dispatches via `McpServer::handle`, and
//! writes responses to stdout.
//!
//! Connection to the daemon happens via a `DaemonClient` injected at
//! startup. The production binding (UDS / Windows named pipe with
//! line-delimited JSON-RPC mirroring the daemon's transport layer)
//! is wired in by task_22's onboarding scripts; this entrypoint
//! prints a stub message when run standalone so the build still
//! produces a runnable binary.
//!
//! Sample agent configuration (Claude Code `mcpServers` block):
//!
//! ```jsonc
//! {
//!   "mcpServers": {
//!     "ffs": {
//!       "command": "ffs-mcp",
//!       "args": [],
//!       "env": {
//!         "FFS_DAEMON_SOCKET": "/Users/you/.ffs/run/ffs.sock",
//!         "FFS_AGENT_KEY": "/Users/you/.ffs/keys/claude.ed25519"
//!       }
//!     }
//!   }
//! }
//! ```
//!
//! Set `FFS_ALLOW_AUTHOR=1` (or pass `--allow-author` once the flag
//! is wired) to issue the agent a temporary write capability for
//! local testing per the task spec.

use std::process::ExitCode;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    // Production wiring (UDS DaemonClient + agent-key configuration)
    // lands in task_22; this binary surfaces a clear stub until
    // then so a stray invocation doesn't claim to be working.
    eprintln!(
        "ffs-mcp {} — daemon binding not wired yet (task_22). \
         Use the library API + InProcessDaemonClient for integration testing.",
        env!("CARGO_PKG_VERSION")
    );
    ExitCode::from(0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        assert_eq!(ffs_mcp::CRATE_NAME, "ffs-mcp");
    }
}
