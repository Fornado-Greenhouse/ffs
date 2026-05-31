//! `ffs-mcp` binary entrypoint.
//!
//! The MCP server is a separate process from the daemon (per
//! ADR-013); an MCP-aware agent (e.g., Claude Code) spawns it as a
//! subprocess and speaks line-delimited JSON-RPC 2.0 over the
//! subprocess's stdio. This binary reads stdin, dispatches via
//! `McpServer::handle`, and writes responses to stdout.
//!
//! Configuration via environment variables:
//!
//! - `FFS_DAEMON_SOCKET` — path to the daemon's UDS (Linux/macOS) or
//!   named pipe (Windows). Defaults to `$FFS_DATA_DIR/run/ffs.sock`
//!   when `FFS_DATA_DIR` is set, otherwise `$HOME/.ffs/run/ffs.sock`.
//! - `FFS_AGENT_IDENTITY` — identity URI stamped onto
//!   `ffs_author_atom` provenance entries (defaults to
//!   `mcp-agent:local`).
//! - `FFS_LOG` — `tracing-subscriber` env filter (default `info`).
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
//!         "FFS_AGENT_IDENTITY": "mcp-agent:claude-code"
//!       }
//!     }
//!   }
//! }
//! ```

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

use ffs_mcp::{DaemonClient, DaemonError, McpServer, classify_daemon_error, serve_stdio};

fn main() -> ExitCode {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    match runtime.block_on(run()) {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("ffs-mcp: {e}");
            ExitCode::from(1)
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("FFS_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let socket = resolve_socket_path()?;
    let identity =
        std::env::var("FFS_AGENT_IDENTITY").unwrap_or_else(|_| "mcp-agent:local".to_string());

    tracing::info!(
        socket = %socket.display(),
        identity = %identity,
        "ffs-mcp starting"
    );

    let client = Arc::new(UdsDaemonClient::new(socket));
    let server = McpServer::new(client, identity);
    serve_stdio(server).await?;
    Ok(())
}

fn resolve_socket_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Ok(explicit) = std::env::var("FFS_DAEMON_SOCKET") {
        return Ok(PathBuf::from(explicit));
    }
    let data_dir = if let Ok(d) = std::env::var("FFS_DATA_DIR") {
        PathBuf::from(d)
    } else {
        let home =
            std::env::var_os("HOME").ok_or("FFS_DAEMON_SOCKET unset and $HOME unavailable")?;
        PathBuf::from(home).join(".ffs")
    };
    Ok(data_dir.join("run").join("ffs.sock"))
}

/// Production `DaemonClient` that opens one Unix-domain-socket
/// connection per call. The daemon's transport speaks
/// newline-delimited JSON-RPC 2.0 (ADR-019), so a fresh connection
/// per call is fine for the MCP server's expected call volume; if
/// that changes, pool connections behind the same trait.
///
/// Reuses the well-known JSON-RPC error codes (`ApiError`-style) by
/// classifying them via `classify_daemon_error` — no `ffs-daemon`
/// dependency required.
struct UdsDaemonClient {
    socket: PathBuf,
    next_id: AtomicU64,
    // A mutex around the connection state isn't strictly needed
    // (each call opens its own socket), but keeping the field
    // present lets us upgrade to pooling later without breaking the
    // trait signature.
    _connect_guard: Mutex<()>,
}

impl UdsDaemonClient {
    fn new(socket: PathBuf) -> Self {
        Self {
            socket,
            next_id: AtomicU64::new(1),
            _connect_guard: Mutex::new(()),
        }
    }
}

#[async_trait]
impl DaemonClient for UdsDaemonClient {
    async fn call(&self, method: &str, params: Value) -> Result<Value, DaemonError> {
        #[cfg(unix)]
        let stream = tokio::net::UnixStream::connect(&self.socket)
            .await
            .map_err(|e| {
                DaemonError::Transport(format!("connect {}: {e}", self.socket.display()))
            })?;
        #[cfg(windows)]
        let stream = tokio::net::windows::named_pipe::ClientOptions::new()
            .open(&self.socket)
            .map_err(|e| DaemonError::Transport(format!("open {}: {e}", self.socket.display())))?;

        #[cfg(unix)]
        let (read_half, mut write_half) = stream.into_split();
        #[cfg(windows)]
        let (read_half, mut write_half) = tokio::io::split(stream);

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut line = serde_json::to_string(&req)
            .map_err(|e| DaemonError::Transport(format!("encode: {e}")))?;
        line.push('\n');
        write_half
            .write_all(line.as_bytes())
            .await
            .map_err(|e| DaemonError::Transport(format!("write: {e}")))?;
        write_half
            .flush()
            .await
            .map_err(|e| DaemonError::Transport(format!("flush: {e}")))?;

        let mut reader = BufReader::new(read_half).lines();
        loop {
            let next = reader
                .next_line()
                .await
                .map_err(|e| DaemonError::Transport(format!("read: {e}")))?;
            let Some(line) = next else {
                return Err(DaemonError::Transport(
                    "daemon closed without response".into(),
                ));
            };
            let v: Value = serde_json::from_str(&line)
                .map_err(|e| DaemonError::Transport(format!("parse: {e}")))?;
            // Notification frames have no `id` — skip them.
            if v.get("id").is_none() {
                continue;
            }
            // Match our id, else keep reading (shouldn't happen on a
            // fresh per-call connection but kept for safety).
            if v.get("id") != Some(&serde_json::json!(id)) {
                continue;
            }
            if let Some(err) = v.get("error") {
                let code = err.get("code").and_then(Value::as_i64).unwrap_or(-32000) as i32;
                let message = err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
                    .to_string();
                let data = err.get("data").cloned();
                return Err(classify_daemon_error(code, message, data));
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        assert_eq!(ffs_mcp::CRATE_NAME, "ffs-mcp");
    }

    // Note: a direct test of `resolve_socket_path()` would have to
    // mutate the process environment, which the workspace lint
    // policy (`-F unsafe-code`) forbids. The function is exercised
    // end-to-end by the `binary_end_to_end` test in `ffs-daemon`
    // through the published `FFS_DAEMON_SOCKET` contract instead.
}
