//! `ScribeExtractor` implementation backed by the
//! `ffs-skills-host` subprocess host (task_26).
//!
//! The dispatcher exposes a `Dispatcher::scribe:
//! Option<Arc<dyn ScribeExtractor>>` slot. Tests inject in-process
//! stubs that synthesize proposals without spawning Python; the
//! production daemon binary wires this `SkillsHostScribeExtractor`
//! which forwards to the scribe skill bundle installed under
//! `$FFS_DATA_DIR/skills/scribe/`.
//!
//! Wire format (matches the scribe's `handle(inp)` contract in
//! `skills/scribe/extraction.py`):
//!
//! - Invoke input: `{"source_uri": "<uri>", "content": "<markdown>"}`
//! - Successful response: `{"proposals": [...], "warnings": [...]}`
//!   where each proposal matches `ffs_core::quarantine::Proposal`.
//!
//! Skill-side errors (the Python handler raised, the skill crashed
//! mid-invocation, or the per-call timeout fired) are translated to
//! `ScribeExtractError::Failed(<diagnostic>)`. The supervisor
//! restarts the skill in the background; the next call to `extract`
//! gets a fresh subprocess.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use ffs_core::predicate::SpecRegistry;
use ffs_core::quarantine::CrossRef;
use ffs_core::store::AtomStore;
use ffs_core::{
    Iso8601, Multihash, PredicateName, Proposal, Provenance, ResolutionConfig, SourceKind,
};
use ffs_skills_host::{SkillError, SkillsHost};

use crate::dispatch::{ScribeExtractError, ScribeExtractor, SkillInvoker};
use crate::resolver::{StoreLookup, SubmissionContext, resolve_set};

/// Production `ScribeExtractor` that forwards extraction calls to
/// the scribe `SkillProcess` inside `SkillsHost`. Holds the host as
/// an `Arc` so the same instance can be shared between the
/// dispatcher and the surrounding wiring (auditor health summaries
/// read the skill restart counts, etc.).
pub struct SkillsHostScribeExtractor {
    host: Arc<SkillsHost>,
    /// Bundle name to look up — for MVP always `"scribe"`. Stored
    /// so a future `--scribe-name` flag can override without
    /// touching the impl.
    skill_name: String,
}

impl SkillsHostScribeExtractor {
    pub fn new(host: Arc<SkillsHost>) -> Self {
        Self {
            host,
            skill_name: "scribe".into(),
        }
    }

    /// Construct with an explicit skill name. Used by tests that
    /// install a stub skill under a non-canonical name.
    pub fn with_name(host: Arc<SkillsHost>, skill_name: impl Into<String>) -> Self {
        Self {
            host,
            skill_name: skill_name.into(),
        }
    }
}

#[async_trait]
impl ScribeExtractor for SkillsHostScribeExtractor {
    async fn extract(
        &self,
        source_uri: &str,
        content: &[u8],
    ) -> Result<Vec<Proposal>, ScribeExtractError> {
        let skill = self.host.get(&self.skill_name).ok_or_else(|| {
            ScribeExtractError::Failed(format!(
                "scribe skill `{}` not registered with skills host",
                self.skill_name
            ))
        })?;

        // Scribe's contract accepts a Markdown string under
        // `content`. Bytes that aren't valid UTF-8 are surfaced as
        // a Failed error rather than a silent lossy decode — we'd
        // rather not pretend to have parsed a binary file as
        // markdown.
        let content_str = std::str::from_utf8(content)
            .map_err(|e| ScribeExtractError::Failed(format!("content is not valid UTF-8: {e}")))?;

        let input = serde_json::json!({
            "source_uri": source_uri,
            "content": content_str,
        });

        let raw = skill.invoke(input).await.map_err(translate_skill_error)?;
        let proposals = parse_scribe_response(&raw)?;
        Ok(proposals)
    }
}

