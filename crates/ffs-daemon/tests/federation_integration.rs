//! Two-daemon federation handshake scenario.
//!
//! Stands up two `Dispatcher`s in the same process, wires them
//! together via `InMemoryFederationClient`s (per TechSpec § Unit
//! Tests — federation transport is trait-mocked, not run over real
//! TLS), and exercises the full bridge flow:
//!
//! 1. Each side generates its TLS cert + fingerprint from its
//!    Ed25519 signing key (rcgen, deterministic).
//! 2. Each side calls `federation.peer.add` with the *other* side's
//!    fingerprint + pubkey — simulating the out-of-band exchange.
//! 3. Alice calls `bridge.establish` → Bob's server handler updates
//!    its peer record with Alice's capability hash + vocab.
//! 4. Both daemons' `federation.peer.list` reflects the bridge.
//! 5. Inbound handshake with an unregistered cert fingerprint is
//!    rejected (the substrate-of-mTLS check at the trait layer).
//! 6. `bridge.rotate` swaps Alice's pinned fingerprint on Bob's side
//!    when Alice presents a valid old-key signature.

use std::sync::Arc;

use ed25519_dalek::{Signer, SigningKey};
use time::OffsetDateTime;

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::federation_peers::{FederationPeerStore, InMemoryFederationPeerStore};
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::ProjectionRenderer;
use ffs_core::quarantine::InMemoryQuarantine;
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::working_set::InMemoryWorkingSet;
use ffs_core::{Iso8601, Multihash, PublicKey};
use ffs_daemon::{ApiPayload, ApiRequest, ApiResponse, Dispatcher, EventPublisher};
use ffs_federation::client::InMemoryFederationClient;
use ffs_federation::handshake::rotation_signing_bytes;
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

fn grant_federate(store: &dyn AtomStore, owner_key: &SigningKey, owner_pk: PublicKey) {
    let cap = build_capability_atom(
        owner_key,
        owner_pk,
        vec![Action::Read, Action::Write, Action::Federate],
        CapabilityScope::default(),
        Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        None,
        Iso8601::new("2026-01-01T00:00:01Z").unwrap(),
        None,
    )
    .unwrap();
    store.insert(&cap).unwrap();
}

struct Side {
    name: &'static str,
    key: SigningKey,
    cert: SubstrateCertificate,
    peers: Arc<InMemoryFederationPeerStore>,
    _store: Arc<dyn AtomStore>,
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
    grant_federate(&*store, &key, pk_of(&key));
    let renderer =
        Arc::new(ProjectionRenderer::new(store.clone(), registry.clone(), &templates_dir).unwrap());
    let notifier = Arc::new(EventPublisher::new());
    let peers = Arc::new(InMemoryFederationPeerStore::new());
    let cert = generate_from_signing_key(&key, fixed_now()).unwrap();

