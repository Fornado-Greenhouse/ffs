//! Subcommand handlers. Each handler turns a parsed `Args` plus the
//! daemon's response into an `Outcome` (exit code + bytes to write).

use std::path::Path;

use serde_json::Value;

use crate::client::{self, ClientError};
use crate::url::{Address, FfsUrl, parse as parse_url};

pub const EXIT_OK: u8 = 0;
pub const EXIT_GENERAL: u8 = 1;
pub const EXIT_CAPABILITY_DENIED: u8 = 2;
pub const EXIT_NOT_FOUND: u8 = 3;
pub const EXIT_USAGE: u8 = 64;

pub struct Outcome {
    pub code: u8,
    pub stdout: String,
    pub stderr: String,
}

impl Outcome {
    pub fn ok(stdout: String) -> Self {
        Self {
            code: EXIT_OK,
            stdout,
            stderr: String::new(),
        }
    }
    pub fn err(code: u8, stderr: String) -> Self {
        Self {
            code,
            stdout: String::new(),
            stderr,
        }
    }
}

fn map_client_err(e: ClientError) -> Outcome {
    use ffs_daemon::api::{ERR_CAPABILITY_DENIED, ERR_NOT_FOUND};
    let (code, msg) = match e.rpc_code() {
        Some(c) if c == ERR_CAPABILITY_DENIED => {
            (EXIT_CAPABILITY_DENIED, "capability denied".to_string())
        }
        Some(c) if c == ERR_NOT_FOUND => (EXIT_NOT_FOUND, "not found".to_string()),
        _ => (EXIT_GENERAL, e.to_string()),
    };
    Outcome::err(code, format!("{msg}: {e}\n"))
}

fn parse_url_or_usage(s: &str) -> Result<FfsUrl, Outcome> {
    parse_url(s).map_err(|e| Outcome::err(EXIT_USAGE, format!("invalid URL: {e}\n")))
}

fn format_result(
    v: &Value,
    json: bool,
    text_extract: impl FnOnce(&Value) -> Option<String>,
) -> String {
    if json {
        return serde_json::to_string_pretty(v).unwrap_or_default() + "\n";
    }
    if let Some(text) = text_extract(v) {
        if text.ends_with('\n') {
            text
        } else {
            text + "\n"
        }
    } else {
        serde_json::to_string_pretty(v).unwrap_or_default() + "\n"
    }
}

/// `ffs cat <url>` — print the human-readable rendering of an FFS URL.
pub async fn cat(socket: &Path, url: &str, json: bool) -> Outcome {
    let parsed = match parse_url_or_usage(url) {
        Ok(p) => p,
        Err(o) => return o,
    };
    match parsed.address {
        Address::Path { path } => {
            let params = serde_json::json!({
                "path": path,
                "as_of": parsed.as_of,
            });
            match client::call(socket, "projection.render", params).await {
                Ok(resp) => Outcome::ok(format_result(&resp, json, |v| {
                    v.get("markdown")
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string())
                })),
                Err(e) => map_client_err(e),
            }
        }
        Address::Atom { hash } => {
            let params = serde_json::json!({"hash": hash});
            match client::call(socket, "atom.get", params).await {
                Ok(resp) => Outcome::ok(format_result(&resp, true, |_| None)), // atoms always JSON
                Err(e) => map_client_err(e),
            }
        }
        Address::Entity { id } => {
            let params = serde_json::json!({"entity": id, "as_of": parsed.as_of});
            match client::call(socket, "atom.list", params).await {
                Ok(resp) => Outcome::ok(format_result(&resp, true, |_| None)),
                Err(e) => map_client_err(e),
            }
        }
    }
}