fn translate_skill_error(e: SkillError) -> ScribeExtractError {
    match e {
        SkillError::Timeout(d) => {
            ScribeExtractError::Failed(format!("scribe timed out after {d:?}"))
        }
        SkillError::Crashed => ScribeExtractError::Failed("scribe crashed mid-invocation".into()),
        SkillError::SkillReported(reason) => {
            ScribeExtractError::Failed(format!("scribe reported error: {reason}"))
        }
        SkillError::Io(io) => ScribeExtractError::Failed(format!("scribe io: {io}")),
        SkillError::ShutDown => ScribeExtractError::Failed("scribe is shut down".into()),
    }
}

/// Parse a scribe response of the form
/// `{"proposals": [...], "warnings": [...]}` into `Vec<Proposal>`.
/// Missing or empty `proposals` is treated as zero proposals;
/// non-array proposals is a hard failure.
///
/// Each proposal arrives in the Python scribe's wire shape
/// (`provenance[].kind` is a string, `provenance[].hash_hex` is
/// a hex string), not the Rust `Provenance` struct shape. We
/// translate via [`ScribeProposalWire`] so the rest of the
/// daemon sees a canonical `Proposal` with a real `Multihash`.
fn parse_scribe_response(raw: &Value) -> Result<Vec<Proposal>, ScribeExtractError> {
    let Some(arr) = raw.get("proposals") else {
        return Ok(Vec::new());
    };
    let arr = arr.as_array().ok_or_else(|| {
        ScribeExtractError::Failed(format!("scribe returned non-array `proposals`: {raw}"))
    })?;
    let mut out = Vec::with_capacity(arr.len());
    for (idx, item) in arr.iter().enumerate() {
        let wire: ScribeProposalWire = serde_json::from_value(item.clone()).map_err(|e| {
            ScribeExtractError::Failed(format!(
                "scribe proposal #{idx} did not parse: {e}; raw: {item}"
            ))
        })?;
        out.push(Proposal::from(wire));
    }
    Ok(out)
}

/// Wire shape the Python scribe emits. See `skills/scribe/extraction.py`
/// `_make_proposal()`. The `hash_hex` and string `kind` fields don't
/// directly map to the Rust `Provenance` struct, so we translate.
#[derive(Deserialize)]
struct ScribeProposalWire {
    predicate: String,
    claim: Value,
    provenance: Vec<ScribeProvenanceWire>,
    rationale: String,
    /// Extraction engine (task_36 / ADR-026): `"heuristic"` or
    /// `"llm"`. Optional so pre-task_36 scribes still parse.
    #[serde(default)]
    engine: Option<String>,
    /// Model id when the engine is `llm`; empty string or absent
    /// for the heuristic engine (normalized to `None`).
    #[serde(default)]
    model: Option<String>,
    // ---- task_45 multi-entity set (all optional; ADR-030 / ADR-031) ----
    #[serde(default)]
    local_ref: Option<String>,
    #[serde(default)]
    refs: Vec<ScribeRefWire>,
    /// `YYYY-MM-DD` or a full ISO 8601 timestamp.
    #[serde(default)]
    valid_from: Option<String>,
    #[serde(default)]
    valid_to: Option<String>,
    #[serde(default)]
    ends_role: bool,
    /// task_48 (ADR-035): `"clip"` for a morning-read article body.
    #[serde(default)]
    classification_hint: Option<String>,
}

#[derive(Deserialize)]
struct ScribeRefWire {
    field: String,
    local_ref: String,
}

#[derive(Deserialize)]
struct ScribeProvenanceWire {
    kind: String,
    uri: String,
    hash_hex: String,
}

/// Parse a scribe date: a bare `YYYY-MM-DD` becomes midnight UTC; a
/// full ISO 8601 timestamp is validated as-is. Anything else is `None`
/// (the proposal falls back to "now" at accept) rather than an error,
/// so a model's malformed date never sinks a submission.
pub fn parse_scribe_date(raw: &str) -> Option<Iso8601> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    if t.len() == 10 && t.as_bytes()[4] == b'-' && t.as_bytes()[7] == b'-' {
        return Iso8601::new(format!("{t}T00:00:00Z")).ok();
    }
    Iso8601::new(t).ok()
}