    let fed_context = FederationContext {
        responder_pubkey: pk_of(&key),
        responder_capability: Multihash::blake3_of(format!("cap-of-{name}").as_bytes()),
        responder_vocab: vec!["contact.person".into(), "note".into()],
        responder_anchor: Iso8601::new("2026-05-27T08:00:00Z").unwrap(),
        peers: peers.clone() as Arc<dyn FederationPeerStore>,
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
        federation_client: None, // set after wiring routes
        our_cert_fingerprint: Some(cert.fingerprint.clone()),
    };
    Side {
        name,
        key,
        cert,
        peers,
        _store: store,
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

async fn pin_peer(
    side: &Side,
    peer_id: &str,
    peer_pubkey: PublicKey,
    peer_endpoint: &str,
    peer_fingerprint: Multihash,
) {
    let req = ApiRequest {
        jsonrpc: "2.0".into(),
        id: serde_json::json!(1),
        method: "federation.peer.add".into(),
        params: serde_json::json!({
            "peer_id": peer_id,
            "peer_pubkey": peer_pubkey,
            "endpoint": peer_endpoint,
            "fingerprint": peer_fingerprint,
        }),
    };
    let resp = side.dispatcher.handle(req).await;
    assert!(
        matches!(resp.payload, ApiPayload::Success { .. }),
        "{} federation.peer.add failed: {:?}",
        side.name,
        resp.payload
    );
}

fn unwrap_ok(resp: ApiResponse) -> serde_json::Value {
    match resp.payload {
        ApiPayload::Success { result } => result,
        ApiPayload::Error { error } => panic!("expected success; got {error:?}"),
    }
}

#[tokio::test]
async fn two_daemons_complete_bilateral_bridge_handshake() {
    let mut alice = make_side("alice", 51);
    let bob = make_side("bob", 52);

    // Wire Alice's federation_client to route Bob's endpoint to Bob's
    // server context. Production: reqwest hits the URL over the
    // network. In-memory: the InMemoryFederationClient dispatches
    // straight into Bob's FederationContext handlers.
    wire_client(&mut alice, BOB_ENDPOINT, bob.fed_context.clone()).await;

    // Out-of-band fingerprint exchange. Each side pins the other.
    pin_peer(
        &alice,
        "bob",
        pk_of(&bob.key),
        BOB_ENDPOINT,
        bob.cert.fingerprint.clone(),
    )
    .await;
    pin_peer(
        &bob,
        "alice",
        pk_of(&alice.key),
        ALICE_ENDPOINT,
        alice.cert.fingerprint.clone(),
    )
    .await;

    // Alice initiates the in-band handshake.
    let alice_capability = Multihash::blake3_of(b"alice-grants-bob");
    let resp = alice
        .dispatcher
        .handle(ApiRequest {
            jsonrpc: "2.0".into(),
            id: serde_json::json!(1),
            method: "bridge.establish".into(),
            params: serde_json::json!({
                "peer_id": "bob",
                "our_capability": alice_capability,
                "our_vocab": ["contact.person", "note"],
            }),
        })
        .await;
    let result = unwrap_ok(resp);
    let their_cap: Multihash = serde_json::from_value(result["their_capability"].clone()).unwrap();
    assert!(
        result["their_vocab"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "contact.person"),
        "their vocab should include contact.person; got {result}"
    );

    // Alice's peer record now reflects the bridge.
    let alice_pin = alice.peers.get("bob").await.unwrap();
    assert_eq!(alice_pin.our_capability, Some(alice_capability.clone()));
    assert_eq!(alice_pin.their_capability, Some(their_cap.clone()));

    // Bob's peer record was updated during the inbound handshake.
    let bob_view_of_alice = bob.peers.get("alice").await.unwrap();
    assert_eq!(bob_view_of_alice.their_capability, Some(alice_capability));
    assert_eq!(
        bob_view_of_alice.vocab,
        vec!["contact.person".to_string(), "note".to_string()]
    );

    // federation.peer.list reports the bridge on both sides.
    let alice_peers = unwrap_ok(
        alice
            .dispatcher
            .handle(ApiRequest {
                jsonrpc: "2.0".into(),
                id: serde_json::json!(2),
                method: "federation.peer.list".into(),
                params: serde_json::Value::Null,
            })
            .await,
    );
    assert_eq!(alice_peers.as_array().unwrap().len(), 1);
    assert_eq!(alice_peers[0]["peer_id"], "bob");
}

#[tokio::test]
async fn handshake_with_unregistered_fingerprint_is_rejected_at_in_memory_layer() {
    let mut alice = make_side("alice", 53);
    let bob = make_side("bob", 54);
    wire_client(&mut alice, BOB_ENDPOINT, bob.fed_context.clone()).await;

    // Alice does NOT pin Bob on Bob's side (only pins on her own
    // side). When she initiates the handshake, Bob's handler sees
    // her cert fingerprint as unregistered and rejects.
    pin_peer(
        &alice,
        "bob",
        pk_of(&bob.key),
        BOB_ENDPOINT,
        bob.cert.fingerprint.clone(),
    )
    .await;

    let resp = alice
        .dispatcher
        .handle(ApiRequest {
            jsonrpc: "2.0".into(),
            id: serde_json::json!(1),
            method: "bridge.establish".into(),
            params: serde_json::json!({
                "peer_id": "bob",
                "our_capability": Multihash::blake3_of(b"x"),
                "our_vocab": ["contact.person"],
            }),
        })
        .await;
    match resp.payload {
        ApiPayload::Error { error } => {
            assert!(
                error.message.contains("UnregisteredPeer")
                    || error.message.contains("unregistered"),
                "expected unregistered-peer error; got {}",
                error.message
            );
        }
        ApiPayload::Success { result } => {
            panic!("expected rejection; got success: {result}");
        }
    }
}

#[tokio::test]
async fn bridge_rotate_swaps_alices_pin_on_bobs_side() {
    let mut alice = make_side("alice", 55);
    let bob = make_side("bob", 56);
    wire_client(&mut alice, BOB_ENDPOINT, bob.fed_context.clone()).await;

    pin_peer(
        &alice,
        "bob",
        pk_of(&bob.key),
        BOB_ENDPOINT,
        bob.cert.fingerprint.clone(),
    )
    .await;
    pin_peer(
        &bob,
        "alice",
        pk_of(&alice.key),
        ALICE_ENDPOINT,
        alice.cert.fingerprint.clone(),
    )
    .await;

    // Alice generates a new cert (in production: after a key
    // rotation; here we just hash a different blob to simulate).
    let new_fingerprint = Multihash::blake3_of(b"alice-new-cert");
    let resp = alice
        .dispatcher
        .handle(ApiRequest {
            jsonrpc: "2.0".into(),
            id: serde_json::json!(1),
            method: "bridge.rotate".into(),
            params: serde_json::json!({
                "peer_id": "bob",
                "new_fingerprint": new_fingerprint,
            }),
        })
        .await;
    let result = unwrap_ok(resp);
    assert_eq!(result["accepted"], true);

    // Bob's pinned fingerprint for alice should now be the new one.
    let bob_pin = bob.peers.get("alice").await.unwrap();
    assert_eq!(bob_pin.cert_fingerprint, new_fingerprint);
}

#[tokio::test]
async fn rotate_signature_check_independent_of_dispatcher() {
    // Belt-and-suspenders: prove the signing-bytes helper produces
    // bytes that verify against the corresponding key. (The actual
    // rotation flow above exercises this path; this assertion makes
    // a debugging regression obvious.)
    let key = SigningKey::from_bytes(&[57u8; 32]);
    let old_fp = Multihash::blake3_of(b"old");
    let new_fp = Multihash::blake3_of(b"new");
    let signed = rotation_signing_bytes(&old_fp, &new_fp);
    let sig = key.sign(&signed);
    key.verifying_key().verify_strict(&signed, &sig).unwrap();
    // Sanity: store the cert too so the binary's deterministic-gen
    // path is referenced.
    let cert = generate_from_signing_key(&key, fixed_now()).unwrap();
    assert!(cert.der.len() > 100);
}

// Suppress unused-field warnings for fields kept for documentation
// purposes (store, peers, key on Bob — referenced by name in
// assertions through the struct above).
#[allow(dead_code)]
fn _docs(_: &Side) {}
