//! task_46 (ADR-034): the derived-status function and the spec plumbing.
//!
//! `status_of` is the one piece with state-machine flavor, so it gets
//! property tests; the spec table and the override file get loader
//! tests.

use std::path::{Path, PathBuf};

use ed25519_dalek::SigningKey;
use ffs_core::attestation::{
    Attestation, AttestationPolicy, Basis, CorrectionReason, Status, correction_reason_of,
    load_overrides, status_of,
};
use ffs_core::predicate::SpecRegistry;
use ffs_core::{
    AtomEnvelope, AtomTemplate, EntityId, Iso8601, PredicateName, Provenance, PublicKey,
    SourceKind, Tier,
};
use proptest::prelude::*;

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn pubkey(seed: u8) -> PublicKey {
    PublicKey::from_bytes(key(seed).verifying_key().to_bytes())
}

fn iso(s: &str) -> Iso8601 {
    Iso8601::new(s).unwrap()
}

fn head(valid_from: &str, valid_to: Option<&str>) -> AtomEnvelope {
    AtomTemplate {
        v: 1,
        entity: EntityId::new("zEntity"),
        predicate: PredicateName::new("affiliation"),
        claim: serde_json::json!({"person": "zP", "organization": "zO", "title": "CEO"}),
        valid_from: iso(valid_from),
        valid_to: valid_to.map(iso),
        tx_time: iso(valid_from),
        classification: Tier::new("existence"),
        supersedes: None,
        provenance: vec![],
    }
    .sign(&key(1))
    .unwrap()
}

fn att(as_of: &str, basis: Basis, source: &str, attester: u8, tx: &str) -> Attestation {
    Attestation {
        as_of: as_of.into(),
        basis,
        source: source.into(),
        note: None,
        attester: pubkey(attester),
        tx_time: iso(tx),
        hash: None,
    }
}

fn policy(k: u32, window: Option<u32>, independent: bool) -> AttestationPolicy {
    AttestationPolicy {
        k,
        window_days: window,
        independent,
    }
}

const NOW: &str = "2026-09-20T12:00:00Z";

fn arb_basis() -> impl Strategy<Value = Basis> {
    prop_oneof![
        Just(Basis::ReReadSameSource),
        Just(Basis::IndependentSource),
        Just(Basis::PrimarySource),
        Just(Basis::OwnerKnowledge),
    ]
}

/// Attestations dated inside the last 30 days, from a small pool of
/// sources so independence collapse is exercised.
fn arb_attestation() -> impl Strategy<Value = Attestation> {
    (arb_basis(), 0u8..4, 1u8..4, 1u32..30).prop_map(|(basis, src, who, days_ago)| {
        let day = 20u32.saturating_sub(days_ago % 19).max(1);
        att(
            &format!("2026-09-{day:02}"),
            basis,
            &format!("https://example.test/{src}"),
            who,
            &format!("2026-09-{day:02}T09:00:00Z"),
        )
    })
}

