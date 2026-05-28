//! Two-daemon federation pull scenario tests.
//!
//! Stands up Alice + Bob in the same process, wires them via
//! `InMemoryFederationClient` (per TechSpec — federation transport
//! is trait-mocked in tests), grants Alice→Bob a scoped capability,
//! and exercises:
//!
//! - Tier-based selective sharing: a capability scoped to
//!   `existence` only crosses existence-classified atoms; widening
//!   to `work_email` includes work_email atoms.
//! - Intersection: when both sides have entity X, the intersection
//!   query reports it; when only one side has Y, it doesn't.
//! - Revocation: superseding the capability with a no-actions
//!   replacement empties the next pull, and the receiver unmounts.
//! - Performance: pulling 100 atoms completes well under the 5s
//!   budget the requirements call out.
//!
//! Each scenario is a `#[tokio::test]` so they can run in parallel.

use std::sync::Arc;

use ed25519_dalek::SigningKey;
use time::OffsetDateTime;

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::federation_peers::{
    FederationPeer, FederationPeerStore, InMemoryFederationPeerStore,
};
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::ProjectionRenderer;
use ffs_core::quarantine::InMemoryQuarantine;
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::working_set::InMemoryWorkingSet;
use ffs_core::{AtomTemplate, EntityId, Iso8601, Multihash, PredicateName, PublicKey, Tier};
use ffs_daemon::{ApiPayload, ApiRequest, ApiResponse, Dispatcher, EventPublisher};
use ffs_federation::client::InMemoryFederationClient;
use ffs_federation::mount::{InMemoryPeerMount, PeerMountStore};
use ffs_federation::server::FederationContext;
use ffs_federation::{SubstrateCertificate, generate_from_signing_key};

const ALICE_ENDPOINT: &str = "https://alice.example/federation/v1";
const BOB_ENDPOINT: &str = "https://bob.example/federation/v1";

fn pk_of(k: &SigningKey) -> PublicKey {
    PublicKey::from_verifying(&k.verifying_key())
}

fn fixed_now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_780_000_000).unwrap()
}

fn ts(s: &str) -> Iso8601 {
    Iso8601::new(s).unwrap()
}

/// Grant the substrate's owner full local capability (so they can
/// read their own atoms via the dispatcher).
fn grant_owner_full(store: &dyn AtomStore, owner_key: &SigningKey, owner_pk: PublicKey) {
    let cap = build_capability_atom(
        owner_key,
        owner_pk,
        vec![
            Action::Read,
            Action::Write,
            Action::Supersede,
            Action::Federate,
        ],
        CapabilityScope::default(),
        ts("2026-01-01T00:00:00Z"),
        None,
        ts("2026-01-01T00:00:01Z"),
        None,
    )
    .unwrap();
    store.insert(&cap).unwrap();
}

/// Grant `grantee` a capability scoped to `classifications` (the
/// tier-based sharing axis the scenario tests focus on). Returns
/// the granted capability atom's hash so the peer can pin it.
fn grant_peer_classifications(
    store: &dyn AtomStore,
    grantor_key: &SigningKey,
    grantee_pk: PublicKey,
    classifications: Vec<&str>,
    tx_time: &str,
    supersedes: Option<Multihash>,
) -> Multihash {
    let scope = CapabilityScope {
        classifications: Some(classifications.into_iter().map(Tier::new).collect()),
        ..Default::default()
    };
    let cap = build_capability_atom(
        grantor_key,
        grantee_pk,
        vec![Action::Read],
        scope,
        ts("2026-01-01T00:00:00Z"),
        None,
        ts(tx_time),
        supersedes,
    )
    .unwrap();
    store.insert(&cap).unwrap()
}

fn insert_contact(
    store: &dyn AtomStore,
    author_key: &SigningKey,
    entity: &str,
    name: &str,
    classification: &str,
    tx_time: &str,
) -> Multihash {
    let env = AtomTemplate {
        v: 1,
        entity: EntityId::new(entity),
        predicate: PredicateName::new("contact.person"),
        claim: serde_json::json!({"display_name": name}),
        valid_from: ts("2026-01-01T00:00:00Z"),
        valid_to: None,
        tx_time: ts(tx_time),
        classification: Tier::new(classification),
        supersedes: None,
        provenance: vec![],
    }
    .sign(author_key)
    .unwrap();
    store.insert(&env).unwrap()
}

