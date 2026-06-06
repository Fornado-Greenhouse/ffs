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
//! - `FFS_SKILL_TIMEOUT_MS` — optional override for every skill's
//!   per-call timeout (default reads from each `SKILL.md`
//!   `timeout_ms:` field, falling back to 30000 ms). Useful when
//!   running on a slow machine where the default would cause
//!   spurious restarts.
//! - `FFS_KEYRING_DISABLE` — set to `1` to skip the OS keychain
//!   entirely. Useful in CI and inside containers without a session
//!   keychain. When set, the daemon falls back to the env-var path
//!   for both `FFS_OWNER_KEY_HEX` and `FFS_SQLCIPHER_KEY_HEX`.
//! - `FFS_LOG` — `tracing-subscriber` env filter (default `info`).
//!
//! Key precedence (per task_27): env-var → OS keychain →
//! generate-and-warn. The env-var path also writes the value into
//! the keychain (when not disabled) so the next boot can drop the
//! env var. This makes the migration from task_22's env-var
//! bootstrap to a keychain-pinned identity a one-boot operation.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use ffs_core::PublicKey;
use ffs_core::SuppressionRegistry;
use ffs_core::federation_peers::InMemoryFederationPeerStore;
use ffs_core::multihash::Multihash;
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::ProjectionRenderer;
use ffs_core::quarantine::InMemoryQuarantine;
use ffs_core::store::{AtomStore, SqliteAtomStore, StoreError};
use ffs_core::working_set::InMemoryWorkingSet;
use ffs_skills_host::{RefuseAllProxy, SkillsHost};

