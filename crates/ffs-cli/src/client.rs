//! JSON-RPC 2.0 client over the daemon's local IPC (UDS on Linux/macOS,
//! named pipe on Windows). One connection per call (no multiplexing): the
//! client opens, sends a request line, reads response lines, ignores any
//! interleaved notification frames, and returns the matching response.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use ffs_daemon::api::{ApiError, ApiPayload, ApiResponse};

/// Default socket path mirroring the daemon's UDS path on Unix or the
/// named-pipe path on Windows.
pub fn default_socket_path() -> std::path::PathBuf {
    ffs_daemon::transport::default_socket_path()
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("server returned no response line")]
    NoResponse,
    #[error("server response missing matching id")]
    IdMismatch,
    #[error("rpc error: {message} (code {code})")]
    Rpc {
        code: i32,
        message: String,
        data: Option<Value>,
    },
}

impl ClientError {
    pub fn rpc_code(&self) -> Option<i32> {
        if let ClientError::Rpc { code, .. } = self {
            Some(*code)
        } else {
            None
        }
    }
}

impl From<ApiError> for ClientError {
    fn from(e: ApiError) -> Self {
        Self::Rpc {
            code: e.code,
            message: e.message,
            data: e.data,
        }
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Make a single JSON-RPC call to the daemon at `socket`. Each call opens
/// a fresh connection. For batch / persistent use this is wasteful;
/// upgrade to a pooled client when downstream tasks (Obsidian plugin,
/// MCP server) demand it.
pub async fn call<P: Serialize>(
    socket: &Path,
    method: &str,
    params: P,
) -> Result<Value, ClientError> {
    #[cfg(unix)]
    let stream = tokio::net::UnixStream::connect(socket).await?;
    #[cfg(windows)]
    let stream = tokio::net::windows::named_pipe::ClientOptions::new().open(socket)?;

    let (read_half, mut write_half) = {
        #[cfg(unix)]
        {
            stream.into_split()
        }
        #[cfg(windows)]
        {
            tokio::io::split(stream)
        }
    };

    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": serde_json::to_value(params)?,
    });
    let line = serde_json::to_string(&req)? + "\n";
    write_half.write_all(line.as_bytes()).await?;
    write_half.flush().await?;

    // Read lines, skip notification frames (no `id`), return when we find
    // the matching response.
    let mut reader = BufReader::new(read_half).lines();
    loop {
        let Some(line) = reader.next_line().await? else {
            return Err(ClientError::NoResponse);
        };
        let v: Value = serde_json::from_str(&line)?;
        // Notifications have no `id`; responses do.
        if v.get("id").is_none() {
            continue;
        }
        let resp: ApiResponse = serde_json::from_value(v)?;
        if resp.id != serde_json::json!(id) {
            return Err(ClientError::IdMismatch);
        }
        return match resp.payload {
            ApiPayload::Success { result } => Ok(result),
            ApiPayload::Error { error } => Err(error.into()),
        };
    }
}