impl From<ScribeProposalWire> for Proposal {
    fn from(w: ScribeProposalWire) -> Self {
        let provenance = w
            .provenance
            .into_iter()
            .map(|p| Provenance {
                kind: scribe_kind_to_source_kind(&p.kind),
                uri: p.uri,
                hash: hex_to_multihash(&p.hash_hex),
            })
            .collect();
        let mut p = Proposal::new(
            PredicateName::new(w.predicate),
            w.claim,
            provenance,
            w.rationale,
        );
        p.engine = w.engine.filter(|s| !s.is_empty());
        p.model = w.model.filter(|s| !s.is_empty());
        p.classification_hint = w.classification_hint.filter(|s| !s.is_empty());
        p.local_ref = w.local_ref.filter(|s| !s.is_empty());
        p.refs = w
            .refs
            .into_iter()
            .filter(|r| !r.field.is_empty() && !r.local_ref.is_empty())
            .map(|r| CrossRef {
                field: CrossRef::from_wire_field(&r.field),
                local_ref: r.local_ref,
            })
            .collect();
        p.valid_from = w.valid_from.as_deref().and_then(parse_scribe_date);
        p.valid_to = w.valid_to.as_deref().and_then(parse_scribe_date);
        p.ends_role = w.ends_role;
        p
    }
}

/// `source_article` (the article every proposal in a multi-entity set
/// points back to) maps to `IngestFile` because `ffs-core` has no
/// dedicated `SourceKind` for it yet; the uri is kept, which is what
/// dedup and the briefing read. Adding a variant is an envelope
/// decision (ADR needed), noted as a follow-up.
fn scribe_kind_to_source_kind(kind: &str) -> SourceKind {
    match kind {
        "ingest" | "ingest_file" | "source_article" => SourceKind::IngestFile,
        "federation_pull" => SourceKind::FederationPull,
        "fast_path" => SourceKind::FastPath,
        "morning_read" => SourceKind::MorningRead,
        "session" => SourceKind::Session,
        _ => SourceKind::IngestFile,
    }
}

/// Decorator that runs the daemon-side resolver (task_45, ADR-030)
/// over whatever the wrapped extractor returns, so every proposal
/// reaching the quarantine carries `resolution` and `candidates`.
///
/// The resolution config is re-read from
/// `$FFS_DATA_DIR/config/resolution.toml` on every call so an owner
/// edit takes effect on the next submission, like predicate specs.
/// When the file is missing or invalid a compiled default is used
/// and a warning is logged.
pub struct ResolvingExtractor {
    inner: Arc<dyn ScribeExtractor>,
    store: Arc<dyn AtomStore>,
    registry: Arc<SpecRegistry>,
    config_path: Option<std::path::PathBuf>,
}

impl ResolvingExtractor {
    pub fn new(
        inner: Arc<dyn ScribeExtractor>,
        store: Arc<dyn AtomStore>,
        registry: Arc<SpecRegistry>,
        data_dir: Option<&std::path::Path>,
    ) -> Self {
        Self {
            inner,
            store,
            registry,
            config_path: data_dir.map(|d| d.join("config").join("resolution.toml")),
        }
    }

    /// Compiled fallback mirroring `starter/config/resolution.toml`.
    pub fn default_config() -> ResolutionConfig {
        ResolutionConfig::from_toml_str(include_str!("../../../starter/config/resolution.toml"))
            .expect("starter resolution.toml is valid")
    }

    fn load_config(&self) -> ResolutionConfig {
        match &self.config_path {
            Some(path) => match ResolutionConfig::load(path) {
                Ok(cfg) => cfg,
                Err(e) => {
                    tracing::warn!(error = %e, path = %path.display(), "resolution config unusable; using the compiled default");
                    Self::default_config()
                }
            },
            None => Self::default_config(),
        }
    }

