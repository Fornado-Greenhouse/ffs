"""The `heuristic` extraction engine: today's regex and frontmatter
scribe behind the `ExtractionEngine` seam (task_36 § 36.1).

Behaviorally identical to the pre-task_36 ``handle()`` decision flow,
plus the bounded hygiene fixes in ``extraction.py`` (filename as a name
candidate, card-shape parsing, field-label stop words). Always
available, fully offline, zero configuration; remains the default and
the fallback for the llm engine.
"""

from __future__ import annotations

from typing import Any, Dict, List

import extraction as ex
from engine import EngineResult, Submission, make_proposal


class HeuristicEngine:
    name = "heuristic"
    model = ""

    def extract(self, submission: Submission, registry: Any) -> EngineResult:
        _ = registry  # the heuristic engine hardcodes the three MVP predicate shapes
        fm = submission.frontmatter
        sections = submission.sections
        content_text = submission.content_text
        proposals: List[Dict[str, Any]] = []
        warnings: List[str] = []

        def mk(predicate: str, claim: Dict[str, Any], rationale: str) -> Dict[str, Any]:
            return make_proposal(predicate, claim, submission, rationale, self.name, self.model)

        # Conflict detection: frontmatter name vs a body `name:` line.
        if conflicting := ex._conflicting_name(fm, content_text):
            proposals.append(
                mk(
                    "note",
                    {
                        "title": f"name conflict for {ex._name_field(fm)}",
                        "body": (
                            f"frontmatter says name={ex._name_field(fm)!r}; "
                            f"body says name={conflicting!r}. Reconcile manually."
                        ),
                        "tags": ["scribe-ambiguity"],
                    },
                    "structural-ambiguity: conflicting name claims",
                )
            )

        if (claim := ex.extract_contact_person(fm, sections)) is not None:
            proposals.append(
                mk(
                    "contact.person",
                    claim,
                    "extracted display_name + contact fields from frontmatter and `Notes` section",
                )
            )
        elif (claim := ex.extract_person_generic(fm, sections)) is not None:
            proposals.append(
                mk("person.generic", claim, "extracted display_name + role/team from frontmatter")
            )
        elif (
            not ex._name_field(fm)
            and (
                unstructured := ex.extract_contact_person_unstructured(
                    content_text, filename=submission.filename
                )
            )
            is not None
        ):
            unstructured_claim, unstructured_signals = unstructured
            proposals.append(
                mk(
                    "contact.person",
                    unstructured_claim,
                    "matched "
                    + str(len(unstructured_signals))
                    + " unstructured-contact signals: "
                    + ", ".join(unstructured_signals),
                )
            )

        # Always emit a note for unexplained content, unless the
        # structured extraction already captured everything. A parsed
        # key-value card is fully captured by its contact proposal, so
        # a fallback note would only duplicate the card lines.
        card_captured = any(
            p["predicate"] == "contact.person" and "key-value card" in p.get("rationale", "")
            for p in proposals
        )
        if not card_captured and (not proposals or (proposals and content_text.strip())):
            note_claim = ex.extract_note(fm, sections, content_text)
            body_text = note_claim.get("body", "")
            already_have_structured = any(p["predicate"] != "note" for p in proposals)
            if not already_have_structured or (
                body_text and not ex._body_only_in_notes_section(sections)
            ):
                from contract import parse_references, truncate_body

                rationale = "fallback note from raw markdown body"
                text, warning = truncate_body(body_text)
                if warning:
                    note_claim["body"] = text
                    rationale += "; " + warning
                    warnings.append(warning)
                for sec_name, lines in sections:
                    if sec_name.strip().lower() == "references":
                        refs = parse_references(lines)
                        if refs:
                            note_claim["references"] = refs
                        break
                proposals.append(mk("note", note_claim, rationale))

        return EngineResult(proposals=proposals, warnings=warnings)
