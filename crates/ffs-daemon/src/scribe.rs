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

use ffs_core::{Multihash, PredicateName, Proposal, Provenance, SourceKind};
use ffs_skills_host::{SkillError, SkillsHost};

use crate::dispatch::{ScribeExtractError, ScribeExtractor};

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
}

#[derive(Deserialize)]
struct ScribeProvenanceWire {
    kind: String,
    uri: String,
    hash_hex: String,
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
        Proposal {
            predicate: PredicateName::new(w.predicate),
            claim: w.claim,
            provenance,
            rationale: w.rationale,
        }
    }
}

fn scribe_kind_to_source_kind(kind: &str) -> SourceKind {
    match kind {
        "ingest" | "ingest_file" => SourceKind::IngestFile,
        "federation_pull" => SourceKind::FederationPull,
        "fast_path" => SourceKind::FastPath,
        _ => SourceKind::IngestFile,
    }
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