proptest! {
    #[test]
    fn never_current_with_fewer_than_k_independent_attestations(
        atts in prop::collection::vec(arb_attestation(), 0..6),
        k in 1u32..6,
    ) {
        let h = head("2026-08-01T00:00:00Z", None);
        let report = status_of(&h, &atts, &policy(k, Some(365), true), &iso(NOW), None);
        if report.independent_count < k {
            prop_assert_ne!(report.status, Status::Current);
            prop_assert_eq!(report.status, Status::Unconfirmed);
        }
    }

    #[test]
    fn independent_true_collapses_attestations_sharing_basis_and_source(
        n in 1usize..6,
        basis in arb_basis(),
    ) {
        // n attestations from n different signers, all re-reading the
        // SAME url (with tracking noise): one independent confirmation.
        let atts: Vec<Attestation> = (0..n)
            .map(|i| att("2026-09-10", basis, &format!("https://Example.test/a?utm_source={i}"), (i % 3) as u8 + 1, "2026-09-10T09:00:00Z"))
            .collect();
        let h = head("2026-08-01T00:00:00Z", None);
        let collapsed = status_of(&h, &atts, &policy(2, None, true), &iso(NOW), None);
        let counted = status_of(&h, &atts, &policy(2, None, false), &iso(NOW), None);
        prop_assert_eq!(collapsed.independent_count, 1);
        prop_assert_eq!(collapsed.status, Status::Unconfirmed);
        prop_assert_eq!(counted.independent_count, n as u32);
        if n >= 2 {
            prop_assert_eq!(counted.status, Status::Current);
        }
    }

    #[test]
    fn newest_qualifying_attestation_older_than_window_is_stale(
        window in 1u32..200,
        age_days in 1u32..400,
    ) {
        // One attestation `age_days` before NOW; the window decides.
        let day = time::Date::parse(&NOW[..10], &time::macros::format_description!("[year]-[month]-[day]")).unwrap()
            - time::Duration::days(i64::from(age_days));
        let as_of = format!("{:04}-{:02}-{:02}", day.year(), u8::from(day.month()), day.day());
        let atts = vec![att(&as_of, Basis::OwnerKnowledge, "person:owner", 1, &format!("{as_of}T09:00:00Z"))];
        let h = head("2025-01-01T00:00:00Z", None);
        let report = status_of(&h, &atts, &policy(1, Some(window), true), &iso(NOW), None);
        if age_days > window {
            prop_assert_eq!(report.status, Status::Stale);
            prop_assert_eq!(report.since.as_deref(), Some(as_of.as_str()));
        } else {
            prop_assert_eq!(report.status, Status::Current);
        }
    }

    #[test]
    fn unresolved_contradicted_by_yields_disputed(confirmations in prop::collection::vec(arb_attestation(), 0..4)) {
        // The dispute is newer (by tx_time) than every confirmation.
        let mut atts = confirmations;
        atts.push(att("2026-09-20", Basis::ContradictedBy, "zOtherAtom", 2, "2026-09-20T11:00:00Z"));
        let h = head("2026-08-01T00:00:00Z", None);
        let report = status_of(&h, &atts, &policy(1, Some(365), true), &iso(NOW), None);
        prop_assert_eq!(report.status, Status::Disputed);
        // A later re-confirmation resolves it.
        atts.push(att("2026-09-20", Basis::OwnerKnowledge, "person:owner", 1, "2026-09-20T11:30:00Z"));
        let report = status_of(&h, &atts, &policy(1, Some(365), true), &iso(NOW), None);
        prop_assert_ne!(report.status, Status::Disputed);
    }

    #[test]
    fn a_never_true_correction_deprecates_regardless_of_attestations(atts in prop::collection::vec(arb_attestation(), 0..6)) {
        let h = head("2026-08-01T00:00:00Z", None);
        let report = status_of(&h, &atts, &policy(1, Some(90), true), &iso(NOW), Some(CorrectionReason::NeverTrue));
        prop_assert_eq!(report.status, Status::Deprecated);
        let report = status_of(&h, &atts, &policy(1, Some(90), true), &iso(NOW), Some(CorrectionReason::WorldChanged));
        prop_assert_ne!(report.status, Status::Deprecated);
    }

    #[test]
    fn no_window_predicate_is_current_forever_after_one_attestation(years_ago in 0u32..30) {
        let year = 2026 - years_ago;
        let as_of = format!("{year:04}-01-15");
        let atts = vec![att(&as_of, Basis::ReReadSameSource, "https://example.test/x", 1, &format!("{as_of}T09:00:00Z"))];
        let h = head("1990-01-01T00:00:00Z", None);
        let report = status_of(&h, &atts, &policy(1, None, true), &iso(NOW), None);
        prop_assert_eq!(report.status, Status::Current);
        prop_assert!(!report.windowed);
    }
}

