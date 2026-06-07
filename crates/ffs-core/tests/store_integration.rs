//! Integration tests for the atom store: parity between MemAtomStore and
//! SqliteAtomStore, supersession-tree head selection, bitemporal queries,
//! FTS5 search, SQLCipher key enforcement, schema-version refusal.

use ed25519_dalek::SigningKey;
use ffs_core::store::{AtomStore, MemAtomStore, SqliteAtomStore, StoreError};
use ffs_core::{AtomEnvelope, AtomTemplate, EntityId, Iso8601, Multihash, PredicateName, Tier};
use rusqlite::params;

fn key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

fn dek() -> [u8; 32] {
    [42u8; 32]
}

fn atom(
    entity: &str,
    predicate: &str,
    claim: serde_json::Value,
    classification: &str,
    tx_time: &str,
    supersedes: Option<Multihash>,
) -> AtomEnvelope {
    let tmpl = AtomTemplate {
        v: 1,
        entity: EntityId::new(entity),
        predicate: PredicateName::new(predicate),
        claim,
        valid_from: Iso8601::new("2026-05-09T00:00:00Z").unwrap(),
        valid_to: None,
        tx_time: Iso8601::new(tx_time).unwrap(),
        classification: Tier::new(classification),
        supersedes,
        provenance: vec![],
    };
    tmpl.sign(&key()).expect("signing must succeed")
}

/// Insert + lookup + signature/tamper enforcement, run against any backend.
fn assert_basic(store: &dyn AtomStore) {
    let env = atom(
        "alice",
        "contact.person",
        serde_json::json!({"display_name": "Alice"}),
        "existence",
        "2026-05-09T08:00:00Z",
        None,
    );
    let h = store.insert(&env).expect("insert");

    // exists + get round-trip
    assert!(store.exists(&h).unwrap());
    let got = store.get(&h).unwrap().unwrap();
    assert_eq!(got, env);

    // Re-deserialized envelope canonicalizes to the same bytes — proves
    // the store preserves the canonical-JSON representation.
    let original = env.canonical_bytes().unwrap();
    let roundtripped = got.canonical_bytes().unwrap();
    assert_eq!(original, roundtripped);

    // Tampered signature is rejected.
    let mut bad = env.clone();
    bad.classification = Tier::new("notes");
    let err = store.insert(&bad).unwrap_err();
    assert!(
        matches!(err, StoreError::InvalidSignature(_)),
        "expected InvalidSignature, got {err:?}"
    );

    // Idempotent re-insert returns the same hash.
    let h2 = store.insert(&env).unwrap();
    assert_eq!(h, h2);
}

/// Bitemporal `as_of` cutoff and `head_of_chain` over a 3-deep supersession.
fn assert_bitemporal_and_head(store: &dyn AtomStore) {
    let entity = EntityId::new("alice");
    let pred = PredicateName::new("contact.person");

    // v1 at 09:00 — initial name "Alice"
    let v1 = atom(
        "alice",
        "contact.person",
        serde_json::json!({"display_name": "Alice"}),
        "existence",
        "2026-05-09T09:00:00Z",
        None,
    );
    let h1 = store.insert(&v1).unwrap();

    // v2 at 10:00 — supersedes v1 with "Alicia"
    let v2 = atom(
        "alice",
        "contact.person",
        serde_json::json!({"display_name": "Alicia"}),
        "existence",
        "2026-05-09T10:00:00Z",
        Some(h1.clone()),
    );
    let h2 = store.insert(&v2).unwrap();

    // v3 at 11:00 — supersedes v2 with "Alicia Vargas"
    let v3 = atom(
        "alice",
        "contact.person",
        serde_json::json!({"display_name": "Alicia Vargas"}),
        "existence",
        "2026-05-09T11:00:00Z",
        Some(h2.clone()),
    );
    let _h3 = store.insert(&v3).unwrap();

    // Head at "now" — latest non-superseded is v3.
    let head = store.head_of_chain(&entity, &pred, None).unwrap().unwrap();
    assert_eq!(head, v3);

    // Head as_of 09:30 — only v1 is in the cutoff; v1 isn't superseded yet.
    let cutoff = Iso8601::new("2026-05-09T09:30:00Z").unwrap();
    let head_at = store
        .head_of_chain(&entity, &pred, Some(&cutoff))
        .unwrap()
        .unwrap();
    assert_eq!(head_at, v1);

    // Head as_of 10:30 — v1, v2 in cutoff; v1 superseded by v2 → head is v2.
    let cutoff2 = Iso8601::new("2026-05-09T10:30:00Z").unwrap();
    let head_at2 = store
        .head_of_chain(&entity, &pred, Some(&cutoff2))
        .unwrap()
        .unwrap();
    assert_eq!(head_at2, v2);

    // list_by_entity returns all 3 in tx_time DESC order.
    let all = store.list_by_entity(&entity, Some(&pred), None).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0], v3);
    assert_eq!(all[1], v2);
    assert_eq!(all[2], v1);
}

