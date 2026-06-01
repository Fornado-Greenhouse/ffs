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
//!   lands at `$FFS_DATA_DIR/run/ffs.sock`, and the SQLCipher
//!   atom-store database lands at `$FFS_DATA_DIR/atoms.db`.
//! - `FFS_OWNER_KEY_HEX` — 64-hex-character Ed25519 signing key seed.
//!   Production wires this from the OS keychain; for MVP an inline
//!   env-var bootstrap keeps the daemon self-contained. When unset,
//!   the daemon generates a fresh key and warns — fine for a fresh
//!   substrate, problematic for an existing one (the new key won't
//!   verify atoms signed by the old key).
//! - `FFS_SQLCIPHER_KEY_HEX` — 64-hex-character SQLCipher data-
//!   encryption key (DEK). Same fresh-and-warn fallback as
//!   `FFS_OWNER_KEY_HEX`. When the existing `atoms.db` was written
//!   with a different DEK the daemon refuses to open and exits 1.
//!   Keychain-pull lands in task_27.
//! - `FFS_LOG` — `tracing-subscriber` env filter (default `info`).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use ffs_core::PublicKey;
use ffs_core::federation_peers::InMemoryFederationPeerStore;
use ffs_core::multihash::Multihash;
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::ProjectionRenderer;
use ffs_core::quarantine::InMemoryQuarantine;
use ffs_core::store::{AtomStore, SqliteAtomStore, StoreError};
use ffs_core::working_set::InMemoryWorkingSet;

use ffs_daemon::{Dispatcher, EventPublisher, transport};

// The `SpecError`, `RenderError`, and `StoreError` payloads are
// large enough to trigger the `result_large_err` lint when carried
// inline. Box them so `Result<(), StartupError>` stays compact and
// clippy stays happy without forcing every call site to box
// separately.
#[derive(Debug, thiserror::Error)]
enum StartupError {
    #[error("FFS_DATA_DIR is unset and $HOME is not available; pass FFS_DATA_DIR=/path explicitly")]
    NoDataDir,
    #[error("invalid FFS_OWNER_KEY_HEX: {0}")]
    BadKey(String),
    #[error("invalid FFS_SQLCIPHER_KEY_HEX: {0}")]
    BadDek(String),
    #[error("predicate dir {0}: {1}")]
    PredicateDir(PathBuf, Box<ffs_core::predicate::SpecError>),
    #[error("templates dir {0}: not found (run the installer to seed it)")]
    NoTemplates(PathBuf),
    #[error("renderer: {0}")]
    Renderer(Box<ffs_core::projection::RenderError>),
    #[error("atom store {0}: {1}")]
    Store(PathBuf, Box<StoreError>),
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
    let dek = load_or_generate_dek(&data_dir)?;
    let db_path = data_dir.join("atoms.db");

    tracing::info!(
        data_dir = %data_dir.display(),
        db_path = %db_path.display(),
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

    // SQLCipher-backed atom store at $FFS_DATA_DIR/atoms.db. Wrong
    // DEK against an existing database surfaces here as a startup
    // error and a non-zero exit, surfacing key drift loudly instead
    // of silently masking it.
    let store: Arc<dyn AtomStore> = Arc::new(
        SqliteAtomStore::open_with_key(&db_path, &dek)
            .map_err(|e| StartupError::Store(db_path.clone(), Box::new(e)))?,
    );
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
        let seed = decode_hex_32(&hex_seed).map_err(StartupError::BadKey)?;
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

/// Load the SQLCipher DEK from `FFS_SQLCIPHER_KEY_HEX`, or generate
/// a fresh one and warn — same shape as `load_or_generate_owner_key`.
/// The fresh-key path adds an extra warning when `atoms.db` already
/// exists on disk, because in that case the upcoming
/// `SqliteAtomStore::open_with_key` is guaranteed to fail and the
/// user will lose access to their substrate unless they restore the
/// original DEK.
fn load_or_generate_dek(data_dir: &Path) -> Result<[u8; 32], StartupError> {
    if let Ok(hex_seed) = std::env::var("FFS_SQLCIPHER_KEY_HEX") {
        return decode_hex_32(&hex_seed).map_err(StartupError::BadDek);
    }
    use rand::RngCore;
    let mut dek = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut dek);
    let db_path = data_dir.join("atoms.db");
    if db_path.exists() {
        tracing::warn!(
            db_path = %db_path.display(),
            "FFS_SQLCIPHER_KEY_HEX not set but atoms.db already exists. \
             The fresh DEK will fail to open the existing database. \
             Set FFS_SQLCIPHER_KEY_HEX=<64 hex chars> to the original \
             value, or remove atoms.db to start fresh (DESTRUCTIVE)."
        );
    } else {
        let fp = Multihash::blake3_of(&dek).to_multibase();
        tracing::warn!(
            dek_fp = %fp,
            "FFS_SQLCIPHER_KEY_HEX not set — generated a fresh DEK. \
             Save its value via env var or the OS keychain (task_27) \
             before next restart, or atoms.db will be unrecoverable."
        );
    }
    Ok(dek)
}

/// Decode a 64-hex-char string into a 32-byte array. Returns a
/// human-readable `String` error suitable for wrapping in
/// `StartupError::BadKey` / `BadDek`.
fn decode_hex_32(hex: &str) -> Result<[u8; 32], String> {
    let bytes = decode_hex(hex)?;
    if bytes.len() != 32 {
        return Err(format!(
            "expected 32 bytes (64 hex chars), got {}",
            bytes.len()
        ));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
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

    #[test]
    fn decode_hex_32_accepts_64_chars() {
        let hex: String = (0..32).map(|i| format!("{i:02x}")).collect();
        let bytes = decode_hex_32(&hex).expect("64-char hex must decode");
        for (i, &b) in bytes.iter().enumerate() {
            assert_eq!(b as usize, i, "byte {i} preserved");
        }
    }

    #[test]
    fn decode_hex_32_rejects_short_input() {
        // 30 bytes = 60 hex chars.
        let hex: String = (0..30).map(|i| format!("{i:02x}")).collect();
        let err = decode_hex_32(&hex).expect_err("short input must fail");
        assert!(
            err.contains("32 bytes"),
            "error should name expected size: {err}"
        );
    }

    #[test]
    fn decode_hex_32_rejects_long_input() {
        // 40 bytes = 80 hex chars.
        let hex: String = (0..40).map(|i| format!("{i:02x}")).collect();
        let err = decode_hex_32(&hex).expect_err("long input must fail");
        assert!(err.contains("40"), "error should report actual size: {err}");
    }

    #[test]
    fn decode_hex_32_rejects_non_hex() {
        let bad: String = "z".repeat(64);
        assert!(decode_hex_32(&bad).is_err());
    }
}
