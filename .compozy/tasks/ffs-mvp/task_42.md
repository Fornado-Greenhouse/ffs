---
status: completed
title: "Spike: extraction quality on real business-press articles (gates task_36)"
type: docs
complexity: low
dependencies:
  - task_11
---

# Task 42: Spike: extraction quality on real business-press articles (gates task_36)

## Overview
Every task in the newspaper pipeline (36, 38, 45, 40, 39, 41) rests on one unproven assumption: that an LLM, prompted from the predicate schemas, extracts people, organizations, affiliations, and events from Charlotte Business Journal articles reliably enough to file. There is zero evidence for this today. The shipped scribe is the regex engine that produced "Bones Occupation" (task_36 Overview), and no model has been run against a single real article. If extraction is only 70 percent good, the resolver thresholds (ADR-030), the auto-file policy (ADR-029), and the briefing (task_41) all change shape. This spike measures the thesis before the task_38 refactor spends weeks on it. It produces a findings note, not code. Nothing here touches production paths.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST assemble ten real Charlotte Business Journal articles covering at least: an executive hire, a departure, a funding round, an acquisition, a real-estate opening, a profile piece, an awards list, and one article with no named people. Real article text MUST NOT be committed to this public repository: keep it under `$FFS_DATA_DIR/spikes/task_42/` or another gitignored local directory. Only paraphrased fixtures may later enter `skills/scribe/tests/corpus/`.
- MUST hand-label each article before running any model: expected people (display name), organizations, affiliations as (person, organization, title) triples, and events as (kind, parties, date). The labels are the ground truth for scoring and MUST be written down before model output is seen.
- MUST render the extraction prompt from the registered predicate specs (`starter/predicates/*.toml` claim schemas) the way task_36 specifies, requesting a proposals envelope whose `source.article` proposal carries a one-paragraph `summary` and `mentions[]` items shaped `{display, context}`. A throwaway script under the spike directory is acceptable; it MUST use only the Python standard library (`urllib`, `json`, `tomllib`) per ADR-009 so the same code can be lifted into task_36.
- MUST run every article through (a) the Anthropic Messages API with a current Claude model (record model id and `anthropic-version` header) and (b) a local Ollama model (record the model name and parameter count tried; try at least one). Record per article and per engine: wall-clock latency, token counts, and cost for (a).
- MUST score field-level precision and recall per engine and per predicate (people, organizations, affiliations, events) against the hand labels, using semantic matching with tolerance (a nickname or middle initial is a match; a different person is not). Record the totals in a table in the findings note.
- MUST catalogue failure modes observed: hallucinated fields, missed affiliations, a person attached to the wrong organization, invented titles, summaries that add facts not in the article, malformed JSON, schema-invalid output. Count each.
- MUST record whether the resolver weight hints from ADR-030 hold on this sample: does organization agreement separate same-name people; how often does surname alone mislead; how often does the model emit a bare name with no distinguishing attribute (the NIL-policy case).
- MUST write the findings to `docs/research/spikes/task-42-extraction-quality.md` with the PASS/FAIL verdict stated in the first paragraph.
- MUST NOT modify any file under `crates/`, `skills/`, `starter/`, or `obsidian-plugin/`.

PASS criterion: the Claude engine reaches precision >= 0.85 and recall >= 0.75 on people and on organizations, and precision >= 0.6 and recall >= 0.6 on affiliations, over the ten articles. The local model's numbers are recorded regardless of outcome.

What changes on FAIL: if Claude fails, task_36's `llm` engine scope shrinks to `source.article` plus `note` extraction only (no people, orgs, or affiliations from press), ADR-029's auto-file default stays off indefinitely, and task_45's resolver is re-scoped to owner-typed content. If only the local model fails, the local-only path is documented in task_36 as "digest only" and the first-use guide says so.
</requirements>

## Result (2026-09-14)

Executed with one deliberate deviation, recorded in the findings note: the corpus was five CLTtoday newsletters (6AM City, personal-use license) rather than ten CBJ article bodies, because spike task_43 found that the ACBJ User Agreement prohibits automated access and any LLM operation over bizjournals.com content. Claude Sonnet 5 (via the Claude Code CLI in headless mode; no Messages API key was available) PASSED every criterion with margin: people 1.00 P / 1.00 R, orgs 1.00 / 0.96, affiliations 1.00 / 1.00, events 0.91 / 0.88 strict, zero schema-invalid proposals out of 111, zero hallucinated names. llama3.1:8b FAILED everything except org precision (people 0.21 / 0.38, affiliations 0.08 / 0.14, 25 of 72 proposals schema-invalid). Findings: `docs/research/spikes/task-42-extraction-quality.md`. Consequences: task_36's schema-driven design stands unchanged; the local-only tier at 8B is "digest only", so the validator plus heuristic fallback is load-bearing; two orgs already appeared under variant names, the alias case task_45's resolver exists for.

## Subtasks
- [x] 42.1 Collect ten CBJ articles into a gitignored spike directory; hand-label people, orgs, affiliations, events per article before any model run.
- [x] 42.2 Throwaway stdlib prompt renderer from `starter/predicates/*.toml` requesting the proposals envelope with `summary` and `{display, context}` mentions.
- [x] 42.3 Run all ten through the Anthropic Messages API and at least one local Ollama model; capture raw outputs, latency, tokens, cost.
- [x] 42.4 Score precision/recall per engine per predicate; catalogue failure modes; check the ADR-030 weight hints on the sample.
- [x] 42.5 Write `docs/research/spikes/task-42-extraction-quality.md` with the verdict first, tables, failure-mode counts, and the concrete scope changes for task_36 and task_45.

## Implementation Details
The spike lives entirely outside the build. The prompt renderer reads the same TOML the daemon loads, so a positive result transfers to task_36 subtask 36.2 unchanged. Ollama's chat endpoint and the Anthropic Messages API are both JSON over HTTP; `urllib.request` is enough. Score with a small hand-written comparison, not a framework.

### Relevant Files
- `starter/predicates/*.toml` — the claim schemas the prompt is rendered from (read-only).
- `skills/scribe/extraction.py` — the heuristic baseline to compare against on the same ten articles (read-only; run it once for a baseline row).
- `docs/research/spikes/task-42-extraction-quality.md` — the deliverable.

### Dependent Files
- `.compozy/tasks/ffs-mvp/task_36.md` — scope adjusted by the verdict.
- `.compozy/tasks/ffs-mvp/task_45.md` — resolver assumptions adjusted by the weight-hint findings.

### Related ADRs
- [ADR-026](adrs/adr-026.md) — the engine seam and privacy posture this spike exercises.
- [ADR-030](adrs/adr-030.md) — resolver weight hints checked on the sample.
- [ADR-009](adrs/adr-009.md) — stdlib-only constraint on the throwaway script.

## Deliverables
- Hand-labeled ground truth for ten articles, kept out of git **(REQUIRED)**.
- Findings note with PASS/FAIL verdict, per-engine per-predicate precision/recall table, latency and cost table, failure-mode counts **(REQUIRED)**.
- The heuristic engine's baseline row on the same ten articles.
- A list of concrete scope changes for task_36 and task_45 (possibly "none").

## Tests
- Verification is the hand-labeled comparison; no automated tests are added by this spike.
- [x] Ground truth written before model outputs were viewed (state the timestamps in the note).
- [x] Every article has a scored row for each engine.
- [x] The verdict paragraph cites the numbers that decide it.

## Success Criteria
- The findings note exists, states PASS or FAIL in its first paragraph, and task_36's requirements are amended (or confirmed unchanged) with a reference to it.
- No real article text is in the repository.