struct Side {
    _name: &'static str,
    key: SigningKey,
    cert: SubstrateCertificate,
    peers: Arc<InMemoryFederationPeerStore>,
    store: Arc<dyn AtomStore>,
    mounts: Arc<InMemoryPeerMount>,
    dispatcher: Dispatcher,
    fed_context: FederationContext,
    _dir: tempfile::TempDir,
}

fn make_side(name: &'static str, seed: u8) -> Side {
    let dir = tempfile::tempdir().unwrap();
    let predicates_dir = dir.path().join("predicates");
    let templates_dir = dir.path().join("templates");
    std::fs::create_dir_all(&predicates_dir).unwrap();
    std::fs::create_dir_all(&templates_dir).unwrap();
    let registry = Arc::new(SpecRegistry::new());
    registry.load_dir(&predicates_dir).unwrap();
    let store: Arc<dyn AtomStore> = Arc::new(MemAtomStore::new());
    let key = SigningKey::from_bytes(&[seed; 32]);
    grant_owner_full(&*store, &key, pk_of(&key));
    let renderer =
        Arc::new(ProjectionRenderer::new(store.clone(), registry.clone(), &templates_dir).unwrap());
    let notifier = Arc::new(EventPublisher::new());
    let peers = Arc::new(InMemoryFederationPeerStore::new());
    let mounts = Arc::new(InMemoryPeerMount::new());
    let cert = generate_from_signing_key(&key, fixed_now()).unwrap();

    let fed_context = FederationContext {
        responder_pubkey: pk_of(&key),
        responder_capability: Multihash::blake3_of(format!("cap-of-{name}").as_bytes()),
        responder_vocab: vec!["contact.person".into(), "note".into()],
        responder_anchor: ts("2026-05-27T08:00:00Z"),
        peers: peers.clone() as Arc<dyn FederationPeerStore>,
        store: store.clone(),
    };
    let dispatcher = Dispatcher {
        store: store.clone(),
        registry,
        renderer,
        notifier,
        owner: pk_of(&key),
        quarantine: Arc::new(InMemoryQuarantine::new()),
        scribe: None,
        working_set: Arc::new(InMemoryWorkingSet::new()),
        signing_key: Some(Arc::new(key.clone())),
        federation_peers: peers.clone(),
        federation_client: None,
        our_cert_fingerprint: Some(cert.fingerprint.clone()),
        peer_mounts: mounts.clone() as Arc<dyn PeerMountStore>,
    };
    Side {
        _name: name,
        key,
        cert,
        peers,
        store,
        mounts,
        dispatcher,
        fed_context,
        _dir: dir,
    }
}

async fn wire_client(side: &mut Side, peer_endpoint: &str, peer_ctx: FederationContext) {
    let client = InMemoryFederationClient::new();
    client.route(peer_endpoint.to_string(), peer_ctx).await;
    side.dispatcher.federation_client = Some(Arc::new(client));
}

async fn pin_peer_with_capability(
    side: &Side,
    peer_id: &str,
    peer_pubkey: PublicKey,
    peer_endpoint: &str,
    peer_fingerprint: Multihash,
    their_capability: Option<Multihash>,
) {
    let peer = FederationPeer {
        peer_id: peer_id.to_string(),
        peer_pubkey,
        endpoint: peer_endpoint.to_string(),
        cert_fingerprint: peer_fingerprint,
        our_capability: None,
        their_capability,
        vocab: vec!["contact.person".into()],
        watermarks: Default::default(),
        established_at: ts("2026-05-27T08:00:00Z"),
        last_seen_at: None,
    };
    side.peers.upsert(peer).await.unwrap();
}

fn unwrap_ok(resp: ApiResponse) -> serde_json::Value {
    match resp.payload {
        ApiPayload::Success { result } => result,
        ApiPayload::Error { error } => panic!("expected success; got {error:?}"),
    }
}

async fn pull(side: &Side, peer_id: &str) -> serde_json::Value {
    let resp = side
        .dispatcher
        .handle(ApiRequest {
            jsonrpc: "2.0".into(),
            id: serde_json::json!(1),
            method: "federation.pull".into(),
            params: serde_json::json!({"peer_id": peer_id}),
        })
        .await;
    unwrap_ok(resp)
}

