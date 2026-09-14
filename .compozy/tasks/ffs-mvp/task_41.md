---
status: pending
title: Morning briefing — movers-and-shakers summary from the auditor (what we do next)
type: backend
complexity: medium
dependencies:
  - task_13
  - task_38
  - task_39
  - task_40
---

# Task 41: Morning briefing — movers-and-shakers summary from the auditor (what we do next)

## Overview
Once the courier (task_40) files the paper every morning and the auto-file policy (ADR-029 / task_39) keeps the cabinets current without a review queue of thirty cards, the substrate holds the local business graph — but nobody is *reading it back*. The owner's question is not "what atoms arrived?" but "who moved, what's new, who should I call?" That is a derived view over a window of atoms: people newly seen, role and organization changes, organizations trending in the news, events by kind, people who have been mentioned often enough to deserve promotion to a real contact, and existing contacts whose organization just made news.

The auditor already owns the "read the substrate, publish a summary atom" loop (task_13: `auditor.daily_summary`, five-item panel). This task adds a second, longer output — `auditor.briefing` — computed from the atoms since the previous briefing, rendered into the vault as a `briefings/` path family so it reads like a page in Obsidian and via `ffs cat`, surfaced in the plugin panel with a "promote to contact" action that goes through the quarantine (the human gate stays), and exposed to agents through `ffs_audit_query` so an outreach agent can draft the follow-ups. The five-item discipline of the daily health summary is unchanged; the briefing is a separate atom with its own template.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST add an `auditor.briefing` predicate (starter spec + Tera template) whose claim carries: `window` (`{from, to}` ISO 8601), `new_people[]` (entity, display_name, organization, first_seen_article), `changes[]` (entity, field, from, to, source_article) for superseded scalar fields on `person.generic` / `contact.person` / `org.company`, `trending_orgs[]` (entity, display_name, mentions_this_window, mentions_prior_window), `events[]` grouped by `event.business` kind, `promotion_candidates[]` (entity, display_name, mention_count, article_count, reason), `follow_ups[]` (contact entity, display_name, org entity, triggering article), `auto_filed_count` and `reviewed_count` (from ADR-029 policy outcomes), and `narrative` (a short plain-prose paragraph). The auditor MUST compute all of this from existing RPCs (`atom.list` by predicate with a `since` watermark, `entity.search`, `audit.query`); a new daemon RPC is added ONLY if a windowed list-by-predicate cannot be expressed with what exists — document the check either way.
- MUST make the briefing cadence configurable: weekly by default (`FFS_AUDITOR_BRIEFING_INTERVAL=7d`), daily allowed (`1d`), computed from atoms with `tx_time` after the previous briefing's `window.to` (first run: last 7 days). The auditor's `invoke.input` gains `{"op": "briefing"}` alongside `tick`; the daemon-side scheduler that drives `tick` MUST also drive `briefing` on its own interval (if no scheduler exists yet for the auditor, add one, documented in `main.rs`'s env-var docblock).
- MUST define promotion thresholds as env-tunable with defaults: a `person.generic` becomes a promotion candidate when mentioned in ≥ 3 articles (`FFS_BRIEFING_PROMOTE_MIN_ARTICLES`) OR mentioned alongside an organization that an existing `contact.person` belongs to. Follow-up suggestions: any `contact.person` whose `organization` matches an `org.company` mentioned in the window. Trending: mention count this window vs the prior window of equal length, listed when this window's count is ≥ 2 and greater than prior.
- MUST render the briefing into the vault at `briefings/YYYY-MM-DD.md` via a fifth registry-declared path family (ADR-028 mechanism) with recency listing (`briefings/recent/`), using a template whose sections are, in order: Narrative, New people, Changes, Trending organizations, Events, Promotion candidates, Follow-ups, Filing stats. Every entity reference in the rendered page MUST be a wikilink to that entity's projection so the briefing is navigable in Obsidian's graph.
- MUST extend the Obsidian plugin's daily summary panel with a "Briefing" section that renders the latest `auditor.briefing` (narrative + counts + the promotion and follow-up lists) and offers two actions per promotion candidate: **Promote to contact** — submits a `contact.person` proposal built from the `person.generic` head (display_name, organization, role, aliases) into the ingest quarantine via `ingest.submit` with a `predicate: contact.person` hint and provenance pointing at the briefing atom, so the human gate is preserved; **Dismiss** — records a dismissal so the candidate is not re-listed for 30 days (persisted like task_28's plugin state, not as an atom). Existing accept/reject cards and the five-item panel are unchanged.
- MUST add an optional `kind` parameter to `audit.query` (`daily_summary` | `briefing`, default `daily_summary` for backward compatibility) and thread it through the `ffs_audit_query` MCP tool schema so an agent can read the latest briefing and draft outreach; `ffs health` SHOULD gain `--briefing` to print the latest one.
- MUST keep the auditor Python stdlib-only (ADR-009) and MUST NOT let the briefing computation exceed the skill's `timeout_ms`; if the window's atom count exceeds a documented ceiling the auditor truncates lists (top-N by count) and says so in `narrative`.
- MUST NOT auto-promote or auto-contact anyone: every promotion is a quarantined proposal; every follow-up is a suggestion in text. The briefing reads the graph; it does not write to it except to publish itself.
- SHOULD describe the briefing (what it contains, how often, how to change cadence) in `docs/onboarding/first-use-guide.md` and add the `briefings/` family to the path-library overview.
</requirements>