    /// Resolve an already-extracted set (used by `ingest.submit`
    /// callers and tests that have proposals in hand).
    pub fn resolve(&self, source_uri: &str, mut set: Vec<Proposal>) -> Vec<Proposal> {
        let cfg = self.load_config();
        let lookups = StoreLookup {
            store: &*self.store,
            registry: &self.registry,
        };
        let mut ctx = SubmissionContext::new(source_uri, now_iso());
        let report = resolve_set(&mut set, &lookups, &cfg, &mut ctx);
        for w in &report.warnings {
            tracing::info!(source_uri = %source_uri, "resolver: {w}");
        }
        if !report.backfill.is_empty() {
            set = attach_backfills(set, &report.backfill, &*self.store, &self.registry);
        }
        set
    }
}

#[async_trait]
impl ScribeExtractor for ResolvingExtractor {
    async fn extract(
        &self,
        source_uri: &str,
        content: &[u8],
    ) -> Result<Vec<Proposal>, ScribeExtractError> {
        let set = self.inner.extract(source_uri, content).await?;
        Ok(self.resolve(source_uri, set))
    }
}

/// When a bare mention crosses the second-sighting line, the earlier
/// article (identified by the sighting's submission id, which is its
/// source uri) should gain the entity on its mention. This adds a
/// synthetic supersession proposal of that article's head atom with
/// the mention referencing the newly minted person's `local_ref`;
/// accept rewrites the ref once the id exists. Skipped with a warning
/// when the earlier article was never accepted.
fn attach_backfills(
    mut set: Vec<Proposal>,
    backfills: &[(String, String)],
    store: &dyn AtomStore,
    registry: &SpecRegistry,
) -> Vec<Proposal> {
    let Some(article_family) = registry
        .families()
        .into_iter()
        .find(|f| f.predicate == "source.article")
    else {
        return set;
    };
    let pred = PredicateName::new(&article_family.predicate);
    let articles = store
        .list_by_predicate(&pred, None, 10_000)
        .unwrap_or_default();
    for (earlier_uri, display) in backfills {
        // The minted person: the proposal whose display equals the sighting's.
        let person_ref = set
            .iter()
            .find(|p| {
                ffs_core::resolve::normalized_name_key(
                    p.claim
                        .get("display_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or(""),
                ) == ffs_core::resolve::normalized_name_key(display)
            })
            .and_then(|p| p.local_ref.clone());
        let Some(person_ref) = person_ref else {
            continue;
        };
        let earlier = articles
            .iter()
            .find(|a| a.provenance.iter().any(|pv| &pv.uri == earlier_uri));
        let Some(earlier) = earlier else {
            tracing::warn!(earlier = %earlier_uri, "backfill skipped: earlier article was never accepted");
            continue;
        };
        let Ok(Some(head)) = store.head_of_chain(&earlier.entity, &pred, None) else {
            continue;
        };
        let idx = head
            .claim
            .get("mentions")
            .and_then(|m| m.as_array())
            .and_then(|arr| {
                arr.iter().position(|m| {
                    m.get("display").and_then(|d| d.as_str()).is_some_and(|d| {
                        ffs_core::resolve::normalized_name_key(d)
                            == ffs_core::resolve::normalized_name_key(display)
                    })
                })
            });
        let Some(idx) = idx else {
            continue;
        };
        let mut bp = Proposal::new(
            pred.clone(),
            head.claim.clone(),
            head.provenance.clone(),
            format!(
                "backfill: second sighting of {display:?} minted an entity; earlier mention gains it"
            ),
        );
        bp.local_ref = Some(format!("backfill-{}", set.len()));
        bp.entity = Some(head.entity.clone());
        bp.resolution = Some(ffs_core::quarantine::Resolution::Existing);
        bp.refs = vec![CrossRef {
            field: format!("mentions/{idx}/entity"),
            local_ref: person_ref,
        }];
        set.push(bp);
    }
    set
}

fn now_iso() -> Iso8601 {
    let s = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Iso8601::DEFAULT)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into());
    Iso8601::new(s).unwrap_or_else(|_| Iso8601::new("1970-01-01T00:00:00Z").expect("constant"))
}