/// `ffs ls <url>` — list entries at the URL. Path URLs render the listing
/// markdown; entity URLs return one atom hash per line.
pub async fn ls(socket: &Path, url: &str, json: bool) -> Outcome {
    let parsed = match parse_url_or_usage(url) {
        Ok(p) => p,
        Err(o) => return o,
    };
    match parsed.address {
        Address::Path { path } => {
            let params = serde_json::json!({"path": path});
            match client::call(socket, "path.list", params).await {
                Ok(resp) => Outcome::ok(format_result(&resp, json, |v| {
                    v.get("markdown")
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string())
                })),
                Err(e) => map_client_err(e),
            }
        }
        Address::Entity { id } => {
            let params = serde_json::json!({"entity": id, "as_of": parsed.as_of});
            match client::call(socket, "atom.list", params).await {
                Ok(resp) => {
                    if json {
                        Outcome::ok(serde_json::to_string_pretty(&resp).unwrap_or_default() + "\n")
                    } else {
                        let mut out = String::new();
                        if let Some(arr) = resp.as_array() {
                            for env in arr {
                                if let Some(predicate) =
                                    env.get("predicate").and_then(|v| v.as_str())
                                {
                                    out.push_str(predicate);
                                    out.push(' ');
                                }
                                if let Some(tx) = env.get("tx_time").and_then(|v| v.as_str()) {
                                    out.push_str(tx);
                                }
                                out.push('\n');
                            }
                        }
                        Outcome::ok(out)
                    }
                }
                Err(e) => map_client_err(e),
            }
        }
        Address::Atom { .. } => Outcome::err(
            EXIT_USAGE,
            "`ls` is not meaningful for atom URLs; use `cat` or `get`\n".into(),
        ),
    }
}

/// `ffs get <url>` — fetch the raw atom envelope (atom URL) or atoms-for-entity.
pub async fn get(socket: &Path, url: &str) -> Outcome {
    let parsed = match parse_url_or_usage(url) {
        Ok(p) => p,
        Err(o) => return o,
    };
    match parsed.address {
        Address::Atom { hash } => {
            let params = serde_json::json!({"hash": hash});
            match client::call(socket, "atom.get", params).await {
                Ok(resp) => Outcome::ok(serde_json::to_string(&resp).unwrap_or_default() + "\n"),
                Err(e) => map_client_err(e),
            }
        }
        Address::Entity { id } => {
            let params = serde_json::json!({"entity": id, "as_of": parsed.as_of});
            match client::call(socket, "atom.list", params).await {
                Ok(resp) => Outcome::ok(serde_json::to_string(&resp).unwrap_or_default() + "\n"),
                Err(e) => map_client_err(e),
            }
        }
        Address::Path { .. } => Outcome::err(
            EXIT_USAGE,
            "`get` requires an atom or entity URL; use `cat` for paths\n".into(),
        ),
    }
}

/// `ffs health` — print the daemon's health summary.
pub async fn health(socket: &Path, json: bool) -> Outcome {
    match client::call(socket, "health.summary", serde_json::Value::Null).await {
        Ok(resp) => Outcome::ok(format_result(&resp, json, |v| {
            Some(format!(
                "proposals: {}\nquestions: {}\ndrift_flags: {}\natom_count: {}\n",
                v.get("proposals").and_then(|x| x.as_u64()).unwrap_or(0),
                v.get("questions").and_then(|x| x.as_u64()).unwrap_or(0),
                v.get("drift_flags").and_then(|x| x.as_u64()).unwrap_or(0),
                v.get("atom_count").and_then(|x| x.as_u64()).unwrap_or(0),
            ))
        })),
        Err(e) => map_client_err(e),
    }
}

/// `ffs predicate inspect <name>` — print a predicate spec.
pub async fn predicate_inspect(socket: &Path, name: &str) -> Outcome {
    let params = serde_json::json!({"name": name});
    match client::call(socket, "predicate.inspect", params).await {
        Ok(resp) => Outcome::ok(serde_json::to_string_pretty(&resp).unwrap_or_default() + "\n"),
        Err(e) => map_client_err(e),
    }
}

/// `ffs federation peer add <endpoint> <fingerprint>` — stub via daemon.
pub async fn federation_peer_add(socket: &Path, endpoint: &str, fingerprint: &str) -> Outcome {
    let params = serde_json::json!({"endpoint": endpoint, "fingerprint": fingerprint});
    match client::call(socket, "federation.peer.add", params).await {
        Ok(resp) => Outcome::ok(serde_json::to_string_pretty(&resp).unwrap_or_default() + "\n"),
        Err(e) => map_client_err(e),
    }
}

/// `ffs federation peer list` — list peers.
pub async fn federation_peer_list(socket: &Path) -> Outcome {
    match client::call(socket, "federation.peer.list", serde_json::Value::Null).await {
        Ok(resp) => Outcome::ok(serde_json::to_string_pretty(&resp).unwrap_or_default() + "\n"),
        Err(e) => map_client_err(e),
    }
}