/// Multi-leaf supersession tree: head selects latest tx_time among unsuperseded leaves.
fn assert_multi_leaf_head(store: &dyn AtomStore) {
    let entity = EntityId::new("bob");
    let pred = PredicateName::new("contact.person");

    let root = atom(
        "bob",
        "contact.person",
        serde_json::json!({"display_name": "Bob"}),
        "existence",
        "2026-05-09T08:00:00Z",
        None,
    );
    let hroot = store.insert(&root).unwrap();

    // Two children both supersede `root` (concurrent edits).
    let leaf_early = atom(
        "bob",
        "contact.person",
        serde_json::json!({"display_name": "Robert"}),
        "existence",
        "2026-05-09T09:00:00Z",
        Some(hroot.clone()),
    );
    store.insert(&leaf_early).unwrap();

    let leaf_late = atom(
        "bob",
        "contact.person",
        serde_json::json!({"display_name": "Bobby"}),
        "existence",
        "2026-05-09T10:00:00Z",
        Some(hroot.clone()),
    );
    store.insert(&leaf_late).unwrap();

    // Both leaves are unsuperseded; head selects the latest tx_time.
    let head = store.head_of_chain(&entity, &pred, None).unwrap().unwrap();
    assert_eq!(head, leaf_late);
}

/// FTS5 / substring search returns the matching atom.
fn assert_search(store: &dyn AtomStore) {
    let env = atom(
        "carol",
        "note",
        serde_json::json!({"title": "ideas about distributed systems", "body": "exocortex notes"}),
        "notes",
        "2026-05-09T12:00:00Z",
        None,
    );
    let h = store.insert(&env).unwrap();

    let hits = store.search_fts("distributed", 10).unwrap();
    assert!(
        hits.contains(&h),
        "expected hit for 'distributed'; got {hits:?}"
    );

    let no_hits = store
        .search_fts("zzz_definitely_not_present_zzz", 10)
        .unwrap();
    assert!(no_hits.is_empty());
}

/// list_by_predicate returns atoms for a predicate, ordered DESC, limited.
fn assert_list_by_predicate(store: &dyn AtomStore) {
    for (i, hour) in (0..5u32).zip(["08", "09", "10", "11", "12"]) {
        let env = atom(
            &format!("note-{i}"),
            "note.daily",
            serde_json::json!({"body": format!("entry {i}")}),
            "notes",
            &format!("2026-05-09T{hour}:00:00Z"),
            None,
        );
        store.insert(&env).unwrap();
    }
    let pred = PredicateName::new("note.daily");
    let recent = store.list_by_predicate(&pred, None, 3).unwrap();
    assert_eq!(recent.len(), 3);
    // DESC order: 12:00, 11:00, 10:00
    assert_eq!(recent[0].tx_time.as_str(), "2026-05-09T12:00:00Z");
    assert_eq!(recent[2].tx_time.as_str(), "2026-05-09T10:00:00Z");

    // since_tx watermark at 10:00 returns 11:00 and 12:00 (strict >).
    let since = Iso8601::new("2026-05-09T10:00:00Z").unwrap();
    let after = store.list_by_predicate(&pred, Some(&since), 10).unwrap();
    assert_eq!(after.len(), 2);
}

// ----- Backend test entry points -----

#[test]
fn mem_basic() {
    assert_basic(&MemAtomStore::new());
}

#[test]
fn sqlite_basic() {
    assert_basic(&SqliteAtomStore::open_in_memory(&dek()).unwrap());
}

#[test]
fn mem_bitemporal_and_head() {
    assert_bitemporal_and_head(&MemAtomStore::new());
}

#[test]
fn sqlite_bitemporal_and_head() {
    assert_bitemporal_and_head(&SqliteAtomStore::open_in_memory(&dek()).unwrap());
}

#[test]
fn mem_multi_leaf_head() {
    assert_multi_leaf_head(&MemAtomStore::new());
}

#[test]
fn sqlite_multi_leaf_head() {
    assert_multi_leaf_head(&SqliteAtomStore::open_in_memory(&dek()).unwrap());
}

#[test]
fn mem_search() {
    assert_search(&MemAtomStore::new());
}

#[test]
fn sqlite_search() {
    assert_search(&SqliteAtomStore::open_in_memory(&dek()).unwrap());
}

#[test]
fn mem_list_by_predicate() {
    assert_list_by_predicate(&MemAtomStore::new());
}

#[test]
fn sqlite_list_by_predicate() {
    assert_list_by_predicate(&SqliteAtomStore::open_in_memory(&dek()).unwrap());
}

// ----- SQLCipher-specific tests -----

