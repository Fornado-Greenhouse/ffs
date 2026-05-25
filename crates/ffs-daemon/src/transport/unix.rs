//! Unix-domain-socket transport. Binds at `~/.ffs/run/ffs.sock` with
//! `0600` permissions, refuses to start if the parent directory is
//! group/other-writable, and removes the socket file on graceful shutdown.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::api::{ApiError, ApiRequest, ApiResponse, ERR_INTERNAL, ERR_PARSE};
use crate::dispatch::Dispatcher;

/// Default socket path: `$HOME/.ffs/run/ffs.sock`.
pub fn default_socket_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join(".ffs").join("run").join("ffs.sock")
}

/// Bind the UDS, accept connections, dispatch JSON-RPC, publish events,
/// shut down cleanly on `cancel`. Errors from individual connections are
/// logged but do not stop the listener.
pub async fn serve(
    socket_path: &Path,
    dispatcher: Arc<Dispatcher>,
    cancel: CancellationToken,
) -> std::io::Result<()> {
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
        check_parent_dir_safe(parent)?;
    }
    // Remove a stale socket from a prior crash.
    let _ = std::fs::remove_file(socket_path);

    let listener = UnixListener::bind(socket_path)?;
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
    tracing::info!(path = ?socket_path, "transport: UDS bound");

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!("transport: shutdown requested");
                break;
            }
            accept = listener.accept() => {
                match accept {
                    Ok((stream, _)) => {
                        let disp = dispatcher.clone();
                        let token = cancel.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, disp, token).await {
                                tracing::warn!(error = %e, "connection ended with error");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "accept failed");
                    }
                }
            }
        }
    }

    let _ = std::fs::remove_file(socket_path);
    tracing::info!(path = ?socket_path, "transport: UDS removed");
    Ok(())
}

/// Refuse to bind under a directory writable by group or other. Prevents
/// a malicious local process from swapping the socket out from under us.
fn check_parent_dir_safe(parent: &Path) -> std::io::Result<()> {
    let meta = std::fs::metadata(parent)?;
    let mode = meta.permissions().mode();
    if mode & 0o022 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "unsafe permissions on {}: {:o} (group/other writable)",
                parent.display(),
                mode
            ),
        ));
    }
    Ok(())
}

async fn handle_connection(
    stream: UnixStream,
    dispatcher: Arc<Dispatcher>,
    cancel: CancellationToken,
) -> std::io::Result<()> {
    let (read_half, write_half) = stream.into_split();
    let writer = Arc::new(Mutex::new(write_half));
    let mut reader = BufReader::new(read_half).lines();

    // Event-forwarding task: subscribes to the publisher and writes
    // notifications onto the same connection.
    let mut event_rx = dispatcher.notifier.subscribe();
    let event_writer = writer.clone();
    let event_cancel = cancel.clone();
    let event_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = event_cancel.cancelled() => break,
                recv = event_rx.recv() => {
                    match recv {
                        Ok(line) => {
                            let mut w = event_writer.lock().await;
                            if w.write_all(line.as_bytes()).await.is_err() { break; }
                            if w.write_all(b"\n").await.is_err() { break; }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::warn!(skipped, "client lagged; sending event.resync");
                            let resync = serde_json::json!({
                                "jsonrpc": "2.0",
                                "method": "event.resync",
                                "params": {
                                    "skipped": skipped,
                                    "reason": "subscriber fell behind; re-query state"
                                }
                            });
                            if let Ok(line) = serde_json::to_string(&resync) {
                                let mut w = event_writer.lock().await;
                                if w.write_all(line.as_bytes()).await.is_err() { break; }
                                if w.write_all(b"\n").await.is_err() { break; }
                            }
                        }
                    }
                }
            }
        }
    });

    // Request-reading loop.
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            line = reader.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        let response = match serde_json::from_str::<ApiRequest>(&line) {
                            Ok(req) => dispatcher.handle(req).await,
                            Err(e) => ApiResponse::error(
                                serde_json::Value::Null,
                                ApiError {
                                    code: ERR_PARSE,
                                    message: e.to_string(),
                                    data: None,
                                },
                            ),
                        };
                        let out = serde_json::to_string(&response).unwrap_or_else(|e| {
                            // Last-resort: synthesize a minimal error JSON.
                            format!(
                                r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":{ERR_INTERNAL},"message":"serialize: {}"}}}}"#,
                                e.to_string().replace('"', "\\\"")
                            )
                        });
                        let mut w = writer.lock().await;
                        w.write_all(out.as_bytes()).await?;
                        w.write_all(b"\n").await?;
                    }
                    Ok(None) => break, // EOF
                    Err(e) => return Err(e),
                }
            }
        }
    }

    event_task.abort();
    Ok(())
}
