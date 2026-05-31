//! `ffs-daemon` binary entrypoint.
//!
//! This is the load-bearing process per ADR-015: it owns the SQLite atom
//! store, the predicate registry, the projection renderer, the
//! fastpath watcher, the skills host, the federation peer state, and
//! the local JSON-RPC dispatcher exposed over a Unix domain socket
//! (Linux/macOS) or named pipe (Windows; not built on this branch).
//!
//! Configuration comes from environment variables — flag parsing is
//! intentionally deferred to keep the binary scope-tight; the
//! installer (task_22) writes a small wrapper script that sets
//! these variables before invoking the daemon.
//!
//! Environment knobs:
//!
//! - `FFS_DATA_DIR` — root of the per-user `~/.ffs/` tree (default
//!   `$HOME/.ffs`). Predicates load from `$FFS_DATA_DIR/config/predicates/`,
//!   templates from `$FFS_DATA_DIR/config/templates/`, the socket
//!   lands at `$FFS_DATA_DIR/run/ffs.sock`.
//! - `FFS_OWNER_KEY_HEX` — 64-hex-character Ed25519 signing key seed.
//!   Production wires this from the OS keychain; for MVP an inline
//!   env-var bootstrap keeps the daemon self-contained. When unset,
//!   the daemon generates a fresh key and warns — fine for a fresh
//!   substrate, problematic for an existing one (the new key won't
//!   verify atoms signed by the old key).
//! - `FFS_LOG` — `tracing-subscriber` env filter (default `info`).

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use ffs_core::PublicKey;
use ffs_core::federation_peers::InMemoryFederationPeerStore;
use ffs_core::multihash::Multihash;
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::ProjectionRenderer;
use ffs_core::quarantine::InMemoryQuarantine;
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::working_set::InMemoryWorkingSet;

use ffs_daemon::{Dispatcher, EventPublisher, transport};

// The `SpecError` and `RenderError` payloads are large enough to
// trigger the `result_large_err` lint when carried inline. Box them
// so `Result<(), StartupError>` stays compact and clippy stays
// happy without forcing every call site to box separately.
#[derive(Debug, thiserror::Error)]
enum StartupError {
    #[error("FFS_DATA_DIR is unset and $HOME is not available; pass FFS_DATA_DIR=/path explicitly")]
    NoDataDir,
    #[error("invalid FFS_OWNER_KEY_HEX: {0}")]
    BadKey(String),
    #[error("predicate dir {0}: {1}")]
    PredicateDir(PathBuf, Box<ffs_core::predicate::SpecError>),
    #[error("templates dir {0}: not found (run the installer to seed it)")]
    NoTemplates(PathBuf),
    #[error("renderer: {0}")]
    Renderer(Box<ffs_core::projection::RenderError>),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

fn main() -> ExitCode {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    match runtime.block_on(run()) {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("ffs-daemon: {e}");
            ExitCode::from(1)
        }
    }
}