## Subtasks
- [ ] 41.1 `auditor.briefing` predicate spec + Tera template + `briefings/` path family declaration (ADR-028 mechanism); render test.
- [ ] 41.2 Auditor `briefing` op: window computation from the previous briefing's `window.to`, new-people / changes / trending / events / promotion / follow-up derivations over existing RPCs; documented check on whether a new RPC is needed.
- [ ] 41.3 Scheduler: daemon drives `tick` and `briefing` on their own intervals; env vars documented; first-run window is 7 days.
- [ ] 41.4 `audit.query` `kind` parameter (daemon + `api.rs`), `ffs_audit_query` MCP schema/translator update, `ffs health --briefing`.
- [ ] 41.5 Plugin "Briefing" section: render latest briefing, **Promote to contact** (quarantined `contact.person` proposal with briefing provenance), **Dismiss** (30-day local suppression); vitest coverage.
- [ ] 41.6 e2e: file two CBJ-shaped articles (task_40 fixtures) mentioning the same new person and an org an existing contact belongs to; run the auditor `briefing` op; assert one `auditor.briefing` atom, a `briefings/<date>.md` file with wikilinks, one promotion candidate, one follow-up.
- [ ] 41.7 Docs: first-use-guide briefing section; path-library overview gains `briefings/`; `skills/auditor/SKILL.md` documents the new op and wire shape.

## Implementation Details
Current structure: `skills/auditor/audit.py` exposes `handle({"op": "tick", "window_hours": N})` → `aggregate_metrics` (via `health.summary`) → `evaluate_flags` → `build_claim` → `publish` (`audit.publish_summary`). `crates/ffs-daemon/src/dispatch.rs::audit_query` lists atoms for the fixed entity `auditor` and predicate `auditor.daily_summary`, capability-filters, and sorts newest first; `AuditQueryParams` has only `since`. The daemon's `main.rs` currently wires the scribe and the ingest watcher; there is no periodic auditor driver visible in the binary (task_13's tick is invoked by tests and the plugin path) — 41.3 must confirm this and add the driver if absent. The plugin's `SummaryPanelModel` (`obsidian-plugin/src/summary.ts`) owns pending-proposal cards and `accept` / `reject`; the briefing section is a sibling model fed by `audit.query` with `kind: "briefing"`.

"Changes" are derived from supersession: for each `person.generic` / `contact.person` / `org.company` atom committed in the window whose `supersedes` is set, diff the scalar fields of the head against its parent; array-field growth (a new `mentions[]` entry) is not a change, it is a mention. This is why ADR-029's additive-vs-conflict routing matters upstream: conflicts that the owner reviewed and accepted are exactly the changes worth briefing.

The briefing publishes through the same `audit.publish_summary` path with `predicate: auditor.briefing` (the RPC gains a `predicate` argument restricted to the two auditor predicates), so signing, capability, and materialization behave identically to the daily summary.

### Relevant Files
- `skills/auditor/audit.py`, `skills/auditor/SKILL.md`, `skills/auditor/tests/test_audit.py` — `briefing` op and derivations.
- `starter/predicates/auditor.briefing.toml`, `starter/templates/auditor-briefing.md.tera` — new.
- `crates/ffs-core/src/projection/path.rs` (or the ADR-028 registry-declared family loader) — `briefings/` family.
- `crates/ffs-daemon/src/dispatch.rs`, `crates/ffs-daemon/src/api.rs` — `audit.query` `kind`, `audit.publish_summary` `predicate`.
- `crates/ffs-daemon/src/main.rs` — auditor scheduler + env docblock.
- `crates/ffs-mcp/src/tools.rs` — `ffs_audit_query` `kind`.
- `crates/ffs-cli/src/commands.rs` — `ffs health --briefing`.
- `obsidian-plugin/src/summary.ts`, `obsidian-plugin/src/main.ts`, `obsidian-plugin/tests/summary.test.ts` — Briefing section, promote/dismiss.