// ---------------------------------------------------------------------
// Scenario 1: tier-based selective sharing (existence → work_email)
// ---------------------------------------------------------------------

#[tokio::test]
async fn existence_capability_crosses_only_existence_atoms() {
    // Alice authors capability → Bob (existence tier only); Bob pulls.
    let alice = make_side("alice", 71);
    let mut bob = make_side("bob", 72);

    // Alice's substrate contains a mix of classifications.
    insert_contact(
        &*alice.store,
        &alice.key,
        "Sara_Chen",
        "Sara",
        "existence",
        "2026-05-27T08:00:00Z",
    );
    insert_contact(
        &*alice.store,
        &alice.key,
        "Sara_Chen",
        "sara@work.example",
        "work_email",
        "2026-05-27T08:01:00Z",
    );

    // Alice grants Bob an existence-only capability.
    let cap_hash = grant_peer_classifications(
        &*alice.store,
        &alice.key,
        pk_of(&bob.key),
        vec!["existence"],
        "2026-05-27T07:00:00Z",
        None,
    );

    wire_client(&mut bob, ALICE_ENDPOINT, alice.fed_context.clone()).await;
    pin_peer_with_capability(
        &bob,
        "alice",
        pk_of(&alice.key),
        ALICE_ENDPOINT,
        alice.cert.fingerprint.clone(),
        Some(cap_hash),
    )
    .await;
    pin_peer_with_capability(
        &alice,
        "bob",
        pk_of(&bob.key),
        BOB_ENDPOINT,
        bob.cert.fingerprint.clone(),
        None,
    )
    .await;

    let result = pull(&bob, "alice").await;
    assert_eq!(result["atoms_pulled"], 1, "got: {result}");
    // Bob's mount is attributed to alice.
    assert_eq!(bob.mounts.count("alice").await, 1);

    // Verify the pulled atom is the existence one — by classification.
    let pulled = bob
        .store
        .list_by_entity(
            &EntityId::new("Sara_Chen"),
            Some(&PredicateName::new("contact.person")),
            None,
        )
        .unwrap();
    assert_eq!(pulled.len(), 1);
    assert_eq!(pulled[0].classification.as_str(), "existence");
}

#[tokio::test]
async fn widening_capability_to_work_email_pulls_additional_atom() {
    let alice = make_side("alice", 73);
    let mut bob = make_side("bob", 74);

    insert_contact(
        &*alice.store,
        &alice.key,
        "Sara_Chen",
        "Sara",
        "existence",
        "2026-05-27T08:00:00Z",
    );
    insert_contact(
        &*alice.store,
        &alice.key,
        "Sara_Chen",
        "sara@work.example",
        "work_email",
        "2026-05-27T08:01:00Z",
    );
    insert_contact(
        &*alice.store,
        &alice.key,
        "Sara_Chen",
        "sara@personal.example",
        "personal_email",
        "2026-05-27T08:02:00Z",
    );

    // Capability widened to existence + work_email (but NOT personal_email).
    let cap_hash = grant_peer_classifications(
        &*alice.store,
        &alice.key,
        pk_of(&bob.key),
        vec!["existence", "work_email"],
        "2026-05-27T07:00:00Z",
        None,
    );

    wire_client(&mut bob, ALICE_ENDPOINT, alice.fed_context.clone()).await;
    pin_peer_with_capability(
        &bob,
        "alice",
        pk_of(&alice.key),
        ALICE_ENDPOINT,
        alice.cert.fingerprint.clone(),
        Some(cap_hash),
    )
    .await;
    pin_peer_with_capability(
        &alice,
        "bob",
        pk_of(&bob.key),
        BOB_ENDPOINT,
        bob.cert.fingerprint.clone(),
        None,
    )
    .await;

    let result = pull(&bob, "alice").await;
    assert_eq!(
        result["atoms_pulled"], 2,
        "expected existence + work_email; got: {result}"
    );

    let pulled = bob
        .store
        .list_by_entity(
            &EntityId::new("Sara_Chen"),
            Some(&PredicateName::new("contact.person")),
            None,
        )
        .unwrap();
    let classes: std::collections::HashSet<_> = pulled
        .iter()
        .map(|a| a.classification.as_str().to_string())
        .collect();
    assert!(classes.contains("existence"));
    assert!(classes.contains("work_email"));
    assert!(!classes.contains("personal_email"));
}

