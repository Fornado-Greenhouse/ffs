// Spawns a UDS-bound daemon — Unix-only. Windows uses the named-pipe
// transport and its own integration tests.
#![cfg(unix)]

//! End-to-end CLI tests against a spawned daemon. We use the daemon's
//! library (`ffs_daemon::transport::serve`) to host a UDS endpoint, then
//! invoke the CLI's `run` function in-process. This avoids the cost of
//! shelling out to the binary while still exercising the full client →
//! UDS → daemon → response path.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use ffs_cli::{Args, Command, run};
use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::ProjectionRenderer;
use ffs_core::quarantine::InMemoryQuarantine;
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::working_set::InMemoryWorkingSet;
use ffs_core::{AtomTemplate, EntityId, Iso8601, Multihash, PredicateName, PublicKey, Tier};
use ffs_daemon::transport;
use ffs_daemon::{Dispatcher, EventPublisher};

const CONTACT_TOML: &str = r#"
name = "contact.person"
version = 1

[claim_schema]
type = "object"
required = ["display_name"]

[claim_schema.properties]
display_name = { type = "string" }

[rendering]
template = "contact-person.md.tera"
frontmatter_fields = ["display_name"]

[path]
family = "contacts"
name_field = "display_name"
"#;

const CONTACT_TEMPLATE: &str = r#"---
display_name: {{ claim.display_name }}
---
"#;

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[55u8; 32])
}

fn owner_pk() -> PublicKey {
    PublicKey::from_verifying(&owner_key().verifying_key())
}

struct Server {
    socket: std::path::PathBuf,
    cancel: CancellationToken,
    handle: tokio::task::JoinHandle<std::io::Result<()>>,
    _dir: tempfile::TempDir,
    store: Arc<dyn AtomStore>,
}

async fn spawn() -> Server {
    spawn_with(None).await
}

/// A courier-shaped daemon: the given invoker answers `courier.run`,
/// and the temp dir doubles as the data dir so `courier.status` can
/// read `ingest/.courier/last_run.json` when a test writes it.
async fn spawn_with(invoker: Option<Arc<dyn ffs_daemon::SkillInvoker>>) -> Server {
    spawn_full(invoker, None).await
}

/// A daemon that can sign (capability grants, accepts): the owner's key.
async fn spawn_signed() -> Server {
    spawn_full(None, Some(Arc::new(owner_key()))).await
}