use ffs_daemon::ingest_watcher::DEFAULT_POLL_INTERVAL;
use ffs_daemon::{
    Dispatcher, EventPublisher, IngestWatcher, IngestWatcherConfig, SkillsHostScribeExtractor,
    WorkingSetMaterializer, transport,
};

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

    let (signing_key, owner_source) = load_or_generate_owner_key()?;
    let owner_pubkey = PublicKey::from_verifying(&signing_key.verifying_key());
    let owner_pubkey_str = owner_pubkey.to_multibase();
    // The DEK is per-substrate and keyed by the owner pubkey so a
    // future cohort-style multi-substrate-per-user setup can keep
    // each substrate's DEK distinct under the same OS user.
    let (dek, dek_source) = load_or_generate_dek(&data_dir, &owner_pubkey_str)?;
    let db_path = data_dir.join("atoms.db");

    tracing::info!(
        data_dir = %data_dir.display(),
        db_path = %db_path.display(),
        owner = %owner_pubkey_str,
        owner_source = %owner_source,
        dek_source = %dek_source,
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

    // Self-grant bootstrap: a brand-new substrate has zero
    // capability atoms for the owner, which means every RPC that
    // goes through the dispatcher's capability gate (ingest.accept,
    // audit.publish_summary, bridge.establish, …) gets denied. The
    // owner of a fresh substrate is sovereign over it by default;
    // sign and insert a self-grant if no owner capabilities exist
    // yet.
    bootstrap_owner_self_capability(&*store, &signing_key, &owner_pubkey)?;

    let renderer = Arc::new(
        ProjectionRenderer::new(store.clone(), registry.clone(), &templates_dir)
            .map_err(|e| StartupError::Renderer(Box::new(e)))?,
    );

    // Shared singletons: the event publisher (broadcast for
    // notifications), the working-set state, and the suppression
    // registry (anti-loop coordination between the materializer
    // and any future fast-path watcher).
    let publisher = Arc::new(EventPublisher::new());
    let working_set = Arc::new(InMemoryWorkingSet::new());
    let suppression = Arc::new(SuppressionRegistry::new());
    let quarantine = Arc::new(InMemoryQuarantine::new());

    // Skills host: discover bundles under $FFS_DATA_DIR/skills and
    // spawn each as a supervised subprocess. `RefuseAllProxy` is
    // the substrate-access stub; Phase 2 wires a real proxy that
    // routes skill-side `query` frames through the dispatcher with
    // the skill's identity.
    let skills_dir = data_dir.join("skills");
    let mut skills_host = SkillsHost::new(Arc::new(RefuseAllProxy));
    let mut skill_registry = ffs_skills_host::SkillRegistry::new();
    match skill_registry.discover(&skills_dir) {
        Ok(()) => {
            if let Some(timeout) = parse_skill_timeout()? {
                skill_registry.override_all_timeouts(timeout);
                tracing::info!(
                    timeout_ms = timeout.as_millis() as u64,
                    "FFS_SKILL_TIMEOUT_MS override applied"
                );
            }
            skills_host.spawn_from_registry(&skill_registry);
            let names: Vec<&str> = skills_host
                .skills()
                .iter()
                .map(|s| s.manifest.name.as_str())
                .collect();
            tracing::info!(
                skills_dir = %skills_dir.display(),
                skills = ?names,
                "skills host: discovered and spawned"
            );
        }
        Err(e) => {
            tracing::warn!(
                skills_dir = %skills_dir.display(),
                error = %e,
                "skills host discovery failed; daemon will run without scribe"
            );
        }
    }
    let skills_host = Arc::new(skills_host);
    let scribe: Option<Arc<dyn ffs_daemon::dispatch::ScribeExtractor>> =
        if skills_host.get("scribe").is_some() {
            Some(Arc::new(SkillsHostScribeExtractor::new(
                skills_host.clone(),
            )))
        } else {
            tracing::warn!(
                "scribe skill not found under $FFS_DATA_DIR/skills/; \
             ingest will accept submissions but not produce proposals"
            );
            None
        };

    let dispatcher = Dispatcher {
        store: store.clone(),
        registry: registry.clone(),
        renderer: renderer.clone(),
        notifier: publisher.clone(),
        owner: owner_pubkey.clone(),
        quarantine: quarantine.clone(),
        scribe: scribe.clone(),
        working_set: working_set.clone(),
        signing_key: Some(Arc::new(signing_key)),
        federation_peers: Arc::new(InMemoryFederationPeerStore::new()),
        federation_client: None,
        our_cert_fingerprint: None,
        peer_mounts: Arc::new(ffs_federation::mount::InMemoryPeerMount::new()),
    };
    let dispatcher = Arc::new(dispatcher);

    // Working-set materializer: subscribes to event.atom.committed
    // and writes rendered projections to disk under $FFS_DATA_DIR/.
    // Spawned before the transport binds so any commit notification
    // emitted during binding gets observed.
    let materializer = Arc::new(WorkingSetMaterializer::new(
        renderer.clone(),
        working_set.clone(),
        suppression.clone(),
        data_dir.clone(),
        owner_pubkey.clone(),
    ));
    let _materializer_handle = materializer.spawn(publisher.clone());
    tracing::info!("working-set materializer subscribed to event.atom.committed");

    let socket_path = run_dir.join("ffs.sock");
    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_for_signal = cancel.clone();
    let skills_host_for_signal = skills_host.clone();
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
        // Tell every skill to shut down (the supervisors enforce a
        // SHUTDOWN_GRACE window then SIGKILL). Doing this before
        // the transport's own shutdown lets in-flight scribe calls
        // finish or be cancelled before the daemon exits.
        skills_host_for_signal.shutdown_all();
        cancel_for_signal.cancel();
    });

    // Ingest watcher: poll $FFS_DATA_DIR/ingest/, submit new .md
    // files to the quarantine, spawn scribe extraction, move
    // processed files into .processed/. Held in scope so its
    // PollWatcher lives until the daemon exits.
    let _ingest_watcher = IngestWatcher::start(IngestWatcherConfig {
        ingest_dir: data_dir.join("ingest"),
        quarantine: quarantine.clone(),
        scribe: scribe.clone(),
        publisher: publisher.clone(),
        cancel: cancel.clone(),
        poll_interval: DEFAULT_POLL_INTERVAL,
    })
    .map_err(std::io::Error::other)?;
    tracing::info!(
        ingest_dir = %data_dir.join("ingest").display(),
        "ingest watcher started"
    );

    transport::serve(&socket_path, dispatcher, cancel).await?;
    Ok(())
}