// ---------------------------------------------------------------------
// Scenario 2: intersection
// ---------------------------------------------------------------------

#[tokio::test]
async fn intersection_query_returns_true_when_both_sides_have_entity() {
    let alice = make_side("alice", 75);
    let mut bob = make_side("bob", 76);

    // Both have Sara.
    insert_contact(
        &*alice.store,
        &alice.key,
        "Sara",
        "Sara",
        "existence",
        "2026-05-27T08:00:00Z",
    );
    insert_contact(
        &*bob.store,
        &bob.key,
        "Sara",
        "Sara",
        "existence",
        "2026-05-27T08:00:00Z",
    );

    let cap_hash = grant_peer_classifications(
        &*alice.store,
        &alice.key,
        pk_of(&bob.key),
        vec!["existence"],
        "2026-05-27T07:00:00Z",
        None,
    );
    wire_client(&mut bob, ALICE_ENDPOINT, alice.fed_context.clone()).await;
    pin_peer_with_capability(
        &bob,
        "alice",
        pk_of(&alice.key),
        ALICE_ENDPOINT,
        alice.cert.fingerprint.clone(),
        Some(cap_hash),
    )
    .await;
    // Alice must also pin Bob for the inbound mTLS check to pass.
    pin_peer_with_capability(
        &alice,
        "bob",
        pk_of(&bob.key),
        BOB_ENDPOINT,
        bob.cert.fingerprint.clone(),
        None,
    )
    .await;

    // Bob asks Alice "do you have Sara?" via the federation client
    // directly (the trait surface is what the production CLI will
    // hit through a future `bridge.intersection` RPC).
    let client = bob.dispatcher.federation_client.as_ref().unwrap().clone();
    let resp = client
        .intersection(
            ALICE_ENDPOINT,
            &bob.cert.fingerprint,
            &EntityId::new("Sara"),
        )
        .await
        .unwrap();
    assert!(resp.present, "expected Alice to report Sara is present");
    assert_eq!(resp.responder_pubkey, pk_of(&alice.key));
}

#[tokio::test]
async fn intersection_query_returns_false_when_peer_lacks_entity() {
    let alice = make_side("alice", 77);
    let mut bob = make_side("bob", 78);

    // Only Bob has Y; Alice doesn't.
    insert_contact(
        &*bob.store,
        &bob.key,
        "Y",
        "Y",
        "existence",
        "2026-05-27T08:00:00Z",
    );

    let cap_hash = grant_peer_classifications(
        &*alice.store,
        &alice.key,
        pk_of(&bob.key),
        vec!["existence"],
        "2026-05-27T07:00:00Z",
        None,
    );
    wire_client(&mut bob, ALICE_ENDPOINT, alice.fed_context.clone()).await;
    pin_peer_with_capability(
        &bob,
        "alice",
        pk_of(&alice.key),
        ALICE_ENDPOINT,
        alice.cert.fingerprint.clone(),
        Some(cap_hash),
    )
    .await;
    pin_peer_with_capability(
        &alice,
        "bob",
        pk_of(&bob.key),
        BOB_ENDPOINT,
        bob.cert.fingerprint.clone(),
        None,
    )
    .await;

    let client = bob.dispatcher.federation_client.as_ref().unwrap().clone();
    let resp = client
        .intersection(ALICE_ENDPOINT, &bob.cert.fingerprint, &EntityId::new("Y"))
        .await
        .unwrap();
    assert!(!resp.present);
}

// ---------------------------------------------------------------------
// Scenario 3: revocation = unmount
// ---------------------------------------------------------------------