### Dependent Files
- `crates/ffs-daemon/tests/auditor_integration.rs` — briefing e2e.
- `crates/ffs-daemon/tests/summary_panel_integration.rs` — promote-to-contact lands in quarantine.
- `docs/onboarding/first-use-guide.md` — briefing section and `briefings/` in the path overview.

### Related ADRs
- [ADR-028] — registry-declared path families (the `briefings/` family) and the business-graph predicates the briefing reads.
- [ADR-029] — auto-file policy; `auto_filed_count` / `reviewed_count` and the conflict-routing that makes "changes" meaningful.
- [ADR-027: Adopt the OKF Agent Memory Convention](adrs/adr-027.md) — the briefing is the "review knowledge after substantial work" step, performed by the substrate on the owner's behalf; promotions stay proposals.
- [ADR-013: MCP server in MVP](adrs/adr-013.md) — `ffs_audit_query` remains a thin pass-through; `kind` is an additive parameter.
- [ADR-009: Claw integration](adrs/adr-009.md) — auditor stays stdlib-only.
- PRD § Open Questions — "promotion flow from `person.generic` to `contact.person`" is answered here as a quarantined proposal from the briefing panel.

## Deliverables
- `auditor.briefing` predicate, template, `briefings/` path family **(REQUIRED)**.
- Auditor `briefing` op with all seven derivations and configurable cadence **(REQUIRED)**.
- Daemon scheduler for `tick` + `briefing`; `audit.query` `kind`; `ffs_audit_query` and `ffs health --briefing` **(REQUIRED)**.
- Plugin Briefing section with **Promote to contact** (quarantined) and **Dismiss** **(REQUIRED)**.
- Onboarding docs and `SKILL.md` updates.
- Unit tests with 80%+ coverage on new Python, Rust, and TypeScript code **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] `test_briefing_lists_new_people_first_seen_in_window` — fixture atoms: a `person.generic` created in-window appears in `new_people` with its first article.
  - [ ] `test_briefing_detects_role_change_via_supersession` — superseded `role` on a person yields one `changes[]` entry with from/to and the source article.
  - [ ] `test_briefing_trending_requires_growth_over_prior_window` — org with 3 mentions vs 1 prior is listed; 1 vs 1 is not.
  - [ ] `test_briefing_promotion_threshold_by_article_count` — 3 articles → candidate; 2 → not (default threshold).
  - [ ] `test_briefing_promotion_by_shared_org_with_existing_contact`.
  - [ ] `test_briefing_follow_ups_for_contacts_whose_org_made_news`.
  - [ ] `test_briefing_window_starts_at_previous_briefing_end` / `test_first_briefing_window_is_seven_days`.
  - [ ] `test_briefing_truncates_over_ceiling_and_says_so_in_narrative`.
  - [ ] Rust: `auditor_briefing_template_renders_all_sections_with_wikilinks`; `audit_query_kind_filters_briefing_from_daily_summary`; `audit_publish_summary_rejects_non_auditor_predicate`.
  - [ ] MCP: `ffs_audit_query_passes_kind_filter`.
  - [ ] Plugin (vitest): `briefing_section_renders_latest_briefing`; `promote_to_contact_submits_quarantined_proposal_with_briefing_provenance`; `dismiss_suppresses_candidate_for_thirty_days`.
- Integration tests:
  - [ ] `briefing_after_two_articles_yields_atom_file_candidate_and_follow_up` — `auditor_integration`.
  - [ ] `promote_to_contact_lands_in_quarantine_not_in_store` — `summary_panel_integration`.
  - [ ] Scheduler smoke: with `FFS_AUDITOR_BRIEFING_INTERVAL=1s` in a test daemon, a briefing atom appears within the test budget.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- After a week of courier drops, opening Obsidian shows `briefings/<date>.md` answering, in order, who is new, who moved, which organizations are trending, what happened, who to promote, and who to call — every name a clickable wikilink.
- Promoting a candidate produces a `contact.person` card in the daily summary panel, not an atom; nothing reaches the store without the owner's accept.
- An MCP agent can read the latest briefing with `ffs_audit_query {kind: "briefing"}` and the daily summary is unchanged for callers that omit `kind`.
- The auditor remains stdlib-only and completes a briefing over a 500-atom window inside its timeout.