async fn spawn_full(
    invoker: Option<Arc<dyn ffs_daemon::SkillInvoker>>,
    signing_key: Option<Arc<SigningKey>>,
) -> Server {
    let dir = tempfile::tempdir().unwrap();
    let predicates_dir = dir.path().join("predicates");
    let templates_dir = dir.path().join("templates");
    let run_dir = dir.path().join("run");
    std::fs::create_dir_all(&predicates_dir).unwrap();
    std::fs::create_dir_all(&templates_dir).unwrap();
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::set_permissions(&run_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(predicates_dir.join("contact.person.toml"), CONTACT_TOML).unwrap();
    std::fs::write(
        templates_dir.join("contact-person.md.tera"),
        CONTACT_TEMPLATE,
    )
    .unwrap();

    let registry = Arc::new(SpecRegistry::new());
    registry.load_dir(&predicates_dir).unwrap();
    let store: Arc<dyn AtomStore> = Arc::new(MemAtomStore::new());

    // Grant the daemon's owner full capability.
    let cap = build_capability_atom(
        &owner_key(),
        owner_pk(),
        vec![
            Action::Read,
            Action::Write,
            Action::Supersede,
            Action::Erase,
            Action::Classify,
            Action::Federate,
        ],
        CapabilityScope::default(),
        Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        None,
        Iso8601::new("2026-01-01T00:00:01Z").unwrap(),
        None,
    )
    .unwrap();
    store.insert(&cap).unwrap();

    let renderer =
        Arc::new(ProjectionRenderer::new(store.clone(), registry.clone(), &templates_dir).unwrap());
    let notifier = Arc::new(EventPublisher::new());
    let dispatcher = Arc::new(Dispatcher {
        store: store.clone(),
        registry,
        renderer,
        notifier,
        owner: owner_pk(),
        quarantine: Arc::new(InMemoryQuarantine::new()),
        scribe: None,
        working_set: Arc::new(InMemoryWorkingSet::new()),
        signing_key,
        federation_peers: Arc::new(ffs_core::federation_peers::InMemoryFederationPeerStore::new()),
        federation_client: None,
        our_cert_fingerprint: None,
        peer_mounts: Arc::new(ffs_federation::mount::InMemoryPeerMount::new()),
        data_dir: Some(dir.path().to_path_buf()),
        skill_invoker: invoker,
        ingest_agent_identity: None,
        suppression: None,
    });

    let socket = run_dir.join("ffs.sock");
    let cancel = CancellationToken::new();
    let dispatcher_clone = dispatcher.clone();
    let cancel_clone = cancel.clone();
    let sock = socket.clone();
    let handle =
        tokio::spawn(async move { transport::serve(&sock, dispatcher_clone, cancel_clone).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    Server {
        socket,
        cancel,
        handle,
        _dir: dir,
        store,
    }
}

fn insert_contact(store: &dyn AtomStore, entity: &str, name: &str, tx_time: &str) -> Multihash {
    let env = AtomTemplate {
        v: 1,
        entity: EntityId::new(entity),
        predicate: PredicateName::new("contact.person"),
        claim: serde_json::json!({"display_name": name}),
        valid_from: Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        valid_to: None,
        tx_time: Iso8601::new(tx_time).unwrap(),
        classification: Tier::new("existence"),
        supersedes: None,
        provenance: vec![],
    }
    .sign(&owner_key())
    .unwrap();
    store.insert(&env).unwrap()
}

#[tokio::test]
async fn cli_health_against_spawned_daemon() {
    let server = spawn().await;
    let args = Args {
        socket: Some(server.socket.clone()),
        json: false,
        command: Command::Health { briefing: false },
    };
    let out = run(args).await;
    assert_eq!(out.code, 0, "expected EXIT_OK; stderr was:\n{}", out.stderr);
    assert!(out.stdout.contains("proposals:"));
    assert!(out.stdout.contains("atom_count:"));

    server.cancel.cancel();
    let _ = timeout(Duration::from_secs(2), server.handle).await;
}

#[tokio::test]
async fn cli_cat_path_url_returns_projection_markdown() {
    let server = spawn().await;
    insert_contact(
        &*server.store,
        "Sarah_Chen",
        "Sarah Chen",
        "2026-05-25T10:00:00Z",
    );

    let args = Args {
        socket: Some(server.socket.clone()),
        json: false,
        command: Command::Cat {
            url: "ffs://local/contacts/by-name/S/Sarah_Chen.md".into(),
        },
    };
    let out = run(args).await;
    assert_eq!(out.code, 0, "expected EXIT_OK; stderr was:\n{}", out.stderr);
    assert!(
        out.stdout.contains("display_name: Sarah Chen"),
        "expected projection markdown; got:\n{}",
        out.stdout
    );

    server.cancel.cancel();
    let _ = timeout(Duration::from_secs(2), server.handle).await;
}

#[tokio::test]
async fn cli_ls_path_url_returns_recency_listing() {
    let server = spawn().await;
    insert_contact(&*server.store, "Alice", "Alice", "2026-05-25T08:00:00Z");
    insert_contact(&*server.store, "Bob", "Bob", "2026-05-25T09:00:00Z");
    insert_contact(&*server.store, "Carol", "Carol", "2026-05-25T10:00:00Z");

    let args = Args {
        socket: Some(server.socket.clone()),
        json: false,
        command: Command::Ls {
            url: "ffs://local/contacts/recent/".into(),
        },
    };
    let out = run(args).await;
    assert_eq!(out.code, 0, "expected EXIT_OK; stderr was:\n{}", out.stderr);
    // First entry should be the most-recently-touched: Carol (10:00) > Bob > Alice.
    let carol_pos = out.stdout.find("Carol").expect("Carol in listing");
    let bob_pos = out.stdout.find("Bob").expect("Bob in listing");
    let alice_pos = out.stdout.find("Alice").expect("Alice in listing");
    assert!(carol_pos < bob_pos && bob_pos < alice_pos);

    server.cancel.cancel();
    let _ = timeout(Duration::from_secs(2), server.handle).await;
}

#[tokio::test]
async fn cli_cat_with_as_of_returns_historical_state() {
    let server = spawn().await;
    insert_contact(
        &*server.store,
        "Sarah_Chen",
        "Sarah Chen",
        "2026-05-25T10:00:00Z",
    );
    let args = Args {
        socket: Some(server.socket.clone()),
        json: false,
        command: Command::Cat {
            // Render as-of an instant before the atom existed → projection
            // should error with AtomNotFound → EXIT_NOT_FOUND.
            url: "ffs://local/contacts/by-name/S/Sarah_Chen.md?as_of=2026-04-15T00:00:00Z".into(),
        },
    };
    let out = run(args).await;
    assert_eq!(
        out.code,
        ffs_cli::EXIT_NOT_FOUND,
        "expected EXIT_NOT_FOUND for historical-before-creation; stderr was:\n{}",
        out.stderr
    );

    server.cancel.cancel();
    let _ = timeout(Duration::from_secs(2), server.handle).await;
}

#[tokio::test]
async fn cli_capability_denied_exits_with_code_two() {
    // Don't grant capability to a special agent — but the CLI uses the
    // daemon's owner. To get capability denial, we ask for an atom whose
    // classification isn't covered. Simpler: use a wrong daemon owner via
    // a fresh server with a different cap.
    let dir = tempfile::tempdir().unwrap();
    let predicates_dir = dir.path().join("predicates");
    let templates_dir = dir.path().join("templates");
    let run_dir = dir.path().join("run");
    std::fs::create_dir_all(&predicates_dir).unwrap();
    std::fs::create_dir_all(&templates_dir).unwrap();
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::set_permissions(&run_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(predicates_dir.join("contact.person.toml"), CONTACT_TOML).unwrap();
    std::fs::write(
        templates_dir.join("contact-person.md.tera"),
        CONTACT_TEMPLATE,
    )
    .unwrap();

    let registry = Arc::new(SpecRegistry::new());
    registry.load_dir(&predicates_dir).unwrap();
    let store: Arc<dyn AtomStore> = Arc::new(MemAtomStore::new());
    // NO capability for owner_pk() — atoms exist but reads are denied.
    let hash = insert_contact(&*store, "alice", "Alice", "2026-05-25T08:00:00Z");
    let renderer =
        Arc::new(ProjectionRenderer::new(store.clone(), registry.clone(), &templates_dir).unwrap());
    let notifier = Arc::new(EventPublisher::new());
    let dispatcher = Arc::new(Dispatcher {
        store,
        registry,
        renderer,
        notifier,
        owner: owner_pk(),
        quarantine: Arc::new(InMemoryQuarantine::new()),
        scribe: None,
        working_set: Arc::new(InMemoryWorkingSet::new()),
        signing_key: None,
        federation_peers: Arc::new(ffs_core::federation_peers::InMemoryFederationPeerStore::new()),
        federation_client: None,
        our_cert_fingerprint: None,
        peer_mounts: Arc::new(ffs_federation::mount::InMemoryPeerMount::new()),
        data_dir: None,
        skill_invoker: None,
        ingest_agent_identity: None,
        suppression: None,
    });
    let socket = run_dir.join("ffs.sock");
    let cancel = CancellationToken::new();
    let dispatcher_clone = dispatcher.clone();
    let cancel_clone = cancel.clone();
    let sock = socket.clone();
    let handle =
        tokio::spawn(async move { transport::serve(&sock, dispatcher_clone, cancel_clone).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let args = Args {
        socket: Some(socket),
        json: false,
        command: Command::Get {
            url: format!("ffs://local/atom/{}", hash.to_multibase()),
        },
    };
    let out = run(args).await;
    assert_eq!(
        out.code,
        ffs_cli::EXIT_CAPABILITY_DENIED,
        "expected EXIT_CAPABILITY_DENIED; stderr was:\n{}",
        out.stderr
    );

    cancel.cancel();
    let _ = timeout(Duration::from_secs(2), handle).await;
    drop(dir);
}

#[tokio::test]
async fn cli_not_found_on_missing_atom_exits_with_code_three() {
    let server = spawn().await;
    let bogus = Multihash::blake3_of(b"never-existed");
    let args = Args {
        socket: Some(server.socket.clone()),
        json: false,
        command: Command::Get {
            url: format!("ffs://local/atom/{}", bogus.to_multibase()),
        },
    };
    let out = run(args).await;
    assert_eq!(
        out.code,
        ffs_cli::EXIT_NOT_FOUND,
        "expected EXIT_NOT_FOUND; stderr was:\n{}",
        out.stderr
    );

    server.cancel.cancel();
    let _ = timeout(Duration::from_secs(2), server.handle).await;
}

#[tokio::test]
async fn cli_get_atom_returns_canonical_envelope_json() {
    let server = spawn().await;
    let hash = insert_contact(&*server.store, "alice", "Alice", "2026-05-25T08:00:00Z");
    let args = Args {
        socket: Some(server.socket.clone()),
        json: false,
        command: Command::Get {
            url: format!("ffs://local/atom/{}", hash.to_multibase()),
        },
    };
    let out = run(args).await;
    assert_eq!(out.code, 0, "expected EXIT_OK; stderr was:\n{}", out.stderr);
    let v: serde_json::Value = serde_json::from_str(out.stdout.trim()).unwrap();
    assert_eq!(v["entity"], "alice");
    assert_eq!(v["predicate"], "contact.person");

    server.cancel.cancel();
    let _ = timeout(Duration::from_secs(2), server.handle).await;
}

#[tokio::test]
async fn cli_invalid_url_returns_usage_error() {
    let server = spawn().await;
    let args = Args {
        socket: Some(server.socket.clone()),
        json: false,
        command: Command::Cat {
            url: "https://not-an-ffs-url".into(),
        },
    };
    let out = run(args).await;
    assert_eq!(out.code, ffs_cli::EXIT_USAGE);

    server.cancel.cancel();
    let _ = timeout(Duration::from_secs(2), server.handle).await;
}

struct CourierStub;

#[async_trait::async_trait]
impl ffs_daemon::SkillInvoker for CourierStub {
    async fn invoke(
        &self,
        skill: &str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        assert_eq!(skill, "courier");
        let dry = input["dry_run"].as_bool().unwrap_or(false);
        Ok(serde_json::json!({
            "would_submit": if dry { vec!["ingest/2026-09-21-ledger-a.md", "ingest/2026-09-21-ledger-b.md"] } else { vec![] },
            "submitted": if dry { vec![] } else { vec!["ingest/2026-09-21-ledger-a.md"] },
            "items_seen": 2,
            "files_written": if dry { 0 } else { 1 },
            "fetch_failures": 0,
            "last_error": serde_json::Value::Null,
        }))
    }
}

#[tokio::test]
async fn courier_run_dry_run_prints_would_write_list() {
    let server = spawn_with(Some(Arc::new(CourierStub))).await;
    let args = Args {
        socket: Some(server.socket.clone()),
        json: false,
        command: Command::Courier {
            command: ffs_cli::CourierCommand::Run { dry_run: true },
        },
    };
    let out = run(args).await;
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert!(
        out.stdout.starts_with("would submit: 2\n"),
        "{}",
        out.stdout
    );
    assert!(out.stdout.contains("  ingest/2026-09-21-ledger-a.md\n"));
    assert!(out.stdout.contains("items_seen: 2\n"));
    assert!(out.stdout.contains("files_written: 0\n"));
    assert!(
        !out.stdout.contains("filed"),
        "dry run must say would submit, never filed"
    );
    server.cancel.cancel();
    let _ = timeout(Duration::from_secs(2), server.handle).await;
}

#[tokio::test]
async fn courier_status_reads_health_summary() {
    let server = spawn_with(Some(Arc::new(CourierStub))).await;
    // Before any tick: nothing to report.
    let out = run(Args {
        socket: Some(server.socket.clone()),
        json: false,
        command: Command::Courier {
            command: ffs_cli::CourierCommand::Status,
        },
    })
    .await;
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert_eq!(out.stdout, "courier has not run\n");
    // The skill's last_run.json is what both status and health read.
    let dir = server._dir.path().join("ingest").join(".courier");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("last_run.json"),
        r#"{"last_run":"2026-09-21T06:10:00Z","items_seen":14,"files_written":9,"fetch_failures":1,"last_error":null}"#,
    )
    .unwrap();
    let out = run(Args {
        socket: Some(server.socket.clone()),
        json: false,
        command: Command::Courier {
            command: ffs_cli::CourierCommand::Status,
        },
    })
    .await;
    assert_eq!(out.code, 0);
    assert!(out.stdout.contains("last_run: 2026-09-21T06:10:00Z\n"));
    assert!(out.stdout.contains("items_seen: 14\n"));
    assert!(out.stdout.contains("fetch_failures: 1\n"));
    let health = run(Args {
        socket: Some(server.socket.clone()),
        json: true,
        command: Command::Health { briefing: false },
    })
    .await;
    assert!(health.stdout.contains("\"courier\""), "{}", health.stdout);
    assert!(
        health.stdout.contains("\"files_written\": 9"),
        "{}",
        health.stdout
    );
    server.cancel.cancel();
    let _ = timeout(Duration::from_secs(2), server.handle).await;
}

// ---- task_39: capability grants from the CLI ----

fn grant_cmd(action: &str, max_per_day: Option<u32>, unlimited: bool) -> Command {
    Command::Capability {
        command: ffs_cli::CapabilityCommand::Grant {
            action: action.into(),
            grantee: "mcp:agent/courier".into(),
            predicates: vec!["contact.person".into()],
            classifications: vec![],
            max_per_day,
            unlimited,
            valid_to: None,
        },
    }
}

/// `ffs capability grant --action accept` without `--max-per-day` or
/// `--unlimited` is a usage error before any RPC (ADR-029: every accept
/// grant carries a cap).
#[tokio::test]
async fn grant_accept_without_cap_is_usage_error() {
    let server = spawn_signed().await;
    let out = run(Args {
        socket: Some(server.socket.clone()),
        json: false,
        command: grant_cmd("accept", None, false),
    })
    .await;
    assert_eq!(
        out.code, 64,
        "EXIT_USAGE; stdout:\n{}\nstderr:\n{}",
        out.stdout, out.stderr
    );
    assert!(out.stderr.contains("--max-per-day"), "{}", out.stderr);
    assert!(
        server
            .store
            .list_by_predicate(
                &PredicateName::new(ffs_core::capability::CAPABILITY_PREDICATE),
                None,
                100
            )
            .unwrap()
            .len()
            == 1,
        "only the owner's bootstrap grant exists"
    );

    // With a cap it goes through and lists with the cap and zero usage.
    let out = run(Args {
        socket: Some(server.socket.clone()),
        json: true,
        command: grant_cmd("accept", Some(50), false),
    })
    .await;
    assert_eq!(out.code, 0, "stderr:\n{}", out.stderr);
    let v: serde_json::Value = serde_json::from_str(out.stdout.trim()).unwrap();
    let grant_hash = v["grant_hash"].as_str().unwrap().to_string();
    let out = run(Args {
        socket: Some(server.socket.clone()),
        json: false,
        command: Command::Capability {
            command: ffs_cli::CapabilityCommand::List,
        },
    })
    .await;
    assert_eq!(out.code, 0, "stderr:\n{}", out.stderr);
    let line = out
        .stdout
        .lines()
        .find(|l| l.starts_with(&grant_hash))
        .unwrap_or_else(|| panic!("grant missing from list:\n{}", out.stdout));
    assert!(line.contains("actions=accept"), "{line}");
    assert!(line.contains("predicates=contact.person"), "{line}");
    assert!(line.contains("cap=50/day"), "{line}");
    assert!(line.contains("used_today=0"), "{line}");

    server.cancel.cancel();
    let _ = timeout(Duration::from_secs(2), server.handle).await;
}

/// `ffs capability revoke <hash>` authors a superseding capability atom
/// with no actions (ADR-007: revocation is supersession); the grant
/// leaves `capability list`.
#[tokio::test]
async fn revoke_authors_empty_action_supersession() {
    let server = spawn_signed().await;
    let out = run(Args {
        socket: Some(server.socket.clone()),
        json: true,
        command: grant_cmd("write", None, false),
    })
    .await;
    assert_eq!(out.code, 0, "stderr:\n{}", out.stderr);
    let v: serde_json::Value = serde_json::from_str(out.stdout.trim()).unwrap();
    let grant_hash = v["grant_hash"].as_str().unwrap().to_string();

    let out = run(Args {
        socket: Some(server.socket.clone()),
        json: false,
        command: Command::Capability {
            command: ffs_cli::CapabilityCommand::Revoke {
                grant_hash: grant_hash.clone(),
            },
        },
    })
    .await;
    assert_eq!(out.code, 0, "stderr:\n{}", out.stderr);
    assert!(
        out.stdout.contains(&format!("revoked: {grant_hash}")),
        "{}",
        out.stdout
    );
    let superseded_by = out
        .stdout
        .lines()
        .find_map(|l| l.strip_prefix("superseded_by: "))
        .unwrap()
        .trim()
        .to_string();

    let env = server
        .store
        .get(&Multihash::from_multibase(&superseded_by).unwrap())
        .unwrap()
        .expect("revocation atom stored");
    assert_eq!(
        env.supersedes.as_ref().map(|h| h.to_multibase()).as_deref(),
        Some(grant_hash.as_str())
    );
    let claim = ffs_core::capability::CapabilityClaim::from_envelope(&env).unwrap();
    assert!(claim.actions.is_empty(), "revocation carries no actions");

    let out = run(Args {
        socket: Some(server.socket.clone()),
        json: false,
        command: Command::Capability {
            command: ffs_cli::CapabilityCommand::List,
        },
    })
    .await;
    assert_eq!(out.code, 0);
    assert!(
        !out.stdout.contains(&grant_hash) && !out.stdout.contains(&superseded_by),
        "neither the grant nor its revocation is active:\n{}",
        out.stdout
    );

    server.cancel.cancel();
    let _ = timeout(Duration::from_secs(2), server.handle).await;
}
