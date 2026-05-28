//! `DaemonClient` — the outbound RPC transport from the MCP server to
//! the substrate daemon.
//!
//! The MCP server is a separate process from the daemon (per
//! ADR-013 and ADR-019); production wires it to the daemon's UDS
//! or Windows named pipe. Tests inject `InProcessDaemonClient`,
//! which holds a `Dispatcher` directly so the MCP protocol and tool
//! translators can be exercised without spawning a daemon process.
//!
//! `DaemonError::CapabilityDenied` carries the typed code that the
//! tool layer translates to an MCP error frame on tool-call results.

use async_trait::async_trait;
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("capability denied: {reason}")]
    CapabilityDenied { reason: String },
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid params: {0}")]
    InvalidParams(String),
    #[error("daemon error {code}: {message}")]
    Other { code: i32, message: String },
    #[error("transport: {0}")]
    Transport(String),
}

#[async_trait]
pub trait DaemonClient: Send + Sync {
    /// Call a daemon JSON-RPC method with the given params. Returns
    /// the raw `result` payload on success; classified errors on
    /// failure.
    async fn call(&self, method: &str, params: Value) -> Result<Value, DaemonError>;
}

// ----- error mapping shared with the production UDS client -----

/// Translate a daemon ApiError code into a typed `DaemonError`.
///
/// These constants mirror `ffs_daemon::api`'s error codes; the MCP
/// crate intentionally does not depend on ffs-daemon at the library
/// level (the in-process test client lives in `dev-dependencies`
/// instead), so the codes are duplicated here as a stable contract.
/// A mismatch would only surface in tests, so the risk is bounded.
pub const ERR_CAPABILITY_DENIED: i32 = 4001;
pub const ERR_NOT_FOUND: i32 = 4040;
pub const ERR_INVALID_PARAMS: i32 = -32602;

pub fn classify_daemon_error(code: i32, message: String, data: Option<Value>) -> DaemonError {
    match code {
        ERR_CAPABILITY_DENIED => {
            let reason = data
                .as_ref()
                .and_then(|d| d.get("reason"))
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| message.clone());
            DaemonError::CapabilityDenied { reason }
        }
        ERR_NOT_FOUND => DaemonError::NotFound(message),
        ERR_INVALID_PARAMS => DaemonError::InvalidParams(message),
        other => DaemonError::Other {
            code: other,
            message,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_denied_extracts_reason_from_data() {
        let e = classify_daemon_error(
            ERR_CAPABILITY_DENIED,
            "capability denied: no read".into(),
            Some(serde_json::json!({"reason": "no read"})),
        );
        match e {
            DaemonError::CapabilityDenied { reason } => assert_eq!(reason, "no read"),
            other => panic!("expected CapabilityDenied; got {other:?}"),
        }
    }

    #[test]
    fn capability_denied_falls_back_to_message_when_data_missing() {
        let e = classify_daemon_error(
            ERR_CAPABILITY_DENIED,
            "capability denied: no read".into(),
            None,
        );
        match e {
            DaemonError::CapabilityDenied { reason } => {
                assert_eq!(reason, "capability denied: no read");
            }
            other => panic!("expected CapabilityDenied; got {other:?}"),
        }
    }

    #[test]
    fn not_found_classified() {
        let e = classify_daemon_error(ERR_NOT_FOUND, "atom not found".into(), None);
        assert!(matches!(e, DaemonError::NotFound(_)));
    }

    #[test]
    fn unknown_code_falls_through_to_other() {
        let e = classify_daemon_error(5001, "store error".into(), None);
        match e {
            DaemonError::Other { code, .. } => assert_eq!(code, 5001),
            other => panic!("expected Other; got {other:?}"),
        }
    }
}