/// Decode a hex-encoded BLAKE3-256 digest into a Multihash. If the
/// hex doesn't decode to 32 bytes (corrupt or the scribe fell back
/// to sha256), we hash the hex string itself so the resulting
/// Multihash is at least well-formed and deterministic per source.
/// The substrate doesn't currently verify scribe's claimed hash —
/// production wiring (task 22) recomputes from the raw content on
/// acceptance.
fn hex_to_multihash(hex: &str) -> Multihash {
    match decode_hex(hex) {
        Some(bytes) if bytes.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            Multihash::from_blake3(&arr)
        }
        _ => Multihash::blake3_of(hex.as_bytes()),
    }
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    for chunk in s.as_bytes().chunks(2) {
        let hi = (chunk[0] as char).to_digit(16)?;
        let lo = (chunk[1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

/// `SkillInvoker` over the skills host: `courier.run` (task_40) and any
/// future by-name skill trigger go through here. Errors are flattened
/// to strings; the dispatcher maps them to an RPC error.
pub struct SkillsHostInvoker {
    host: Arc<SkillsHost>,
}

impl SkillsHostInvoker {
    pub fn new(host: Arc<SkillsHost>) -> Self {
        Self { host }
    }
}

#[async_trait]
impl SkillInvoker for SkillsHostInvoker {
    async fn invoke(&self, skill: &str, input: Value) -> Result<Value, String> {
        let process = self.host.get(skill).ok_or_else(|| {
            format!("skill `{skill}` is not installed under $FFS_DATA_DIR/skills/")
        })?;
        process
            .invoke(input)
            .await
            .map_err(|e| format!("skill `{skill}` failed: {}", translate_skill_error(e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One scribe-wire-shaped proposal. The scribe emits `hash_hex`
    /// (hex digest, blake3 or sha256 fallback) and a string
    /// `kind`, not the Rust `Provenance` struct's multibase hash +
    /// `SourceKind` enum. Tests use this helper so the wire
    /// contract is exercised end-to-end.
    fn scribe_wire_proposal() -> serde_json::Value {
        serde_json::json!({
            "predicate": "contact.person",
            "claim": {"display_name": "Sara Chen"},
            "provenance": [{
                "kind": "ingest",
                "uri": "file:///tmp/note.md",
                "hash_hex": "a".repeat(64),
            }],
            "rationale": "extracted from frontmatter",
        })
    }

    #[test]
    fn parses_scribe_response_with_one_proposal() {
        let raw = serde_json::json!({
            "proposals": [scribe_wire_proposal()],
            "warnings": [],
        });
        let proposals = parse_scribe_response(&raw).expect("ok");
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].predicate, PredicateName::new("contact.person"));
        assert_eq!(proposals[0].rationale, "extracted from frontmatter");
        assert_eq!(proposals[0].provenance.len(), 1);
        assert_eq!(proposals[0].provenance[0].uri, "file:///tmp/note.md");
        assert!(matches!(
            proposals[0].provenance[0].kind,
            SourceKind::IngestFile
        ));
    }

    #[test]
    fn engine_and_model_absent_parse_as_none() {
        let raw = serde_json::json!({"proposals": [scribe_wire_proposal()]});
        let proposals = parse_scribe_response(&raw).expect("ok");
        assert!(proposals[0].engine.is_none());
        assert!(proposals[0].model.is_none());
    }

    #[test]
    fn engine_and_model_present_are_carried_and_empty_model_normalizes_to_none() {
        let mut llm = scribe_wire_proposal();
        llm["engine"] = serde_json::json!("llm");
        llm["model"] = serde_json::json!("claude-sonnet-5");
        let mut heuristic = scribe_wire_proposal();
        heuristic["engine"] = serde_json::json!("heuristic");
        heuristic["model"] = serde_json::json!("");
        let raw = serde_json::json!({"proposals": [llm, heuristic]});
        let proposals = parse_scribe_response(&raw).expect("ok");
        assert_eq!(proposals[0].engine.as_deref(), Some("llm"));
        assert_eq!(proposals[0].model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(proposals[1].engine.as_deref(), Some("heuristic"));
        assert!(proposals[1].model.is_none());
    }

    #[test]
    fn parses_scribe_response_with_no_proposals_field() {
        let raw = serde_json::json!({"warnings": ["scribe was confused"]});
        let proposals = parse_scribe_response(&raw).expect("ok");
        assert!(proposals.is_empty());
    }

    #[test]
    fn parses_scribe_response_with_empty_proposals() {
        let raw = serde_json::json!({"proposals": [], "warnings": []});
        assert!(parse_scribe_response(&raw).unwrap().is_empty());
    }

    #[test]
    fn rejects_non_array_proposals() {
        let raw = serde_json::json!({"proposals": "not-an-array"});
        let err = parse_scribe_response(&raw).expect_err("should reject");
        assert!(matches!(err, ScribeExtractError::Failed(m) if m.contains("non-array")));
    }

    #[test]
    fn rejects_malformed_proposal_item() {
        // Missing `predicate` — wire-decode fails.
        let raw = serde_json::json!({
            "proposals": [{"claim": {"a": 1}, "provenance": [], "rationale": "x"}],
        });
        let err = parse_scribe_response(&raw).expect_err("should reject");
        assert!(matches!(err, ScribeExtractError::Failed(_)));
    }

    #[test]
    fn translates_timeout_to_failed_with_diagnostic() {
        let e = translate_skill_error(SkillError::Timeout(std::time::Duration::from_secs(30)));
        match e {
            ScribeExtractError::Failed(m) => assert!(m.contains("timed out")),
        }
    }

    #[test]
    fn translates_crashed_to_failed_with_diagnostic() {
        let e = translate_skill_error(SkillError::Crashed);
        match e {
            ScribeExtractError::Failed(m) => assert!(m.contains("crashed")),
        }
    }

    #[test]
    fn translates_skill_reported_to_failed_preserving_reason() {
        let e = translate_skill_error(SkillError::SkillReported("ValueError: bad input".into()));
        match e {
            ScribeExtractError::Failed(m) => {
                assert!(m.contains("scribe reported error"));
                assert!(m.contains("ValueError: bad input"));
            }
        }
    }

    #[test]
    fn proposal_translates_real_scribe_output_shape() {
        // Mirrors `skills/scribe/extraction.py::_make_proposal`.
        let raw = serde_json::json!({
            "proposals": [{
                "predicate": "contact.person",
                "claim": {
                    "display_name": "Sara Chen",
                    "work_email": "sara@example.com",
                    "notes": ["met at picnic"]
                },
                "provenance": [{
                    "kind": "ingest",
                    "uri": "file:///tmp/note.md",
                    "hash_hex": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                }],
                "rationale": "extracted display_name + contact fields from frontmatter and `Notes` section",
            }]
        });
        let proposals = parse_scribe_response(&raw).expect("ok");
        assert_eq!(proposals[0].predicate.as_str(), "contact.person");
        assert_eq!(
            proposals[0]
                .claim
                .get("work_email")
                .and_then(|v| v.as_str()),
            Some("sara@example.com")
        );
        // The 32-byte hex digest survives as a real Multihash.
        let mb = proposals[0].provenance[0].hash.to_multibase();
        assert!(mb.starts_with('z'), "expected multibase prefix; got {mb}");
    }

    #[test]
    fn hex_to_multihash_falls_back_when_input_isnt_32_bytes() {
        // Short hex → falls back to hashing the hex string itself
        // so we still get a deterministic, well-formed Multihash.
        let mh = hex_to_multihash("deadbeef");
        let mb = mh.to_multibase();
        assert!(mb.starts_with('z'));
    }

    #[test]
    fn scribe_kind_maps_ingest_to_ingest_file() {
        assert!(matches!(
            scribe_kind_to_source_kind("ingest"),
            SourceKind::IngestFile
        ));
        assert!(matches!(
            scribe_kind_to_source_kind("ingest_file"),
            SourceKind::IngestFile
        ));
        assert!(matches!(
            scribe_kind_to_source_kind("federation_pull"),
            SourceKind::FederationPull
        ));
        assert!(matches!(
            scribe_kind_to_source_kind("unrecognized"),
            SourceKind::IngestFile // fallback
        ));
    }
}