#[test]
fn ended_facts_are_history_not_a_chore() {
    let h = head("2024-01-01T00:00:00Z", Some("2025-06-30T00:00:00Z"));
    let atts = vec![att(
        "2024-01-01",
        Basis::OwnerKnowledge,
        "person:owner",
        1,
        "2024-01-01T09:00:00Z",
    )];
    let report = status_of(&h, &atts, &policy(1, Some(90), true), &iso(NOW), None);
    assert_eq!(report.status, Status::Ended);
}

#[test]
fn correction_reason_is_read_from_the_superseding_atoms_provenance() {
    let mut env = head("2026-08-01T00:00:00Z", None);
    assert_eq!(correction_reason_of(&env), None);
    env.provenance.push(Provenance {
        kind: SourceKind::Correction,
        uri: CorrectionReason::NeverTrue.provenance_uri(),
        hash: env.content_hash().unwrap(),
    });
    assert_eq!(
        correction_reason_of(&env),
        Some(CorrectionReason::NeverTrue)
    );
}

fn repo_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
}

#[test]
fn starter_attestation_spec_loads_without_a_path_table() {
    let reg = SpecRegistry::new();
    reg.load_dir(&repo_root().join("starter/predicates"))
        .unwrap();
    let spec = reg.get("attestation").expect("attestation spec registered");
    assert!(
        spec.path.is_none(),
        "attestations never render as their own files"
    );
    assert!(spec.reverse_map.is_empty());
    assert_eq!(
        spec.ontology.as_ref().unwrap().bfo,
        "information content entity"
    );
    assert!(reg.families().iter().all(|f| f.predicate != "attestation"));
}

#[test]
fn starter_attestation_windows_match_adr_034_defaults() {
    let reg = SpecRegistry::new();
    reg.load_dir(&repo_root().join("starter/predicates"))
        .unwrap();
    let pol =
        |name: &str| AttestationPolicy::from_spec(reg.get(name).unwrap().attestation.as_ref());
    assert_eq!(pol("affiliation").window_days, Some(90));
    assert_eq!(pol("person.generic").window_days, Some(90));
    assert_eq!(pol("org.company").window_days, Some(180));
    for name in ["source.article", "event.business", "contact.person", "note"] {
        assert_eq!(pol(name).window_days, None, "{name} has no window");
        assert_eq!(pol(name).k, 1);
    }
}

#[test]
fn attestation_table_parses_defaults_and_rejects_unknown_keys() {
    let base = r#"
name = "widget.thing"
version = 1
[claim_schema]
type = "object"
[rendering]
template = "widget.md.tera"
"#;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("a.toml"),
        format!("{base}\n[attestation]\nwindow_days = 30\n"),
    )
    .unwrap();
    let reg = SpecRegistry::new();
    reg.load_dir(dir.path()).unwrap();
    let a = reg.get("widget.thing").unwrap().attestation.unwrap();
    assert_eq!((a.k, a.window_days, a.independent), (1, Some(30), true));

    let bad = tempfile::tempdir().unwrap();
    std::fs::write(
        bad.path().join("b.toml"),
        format!("{base}\n[attestation]\nbogus = 1\n"),
    )
    .unwrap();
    assert!(
        SpecRegistry::new().load_dir(bad.path()).is_err(),
        "unknown [attestation] keys fail the load"
    );
}

#[test]
fn override_file_raises_k_but_a_lower_value_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("attestation.toml");
    std::fs::write(&path, "[affiliation]\nk = 3\n[org.company]\nk = 1\n").unwrap();
    let overrides = load_overrides(&path).unwrap();
    let find = |name: &str| overrides.iter().find(|(n, _)| n == name).map(|(_, k)| *k);
    assert_eq!(
        policy(1, Some(90), true).raised_to(find("affiliation")).k,
        3
    );
    // org.company's spec says k = 2 here; the override of 1 must not lower it.
    assert_eq!(
        policy(2, Some(180), true).raised_to(find("org.company")).k,
        2
    );
    assert!(
        load_overrides(Path::new("/nonexistent/attestation.toml"))
            .unwrap()
            .is_empty()
    );
}