async fn run() -> Result<(), StartupError> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("FFS_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let data_dir = resolve_data_dir()?;
    let predicates_dir = data_dir.join("config").join("predicates");
    let templates_dir = data_dir.join("config").join("templates");
    let run_dir = data_dir.join("run");
    std::fs::create_dir_all(&predicates_dir)?;
    std::fs::create_dir_all(&templates_dir)?;
    std::fs::create_dir_all(&run_dir)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&run_dir, std::fs::Permissions::from_mode(0o700))?;

    let signing_key = load_or_generate_owner_key()?;
    let owner_pubkey = PublicKey::from_verifying(&signing_key.verifying_key());

    tracing::info!(
        data_dir = %data_dir.display(),
        owner = %owner_pubkey.to_multibase(),
        "ffs-daemon starting"
    );

    // Predicate registry + templates. Templates dir empty is a hard
    // error — projection rendering would fail at first request. The
    // installer seeds both.
    let registry = Arc::new(SpecRegistry::new());
    registry
        .load_dir(&predicates_dir)
        .map_err(|e| StartupError::PredicateDir(predicates_dir.clone(), Box::new(e)))?;
    if !templates_dir.exists() || std::fs::read_dir(&templates_dir)?.next().is_none() {
        return Err(StartupError::NoTemplates(templates_dir));
    }

    // In-memory backends across the board for MVP: SQLite atom-store
    // wiring is documented but not flipped on here (the in-memory
    // store is functionally complete and exercised by the workspace
    // test suite). A `--persist` flag is a Phase 2 add.
    let store: Arc<dyn AtomStore> = Arc::new(MemAtomStore::new());
    let renderer = Arc::new(
        ProjectionRenderer::new(store.clone(), registry.clone(), &templates_dir)
            .map_err(|e| StartupError::Renderer(Box::new(e)))?,
    );

    let dispatcher = Dispatcher {
        store: store.clone(),
        registry: registry.clone(),
        renderer,
        notifier: Arc::new(EventPublisher::new()),
        owner: owner_pubkey.clone(),
        quarantine: Arc::new(InMemoryQuarantine::new()),
        scribe: None,
        working_set: Arc::new(InMemoryWorkingSet::new()),
        signing_key: Some(Arc::new(signing_key)),
        federation_peers: Arc::new(InMemoryFederationPeerStore::new()),
        federation_client: None,
        our_cert_fingerprint: None,
        peer_mounts: Arc::new(ffs_federation::mount::InMemoryPeerMount::new()),
    };
    let dispatcher = Arc::new(dispatcher);

    let socket_path = run_dir.join("ffs.sock");
    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_for_signal = cancel.clone();
    tokio::spawn(async move {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("received SIGINT");
            }
            _ = sigterm.recv() => {
                tracing::info!("received SIGTERM");
            }
        }
        cancel_for_signal.cancel();
    });

    transport::serve(&socket_path, dispatcher, cancel).await?;
    Ok(())
}

fn resolve_data_dir() -> Result<PathBuf, StartupError> {
    if let Ok(explicit) = std::env::var("FFS_DATA_DIR") {
        return Ok(PathBuf::from(explicit));
    }
    let home = std::env::var_os("HOME").ok_or(StartupError::NoDataDir)?;
    Ok(PathBuf::from(home).join(".ffs"))
}

fn load_or_generate_owner_key() -> Result<SigningKey, StartupError> {
    if let Ok(hex_seed) = std::env::var("FFS_OWNER_KEY_HEX") {
        let bytes = decode_hex(&hex_seed).map_err(StartupError::BadKey)?;
        if bytes.len() != 32 {
            return Err(StartupError::BadKey(format!(
                "expected 32 bytes (64 hex chars), got {}",
                bytes.len()
            )));
        }
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&bytes);
        return Ok(SigningKey::from_bytes(&seed));
    }
    // No env-var seed: generate a fresh key and warn. This is fine
    // for a brand-new substrate; for an existing one the user
    // should set FFS_OWNER_KEY_HEX so prior atoms still verify.
    use rand::RngCore;
    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    let key = SigningKey::from_bytes(&seed);
    let fp = Multihash::blake3_of(&seed).to_multibase();
    tracing::warn!(
        key_fp = %fp,
        "FFS_OWNER_KEY_HEX not set — generated a fresh signing key. \
         Existing atoms signed by other keys will not validate. \
         Set FFS_OWNER_KEY_HEX=<64 hex chars> to pin a stable identity."
    );
    Ok(key)
}

fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return Err("hex must be even length".into());
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    for chunk in s.as_bytes().chunks(2) {
        let hi = (chunk[0] as char)
            .to_digit(16)
            .ok_or_else(|| format!("bad hex char {}", chunk[0] as char))?;
        let lo = (chunk[1] as char)
            .to_digit(16)
            .ok_or_else(|| format!("bad hex char {}", chunk[1] as char))?;
        out.push(((hi << 4) | lo) as u8);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke() {
        assert_eq!(ffs_core::CRATE_NAME, "ffs-core");
    }

    #[test]
    fn decode_hex_round_trip() {
        assert_eq!(decode_hex("00ff").unwrap(), vec![0u8, 255u8]);
        assert!(decode_hex("00f").is_err());
        assert!(decode_hex("zz").is_err());
    }
}
