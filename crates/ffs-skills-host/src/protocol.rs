//! Line-delimited JSON envelopes spoken over a skill's stdio. The
//! channel is bidirectional: the host invokes the skill, and the
//! skill may issue substrate-access queries back to the host during
//! that invocation. Each message has an `id` (string) so the two
//! sides can correlate responses without ordering assumptions.
//!
//! Wire shape (one JSON object per line):
//!
//! ```text
//! Host → Skill:
//!   {"id":"abc","kind":"invoke","input":{...}}            // request a unit of work
//!   {"id":"xyz","kind":"query_response","result":{...}}   // answer a query the skill made
//!   {"id":"xyz","kind":"query_error","error":"..."}       // reject a query
//!
//! Skill → Host:
//!   {"id":"abc","kind":"result","output":{...}}           // finish an invocation
//!   {"id":"abc","kind":"error","error":"..."}             // invocation failed
//!   {"id":"xyz","kind":"query","method":"atom.get",       // ask the substrate
//!    "params":{...}}
//! ```
//!
//! IDs are skill-private strings; the host echoes the skill's id on
//! responses to queries and uses host-private ids for invocations.

use serde::{Deserialize, Serialize};

/// Anything the host writes to a skill's stdin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostToSkill {
    Invoke {
        id: String,
        input: serde_json::Value,
    },
    QueryResponse {
        id: String,
        result: serde_json::Value,
    },
    QueryError {
        id: String,
        error: String,
    },
    /// Polite shutdown signal: the skill should finish in-flight work
    /// and exit. The host follows up with SIGTERM after 5s if needed.
    Shutdown {},
}

/// Anything the skill writes to its stdout.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkillToHost {
    Result {
        id: String,
        output: serde_json::Value,
    },
    Error {
        id: String,
        error: String,
    },
    Query {
        id: String,
        method: String,
        params: serde_json::Value,
    },
    /// Optional log line the host forwards to `tracing`. Keeps skill
    /// debug output out of the result stream.
    Log {
        level: String,
        message: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("malformed JSON from skill: {0}")]
    BadJson(#[from] serde_json::Error),
    #[error("skill closed its stdout before responding")]
    StreamClosed,
}

/// Serialize a host message to a single line (no embedded newlines)
/// terminated by `\n`. Skills parse one line per message.
pub fn encode_host(msg: &HostToSkill) -> Result<String, serde_json::Error> {
    let mut s = serde_json::to_string(msg)?;
    s.push('\n');
    Ok(s)
}

/// Parse one line from a skill into a `SkillToHost`.
pub fn decode_skill(line: &str) -> Result<SkillToHost, serde_json::Error> {
    serde_json::from_str(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_invoke_round_trips() {
        let msg = HostToSkill::Invoke {
            id: "abc".into(),
            input: serde_json::json!({"k": "v"}),
        };
        let encoded = encode_host(&msg).unwrap();
        assert!(encoded.ends_with('\n'));
        let decoded: HostToSkill = serde_json::from_str(encoded.trim()).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn skill_result_round_trips() {
        let line = r#"{"id":"abc","kind":"result","output":{"ok":true}}"#;
        let parsed = decode_skill(line).unwrap();
        match parsed {
            SkillToHost::Result { id, output } => {
                assert_eq!(id, "abc");
                assert_eq!(output, serde_json::json!({"ok": true}));
            }
            other => panic!("expected Result; got {other:?}"),
        }
    }

    #[test]
    fn skill_query_decodes() {
        let line = r#"{"id":"q1","kind":"query","method":"atom.get","params":{"hash":"abc"}}"#;
        let parsed = decode_skill(line).unwrap();
        assert!(matches!(parsed, SkillToHost::Query { .. }));
    }

    #[test]
    fn skill_log_decodes() {
        let line = r#"{"kind":"log","level":"info","message":"hi"}"#;
        let parsed = decode_skill(line).unwrap();
        match parsed {
            SkillToHost::Log { level, message } => {
                assert_eq!(level, "info");
                assert_eq!(message, "hi");
            }
            other => panic!("expected Log; got {other:?}"),
        }
    }
}
