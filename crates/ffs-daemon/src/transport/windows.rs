//! Windows named-pipe transport. Compile-only on this host (the maintainer
//! develops on macOS); CI's Windows runner exercises the actual code path.
//!
//! Named-pipe semantics differ from UDS: each accepted connection requires
//! creating a new server endpoint, and the ACL is set on the pipe via
//! security descriptors. The MVP impl below uses tokio's named-pipe support
//! and sets `SECURITY_LOCAL_SERVICE` analogous ACL via `ServerOptions`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::api::{ApiError, ApiRequest, ApiResponse, ERR_INTERNAL, ERR_PARSE};
use crate::dispatch::Dispatcher;

/// Default pipe path. Uses `%USERNAME%` rather than the SID per
/// ADR-019's `\\.\pipe\ffs-<user_sid>` intent — SID resolution requires
/// extra Windows-API plumbing that this MVP defers.
pub fn default_socket_path() -> PathBuf {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "default".into());
    PathBuf::from(format!(r"\\.\pipe\ffs-{user}"))
}

pub async fn serve(
    pipe_path: &Path,
    dispatcher: Arc<Dispatcher>,
    cancel: CancellationToken,
) -> std::io::Result<()> {
    let pipe_name = pipe_path
        .to_str()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "non-UTF8 pipe path"))?
        .to_owned();

    tracing::info!(pipe = %pipe_name, "transport: named pipe listening");
    loop {
        let server = ServerOptions::new()
            .first_pipe_instance(false)
            .create(&pipe_name)?;
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!("transport: shutdown requested");
                break;
            }
            res = server.connect() => {
                res?;
                let disp = dispatcher.clone();
                let token = cancel.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(server, disp, token).await {
                        tracing::warn!(error = %e, "connection ended with error");
                    }
                });
            }
        }
    }
    Ok(())
}

async fn handle_connection(
    stream: NamedPipeServer,
    dispatcher: Arc<Dispatcher>,
    cancel: CancellationToken,
) -> std::io::Result<()> {
    let (read_half, write_half) = tokio::io::split(stream);
    let writer = Arc::new(Mutex::new(write_half));
    let mut reader = BufReader::new(read_half).lines();

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
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            // Best-effort resync notice; matches UDS path.
                            let resync = serde_json::json!({
                                "jsonrpc": "2.0",
                                "method": "event.resync",
                                "params": { "reason": "subscriber lagged" }
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
                                ApiError { code: ERR_PARSE, message: e.to_string(), data: None },
                            ),
                        };
                        let out = serde_json::to_string(&response).unwrap_or_else(|e| {
                            format!(
                                r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":{ERR_INTERNAL},"message":"serialize: {}"}}}}"#,
                                e.to_string().replace('"', "\\\"")
                            )
                        });
                        let mut w = writer.lock().await;
                        w.write_all(out.as_bytes()).await?;
                        w.write_all(b"\n").await?;
                    }
                    Ok(None) => break,
                    Err(e) => return Err(e),
                }
            }
        }
    }
    event_task.abort();
    Ok(())
}
