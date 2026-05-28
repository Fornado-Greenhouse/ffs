//! Line-delimited JSON-RPC over an async reader / writer. Production
//! uses stdin/stdout (the canonical MCP transport per the spec);
//! tests pipe in-memory readers/writers so they don't depend on a
//! real stdio handle.

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::{McpRequest, McpResponse, McpServer};

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Stdin/stdout MCP loop the binary entrypoint runs. Returns on EOF
/// (client closed stdin), an explicit `notifications/cancelled`
/// from the client, or any I/O error.
pub async fn serve_stdio(server: McpServer) -> Result<(), TransportError> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    serve(server, stdin, stdout).await
}

/// Generic transport loop — runs the protocol over any `AsyncRead +
/// AsyncWrite` pair. Tests use `tokio::io::duplex` to pipe one end
/// to a stub client and assert on the response stream.
pub async fn serve<R, W>(server: McpServer, reader: R, mut writer: W) -> Result<(), TransportError>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let req: McpRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                // Per JSON-RPC 2.0 spec, return a parse error with
                // null id when the request can't be parsed.
                let resp = McpResponse::error(
                    serde_json::Value::Null,
                    crate::protocol::ERR_PARSE,
                    format!("parse: {e}"),
                    None,
                );
                write_response(&mut writer, &resp).await?;
                continue;
            }
        };
        let resp = server.handle(req).await;
        write_response(&mut writer, &resp).await?;
    }
    Ok(())
}

async fn write_response<W>(writer: &mut W, resp: &McpResponse) -> Result<(), TransportError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let line = serde_json::to_string(resp).expect("McpResponse must serialize");
    writer.write_all(line.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DaemonClient, DaemonError, McpServer};
    use async_trait::async_trait;
    use std::sync::Arc;
    use tokio::io::AsyncBufReadExt;

    struct PanicClient;
    #[async_trait]
    impl DaemonClient for PanicClient {
        async fn call(
            &self,
            _: &str,
            _: serde_json::Value,
        ) -> Result<serde_json::Value, DaemonError> {
            panic!("client shouldn't be called for tools/list test");
        }
    }

    #[tokio::test]
    async fn serve_round_trips_initialize_then_tools_list() {
        // tokio::io::duplex returns two fully-bidirectional halves;
        // each side reads what the OTHER side wrote. Split each side
        // into (read, write) so the server can read requests from
        // its end and write responses back, while the client does
        // the mirror image.
        let (client_side, server_side) = tokio::io::duplex(64 * 1024);
        let (server_read, mut server_write) = tokio::io::split(server_side);
        let (client_read, mut client_write) = tokio::io::split(client_side);

        let server = McpServer::new(Arc::new(PanicClient), "test-agent");
        let handle = tokio::spawn(async move {
            let _ = serve(server, server_read, &mut server_write).await;
        });

        let init = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        });
        let list = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list"
        });
        let mut payload = serde_json::to_vec(&init).unwrap();
        payload.push(b'\n');
        payload.extend(serde_json::to_vec(&list).unwrap());
        payload.push(b'\n');
        client_write.write_all(&payload).await.unwrap();
        // Shutdown signals EOF to the server's read half. Dropping
        // the WriteHalf alone does not — `tokio::io::split` leaves
        // the underlying DuplexStream alive while the other half
        // (the ReadHalf on the same side) is still in scope.
        client_write.shutdown().await.unwrap();

        let mut lines = BufReader::new(client_read).lines();
        let l1 = lines.next_line().await.unwrap().expect("first response");
        let l2 = lines.next_line().await.unwrap().expect("second response");
        let r1: serde_json::Value = serde_json::from_str(&l1).unwrap();
        let r2: serde_json::Value = serde_json::from_str(&l2).unwrap();
        assert_eq!(r1["id"], 1);
        assert_eq!(r1["result"]["protocolVersion"], crate::PROTOCOL_VERSION);
        assert_eq!(r2["id"], 2);
        assert_eq!(r2["result"]["tools"].as_array().unwrap().len(), 6);

        handle.await.unwrap();
    }

    #[tokio::test]
    async fn serve_returns_parse_error_on_malformed_line() {
        let (client_side, server_side) = tokio::io::duplex(64 * 1024);
        let (server_read, mut server_write) = tokio::io::split(server_side);
        let (client_read, mut client_write) = tokio::io::split(client_side);

        let server = McpServer::new(Arc::new(PanicClient), "test-agent");
        let handle = tokio::spawn(async move {
            let _ = serve(server, server_read, &mut server_write).await;
        });

        client_write.write_all(b"not-json\n").await.unwrap();
        client_write.shutdown().await.unwrap();

        let mut lines = BufReader::new(client_read).lines();
        let l = lines.next_line().await.unwrap().expect("error response");
        let r: serde_json::Value = serde_json::from_str(&l).unwrap();
        assert_eq!(r["error"]["code"], crate::protocol::ERR_PARSE);
        handle.await.unwrap();
    }
}