#[test]
fn sqlcipher_rejects_incorrect_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");

    // Create with one key.
    let store = SqliteAtomStore::open_with_key(&path, &dek()).unwrap();
    let env = atom(
        "alice",
        "contact.person",
        serde_json::json!({"display_name": "Alice"}),
        "existence",
        "2026-05-09T08:00:00Z",
        None,
    );
    store.insert(&env).unwrap();
    drop(store);

    // Re-open with the WRONG key — should fail to read schema_version.
    let wrong_key = [99u8; 32];
    let result = SqliteAtomStore::open_with_key(&path, &wrong_key);
    assert!(result.is_err(), "expected wrong-key open to fail; got Ok");

    // Re-open with the right key — should succeed and find the atom.
    let store2 = SqliteAtomStore::open_with_key(&path, &dek()).unwrap();
    assert!(store2.exists(&env.content_hash().unwrap()).unwrap());
}

#[test]
fn sqlcipher_database_file_is_binary_encrypted_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    let store = SqliteAtomStore::open_with_key(&path, &dek()).unwrap();
    let env = atom(
        "alice",
        "contact.person",
        serde_json::json!({"display_name": "Alice"}),
        "existence",
        "2026-05-09T08:00:00Z",
        None,
    );
    store.insert(&env).unwrap();
    drop(store);

    // Read raw bytes; an encrypted SQLite DB does NOT start with the standard
    // "SQLite format 3" magic. SQLCipher-encrypted files have a randomized
    // first page.
    let bytes = std::fs::read(&path).unwrap();
    assert!(
        !bytes.starts_with(b"SQLite format 3\0"),
        "unencrypted SQLite header found in supposedly-encrypted DB"
    );
    // The plaintext "Alice" string should not appear anywhere in the file.
    assert!(
        !bytes.windows(5).any(|w| w == b"Alice"),
        "plaintext claim payload found in encrypted DB"
    );
}

#[test]
fn unknown_future_schema_version_refuses_to_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");

    // Open at v1, then poison the schema_version table with a future version.
    {
        let store = SqliteAtomStore::open_with_key(&path, &dek()).unwrap();
        // Borrow internal connection by reflection-free means: open a parallel
        // connection to the same file with the same key.
        drop(store);
    }
    {
        // Open a raw rusqlite connection with the SQLCipher key, bump the version.
        let conn = rusqlite::Connection::open(&path).unwrap();
        let hex_key: String = dek().iter().map(|b| format!("{b:02x}")).collect();
        conn.execute_batch(&format!("PRAGMA key = \"x'{hex_key}'\";"))
            .unwrap();
        conn.execute(
            "INSERT INTO schema_version(version, applied_at) VALUES (?1, ?2)",
            params![999, "2099-01-01T00:00:00Z"],
        )
        .unwrap();
        drop(conn);
    }
    // Now re-opening via SqliteAtomStore should refuse.
    let err = SqliteAtomStore::open_with_key(&path, &dek()).unwrap_err();
    match err {
        StoreError::UnsupportedSchemaVersion {
            found: 999,
            supported: ffs_core::store::SCHEMA_VERSION,
        } => {}
        other => {
            panic!("expected UnsupportedSchemaVersion {{found:999, supported:1}}, got {other:?}")
        }
    }
}

// ----- Mem-vs-Sqlite parity -----

#[test]
fn mem_and_sqlite_produce_identical_results_for_canonical_fixture() {
    let mem = MemAtomStore::new();
    let sql = SqliteAtomStore::open_in_memory(&dek()).unwrap();

    let fixture = vec![
        atom(
            "alice",
            "contact.person",
            serde_json::json!({"display_name": "Alice"}),
            "existence",
            "2026-05-09T08:00:00Z",
            None,
        ),
        atom(
            "bob",
            "contact.person",
            serde_json::json!({"display_name": "Bob"}),
            "existence",
            "2026-05-09T08:30:00Z",
            None,
        ),
        atom(
            "alice",
            "contact.person",
            serde_json::json!({"display_name": "Alicia"}),
            "existence",
            "2026-05-09T09:00:00Z",
            None,
        ),
    ];
    let mut hashes = Vec::new();
    for a in &fixture {
        let h_mem = mem.insert(a).unwrap();
        let h_sql = sql.insert(a).unwrap();
        assert_eq!(h_mem, h_sql);
        hashes.push(h_mem);
    }

    // Same get results.
    for h in &hashes {
        let m = mem.get(h).unwrap().unwrap();
        let s = sql.get(h).unwrap().unwrap();
        assert_eq!(m, s);
    }

    // Same list_by_entity ordering.
    let entity = EntityId::new("alice");
    let pred = PredicateName::new("contact.person");
    let m = mem.list_by_entity(&entity, Some(&pred), None).unwrap();
    let s = sql.list_by_entity(&entity, Some(&pred), None).unwrap();
    assert_eq!(m, s);

    // Same head_of_chain.
    let mh = mem.head_of_chain(&entity, &pred, None).unwrap();
    let sh = sql.head_of_chain(&entity, &pred, None).unwrap();
    assert_eq!(mh, sh);
}