/// If the owner has zero capability atoms in the store, sign and
/// insert a sovereign self-grant (Read + Write + Supersede × default
/// scope, no expiry). Idempotent: re-running against a store that
/// already has owner caps is a no-op.
///
/// Why this is a daemon-binary concern rather than something the
/// installer does: the signing key lives in the daemon's env, so
/// only the daemon can sign on the owner's behalf. The installer
/// can seed predicates and skills but can't produce signed atoms.
fn bootstrap_owner_self_capability(
    store: &dyn AtomStore,
    signing_key: &SigningKey,
    owner: &PublicKey,
) -> Result<(), StartupError> {
    use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
    use ffs_core::{EntityId, PredicateName};

    let agent_entity = EntityId::new(owner.to_multibase());
    let cap_predicate = PredicateName::new(ffs_core::capability::CAPABILITY_PREDICATE);
    let existing = store
        .list_by_entity(&agent_entity, Some(&cap_predicate), None)
        .map_err(|e| StartupError::Store(PathBuf::from("(self-grant lookup)"), Box::new(e)))?;
    if !existing.is_empty() {
        tracing::debug!(
            cap_count = existing.len(),
            "owner already has capability atoms; skipping self-grant bootstrap"
        );
        return Ok(());
    }

    let now = current_iso8601_for_bootstrap();
    let cap = build_capability_atom(
        signing_key,
        owner.clone(),
        vec![Action::Read, Action::Write, Action::Supersede],
        CapabilityScope::default(),
        now.clone(),
        None, // no expiry — sovereign self-grant
        now,
        None,
    )
    .map_err(|e| StartupError::BadKey(format!("sign self-grant: {e}")))?;
    let hash = store
        .insert(&cap)
        .map_err(|e| StartupError::Store(PathBuf::from("(self-grant insert)"), Box::new(e)))?;
    tracing::info!(
        cap_hash = %hash.to_multibase(),
        "bootstrapped owner self-capability (Read + Write + Supersede, unbounded scope)"
    );
    Ok(())
}

fn current_iso8601_for_bootstrap() -> ffs_core::Iso8601 {
    let now = time::OffsetDateTime::now_utc();
    let s = now
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
    ffs_core::Iso8601::new(&s)
        .unwrap_or_else(|_| ffs_core::Iso8601::new("1970-01-01T00:00:00Z").unwrap())
}

/// Parse the optional `FFS_SKILL_TIMEOUT_MS` env var into a
/// `Duration`. Returns `Ok(None)` when the variable is unset;
/// `Err` when it's set but not a positive integer.
fn parse_skill_timeout() -> Result<Option<std::time::Duration>, StartupError> {
    let Ok(raw) = std::env::var("FFS_SKILL_TIMEOUT_MS") else {
        return Ok(None);
    };
    let ms: u64 = raw.trim().parse().map_err(|_| {
        StartupError::BadKey(format!(
            "FFS_SKILL_TIMEOUT_MS must be a positive integer (got {raw:?})"
        ))
    })?;
    if ms == 0 {
        return Err(StartupError::BadKey(
            "FFS_SKILL_TIMEOUT_MS must be > 0".into(),
        ));
    }
    Ok(Some(std::time::Duration::from_millis(ms)))
}

fn resolve_data_dir() -> Result<PathBuf, StartupError> {
    if let Ok(explicit) = std::env::var("FFS_DATA_DIR") {
        return Ok(PathBuf::from(explicit));
    }
    let home = std::env::var_os("HOME").ok_or(StartupError::NoDataDir)?;
    Ok(PathBuf::from(home).join(".ffs"))
}

/// Where a key was loaded from. Shown in the startup log and by
/// `ffs identity show` so the user can confirm their identity is
/// stable + persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeySource {
    /// Read from the OS keychain (durable across reboots).
    Keychain,
    /// Read from the env-var (also persisted to the keychain on
    /// this boot when the keychain is enabled).
    EnvVar,
    /// Generated fresh this boot. Lost on restart unless captured.
    Fresh,
}

impl std::fmt::Display for KeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Keychain => "keychain",
            Self::EnvVar => "env_var",
            Self::Fresh => "fresh",
        };
        f.write_str(s)
    }
}