#[tokio::test]
async fn revocation_supersedes_capability_and_next_pull_unmounts() {
    let alice = make_side("alice", 79);
    let mut bob = make_side("bob", 80);

    insert_contact(
        &*alice.store,
        &alice.key,
        "Sara",
        "Sara",
        "existence",
        "2026-05-27T08:00:00Z",
    );

    // Initial capability: existence.
    let initial_cap = grant_peer_classifications(
        &*alice.store,
        &alice.key,
        pk_of(&bob.key),
        vec!["existence"],
        "2026-05-27T07:00:00Z",
        None,
    );

    wire_client(&mut bob, ALICE_ENDPOINT, alice.fed_context.clone()).await;
    pin_peer_with_capability(
        &bob,
        "alice",
        pk_of(&alice.key),
        ALICE_ENDPOINT,
        alice.cert.fingerprint.clone(),
        Some(initial_cap.clone()),
    )
    .await;
    pin_peer_with_capability(
        &alice,
        "bob",
        pk_of(&bob.key),
        BOB_ENDPOINT,
        bob.cert.fingerprint.clone(),
        None,
    )
    .await;

    // First pull yields the existence atom.
    let first = pull(&bob, "alice").await;
    assert_eq!(first["atoms_pulled"], 1);
    assert_eq!(bob.mounts.count("alice").await, 1);

    // Alice supersedes the capability with one that grants no
    // classifications — effectively revocation. (build_capability
    // requires at least one action; a tier intersection of empty is
    // the narrowest scope that still parses.)
    let revoked = grant_peer_classifications(
        &*alice.store,
        &alice.key,
        pk_of(&bob.key),
        vec![], // empty classification list = no atoms covered
        "2026-05-27T09:00:00Z",
        Some(initial_cap),
    );

    // Bob's pinned capability hash hasn't moved; the source's
    // evaluator now sees the supersession and filters everything out.
    // Re-pin Bob to the revoked capability so the daemon looks the
    // right atom up. (In production, Bob would learn the new
    // capability via a `revocation-notice` push or detect the empty
    // result and re-handshake; both surface the new cap_hash.)
    let mut bob_view = bob.peers.get("alice").await.unwrap();
    bob_view.their_capability = Some(revoked);
    bob.peers.upsert(bob_view).await.unwrap();

    let second = pull(&bob, "alice").await;
    assert_eq!(second["atoms_pulled"], 0);
    assert_eq!(
        second["revoked"], true,
        "previously-yielding pull now empty should fire revoked: {second}"
    );
    assert_eq!(
        bob.mounts.count("alice").await,
        0,
        "mount should be cleared after revocation"
    );
}

// ---------------------------------------------------------------------
// Scenario 4: performance — pulling 100 atoms in well under 5s
// ---------------------------------------------------------------------

#[tokio::test]
async fn pulling_one_hundred_atoms_completes_well_under_five_seconds() {
    let alice = make_side("alice", 81);
    let mut bob = make_side("bob", 82);

    // Alice authors 100 contact.person atoms, all existence tier.
    for i in 0..100 {
        let entity = format!("person_{i:03}");
        let tx_time = format!("2026-05-27T08:{:02}:{:02}Z", i / 60, i % 60);
        insert_contact(
            &*alice.store,
            &alice.key,
            &entity,
            &entity,
            "existence",
            &tx_time,
        );
    }
    let cap_hash = grant_peer_classifications(
        &*alice.store,
        &alice.key,
        pk_of(&bob.key),
        vec!["existence"],
        "2026-05-27T07:00:00Z",
        None,
    );

    wire_client(&mut bob, ALICE_ENDPOINT, alice.fed_context.clone()).await;
    pin_peer_with_capability(
        &bob,
        "alice",
        pk_of(&alice.key),
        ALICE_ENDPOINT,
        alice.cert.fingerprint.clone(),
        Some(cap_hash),
    )
    .await;
    pin_peer_with_capability(
        &alice,
        "bob",
        pk_of(&bob.key),
        BOB_ENDPOINT,
        bob.cert.fingerprint.clone(),
        None,
    )
    .await;

    let start = std::time::Instant::now();
    let result = pull(&bob, "alice").await;
    let elapsed = start.elapsed();

    assert_eq!(result["atoms_pulled"], 100, "got: {result}");
    assert_eq!(bob.mounts.count("alice").await, 100);
    // 5s budget per the success criteria. In-memory transport runs in
    // microseconds; this assertion catches an O(n^2) regression in
    // the source-side capability evaluator.
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "100-atom pull took {elapsed:?} — over the 5s budget"
    );
}

// ---------------------------------------------------------------------
// Plumbing fields kept for documentation
// ---------------------------------------------------------------------

#[allow(dead_code)]
fn _docs(_: &Side) {}