fn keyring_disabled() -> bool {
    matches!(
        std::env::var("FFS_KEYRING_DISABLE").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

fn keychain_owner_account() -> String {
    std::env::var("USER").unwrap_or_else(|_| "ffs".into())
}

/// Load the owner Ed25519 signing-key seed. Precedence:
///   1. `FFS_OWNER_KEY_HEX` env var (also persisted to keychain on
///      this boot when keychain is enabled).
///   2. OS keychain entry under `(ffs-owner-key, $USER)` —
///      `owner_key_from_keyring` reads-or-creates-and-persists.
///   3. Generate-and-warn (fresh seed each boot — only when
///      keychain is disabled AND the env var isn't set).
fn load_or_generate_owner_key() -> Result<(SigningKey, KeySource), StartupError> {
    if let Ok(hex_seed) = std::env::var("FFS_OWNER_KEY_HEX") {
        let seed = decode_hex_32(&hex_seed).map_err(StartupError::BadKey)?;
        if !keyring_disabled() {
            match ffs_core::store::save_key_to_keychain(
                ffs_core::store::OWNER_KEY_SERVICE,
                &keychain_owner_account(),
                &seed,
            ) {
                Ok(()) => tracing::info!(
                    "FFS_OWNER_KEY_HEX migrated to OS keychain; \
                     you can drop the env var on next boot"
                ),
                Err(e) => {
                    tracing::warn!(error = %e, "could not migrate FFS_OWNER_KEY_HEX to keychain")
                }
            }
        }
        return Ok((SigningKey::from_bytes(&seed), KeySource::EnvVar));
    }

    if !keyring_disabled() {
        match ffs_core::store::owner_key_from_keyring(
            ffs_core::store::OWNER_KEY_SERVICE,
            &keychain_owner_account(),
        ) {
            Ok(seed) => {
                tracing::debug!("owner signing key loaded from OS keychain");
                return Ok((SigningKey::from_bytes(&seed), KeySource::Keychain));
            }
            Err(e) => tracing::warn!(
                error = %e,
                "owner-key keychain lookup failed; falling through to generate-and-warn"
            ),
        }
    }

    use rand::RngCore;
    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    let key = SigningKey::from_bytes(&seed);
    let fp = Multihash::blake3_of(&seed).to_multibase();
    tracing::warn!(
        key_fp = %fp,
        "no owner-key source (env var unset, keychain disabled or unavailable) — \
         generated a fresh signing key. Existing atoms signed by other keys will \
         not validate. Unset FFS_KEYRING_DISABLE to enable keychain persistence."
    );
    Ok((key, KeySource::Fresh))
}

/// Load the SQLCipher DEK. Same precedence shape as
/// `load_or_generate_owner_key`. The DEK is keyed in the keychain by
/// the substrate's owner pubkey multibase so multi-substrate-per-
/// user setups can keep distinct DEKs.
fn load_or_generate_dek(
    data_dir: &Path,
    owner_pubkey_multibase: &str,
) -> Result<([u8; 32], KeySource), StartupError> {
    if let Ok(hex_seed) = std::env::var("FFS_SQLCIPHER_KEY_HEX") {
        let dek = decode_hex_32(&hex_seed).map_err(StartupError::BadDek)?;
        if !keyring_disabled() {
            match ffs_core::store::save_key_to_keychain(
                ffs_core::store::DEK_SERVICE,
                owner_pubkey_multibase,
                &dek,
            ) {
                Ok(()) => tracing::info!(
                    "FFS_SQLCIPHER_KEY_HEX migrated to OS keychain; \
                     you can drop the env var on next boot"
                ),
                Err(e) => {
                    tracing::warn!(error = %e, "could not migrate FFS_SQLCIPHER_KEY_HEX to keychain")
                }
            }
        }
        return Ok((dek, KeySource::EnvVar));
    }

    if !keyring_disabled() {
        match ffs_core::store::dek_from_keyring(
            ffs_core::store::DEK_SERVICE,
            owner_pubkey_multibase,
        ) {
            Ok(dek) => {
                tracing::debug!("SQLCipher DEK loaded from OS keychain");
                return Ok((dek, KeySource::Keychain));
            }
            Err(e) => tracing::warn!(
                error = %e,
                "DEK keychain lookup failed; falling through to generate-and-warn"
            ),
        }
    }

    use rand::RngCore;
    let mut dek = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut dek);
    let db_path = data_dir.join("atoms.db");
    if db_path.exists() {
        tracing::warn!(
            db_path = %db_path.display(),
            "no DEK source (env var unset, keychain disabled or unavailable) — \
             but atoms.db already exists. The fresh DEK will fail to open the \
             existing database. Either set FFS_SQLCIPHER_KEY_HEX to the original \
             value, restore the keychain entry, or remove atoms.db (DESTRUCTIVE)."
        );
    } else {
        let fp = Multihash::blake3_of(&dek).to_multibase();
        tracing::warn!(
            dek_fp = %fp,
            "no DEK source (env var unset, keychain disabled or unavailable) — \
             generated a fresh DEK. Save its value via env var or unset \
             FFS_KEYRING_DISABLE before next restart, or atoms.db will be \
             unrecoverable."
        );
    }
    Ok((dek, KeySource::Fresh))
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
