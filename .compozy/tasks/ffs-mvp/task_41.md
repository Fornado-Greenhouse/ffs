---
status: completed
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

Amended 2026-09-14 (ADR-030, ADR-031). Two research findings reshape the derivations. Under ADR-031 a role is its own `affiliation` atom, not a scalar on the person, so "who moved" is read from affiliation activity: a new affiliation atom is a join, a supersession that sets `valid_to` is a departure, a supersession that changes `title` is a promotion or retitle. Under ADR-030 entity ids are opaque and permanent, so "people newly seen" means entity ids minted in the window, and every entity reference the briefing renders is a `[[target|display]]` wikilink keyed by id, never by name. The briefing also becomes the place where the resolver's open questions surface to the owner: proposals the resolver marked `ambiguous` are listed under "Needs your eye" with their candidate list, and pairs of entities whose aliases overlap are listed under "Possible duplicates" with a one-click merge that writes an `entity.same_as` atom, undoable by supersession.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST add an `auditor.briefing` predicate (starter spec + Tera template) whose claim carries: `window` (`{from, to}` ISO 8601), `new_people[]` (entity, display_name, organization, first_seen_article; an entity is new when its id was minted in the window, per ADR-030's opaque ids), `changes[]` (entity, kind `joined` | `left` | `retitled` | `org_changed`, organization, from, to, source_article) derived from `affiliation` atom activity per ADR-031 (a new affiliation atom is `joined`; a supersession setting `valid_to` is `left`; a supersession changing `title` is `retitled`; scalar supersession on `org.company` such as a rename or relocation is `org_changed`), `trending_orgs[]` (entity, display_name, mentions_this_window, mentions_prior_window), `events[]` grouped by `event.business` kind with participants as entity references, `promotion_candidates[]` (entity, display_name, mention_count, article_count, reason), `follow_ups[]` (contact entity, display_name, org entity, triggering article), `needs_your_eye[]` (submission_id, display, candidates `[{entity, display_name, score}]`) for quarantined proposals whose resolver outcome is `ambiguous`, `possible_duplicates[]` (entity_a, entity_b, shared_aliases) for pairs of live entities in the same family whose `display_name` or `aliases[]` overlap and that are not already related by `entity.same_as` or `entity.different_from`, `auto_filed_count` and `reviewed_count` (from ADR-029 policy outcomes), and `narrative` (a short plain-prose paragraph). Every entity field carries the opaque id; display names are carried alongside for rendering only. The auditor MUST compute all of this from existing RPCs (`atom.list` by predicate with a `since` watermark, `entity.search`, `audit.query`); a new daemon RPC is added ONLY if a windowed list-by-predicate cannot be expressed with what exists: document the check either way.
- MUST make the briefing cadence configurable: weekly by default (`FFS_AUDITOR_BRIEFING_INTERVAL=7d`), daily allowed (`1d`), computed from atoms with `tx_time` after the previous briefing's `window.to` (first run: last 7 days). The auditor's `invoke.input` gains `{"op": "briefing"}` alongside `tick`; the daemon-side scheduler that drives `tick` MUST also drive `briefing` on its own interval (if no scheduler exists yet for the auditor, add one, documented in `main.rs`'s env-var docblock).
- MUST define promotion thresholds as env-tunable with defaults: a `person.generic` becomes a promotion candidate when mentioned in ≥ 3 articles (`FFS_BRIEFING_PROMOTE_MIN_ARTICLES`) OR affiliated (via an `affiliation` atom, or the person's convenience `organization` field when no affiliation atom exists yet) with an organization that an existing `contact.person` is affiliated with. Follow-up suggestions: any `contact.person` with a current `affiliation` atom (no `valid_to`) whose organization entity is mentioned in the window, falling back to the contact's `organization` scalar matched by resolved entity id, never by string. Trending: mention count this window vs the prior window of equal length, listed when this window's count is ≥ 2 and greater than prior.
- MUST render the briefing into the vault at `briefings/YYYY-MM-DD.md` via a fifth registry-declared path family (ADR-028 mechanism) with recency listing (`briefings/recent/`), using a template whose sections are, in order: Narrative, Needs your eye, New people, Changes, Trending organizations, Events, Promotion candidates, Follow-ups, Possible duplicates, Filing stats. Every entity reference in the rendered page MUST be a `[[target|display]]` wikilink resolved from the entity id to that entity's unique projection basename (ADR-030 naming rule), so the briefing is navigable in Obsidian's graph and survives renames.
- MUST extend the Obsidian plugin's daily summary panel with a "Briefing" section that renders the latest `auditor.briefing` (narrative + counts + the promotion and follow-up lists) and offers two actions per promotion candidate: **Promote to contact**: submits a `contact.person` proposal built from the `person.generic` head (display_name, organization, role, aliases) into the ingest quarantine via `ingest.submit` with a `predicate: contact.person` hint and provenance pointing at the briefing atom, so the human gate is preserved; **Dismiss**: records a dismissal so the candidate is not re-listed for 30 days (persisted like task_28's plugin state, not as an atom). The section MUST also render "Needs your eye" items as the reconciliation picker task_39 introduces (choose a candidate, or "someone new"), so an ambiguous proposal can be resolved from the briefing without opening the quarantine list, and "Possible duplicates" items with two actions per pair: **Merge**: writes an owner-signed `entity.same_as` atom through `entity.merge` (task_39) (the losing entity redirects to the winner; history stays in place; undo is supersession of the same-as atom, offered as an **Undo merge** action on the next briefing and in the daily summary's auto-filed section); **Keep separate**: writes an owner-signed `entity.different_from` atom so the pair is never suggested again and the resolver never auto-links them. Existing accept/reject cards and the five-item panel are unchanged.
- MUST add an optional `kind` parameter to `audit.query` (`daily_summary` | `briefing`, default `daily_summary` for backward compatibility) and thread it through the `ffs_audit_query` MCP tool schema so an agent can read the latest briefing and draft outreach; `ffs health` SHOULD gain `--briefing` to print the latest one.
- MUST keep the auditor Python stdlib-only (ADR-009) and MUST NOT let the briefing computation exceed the skill's `timeout_ms`; if the window's atom count exceeds a documented ceiling the auditor truncates lists (top-N by count) and says so in `narrative`.
- MUST NOT auto-promote, auto-contact, or auto-merge anyone: every promotion is a quarantined proposal; every follow-up is a suggestion in text; every merge and every "keep separate" is an explicit owner click that produces an owner-signed atom. The briefing reads the graph; it does not write to it except to publish itself.
- MUST label every affiliation date in the briefing as "as reported <date>" (the article's `published_at` or the affiliation atom's `valid_from`, whichever the resolver set), because press reports announcements, not start dates; a known gap carried from the 2026-09-14 plan review, not a bug to fix here.
- SHOULD describe the briefing (what it contains, how often, how to change cadence) in `docs/onboarding/first-use-guide.md` and add the `briefings/` family to the path-library overview.
</requirements>

## Subtasks
- [x] 41.1 `auditor.briefing` predicate spec + Tera template + `briefings/` path family declaration (ADR-028 mechanism); render test.
- [x] 41.2 Auditor `briefing` op: window computation from the previous briefing's `window.to`; new-people from entity ids minted in the window; changes from `affiliation` atom activity (joined / left / retitled) plus `org.company` scalar supersession (org_changed); trending / events / promotion / follow-up derivations keyed by entity id over existing RPCs; documented check on whether a new RPC is needed.
- [x] 41.3 Scheduler: daemon drives `tick` and `briefing` on their own intervals; env vars documented; first-run window is 7 days.
- [x] 41.4 `audit.query` `kind` parameter (daemon + `api.rs`), `ffs_audit_query` MCP schema/translator update, `ffs health --briefing`.
- [x] 41.5 Plugin "Briefing" section: render latest briefing, **Promote to contact** (quarantined `contact.person` proposal with briefing provenance), **Dismiss** (30-day local suppression); "Needs your eye" rendered as the reconciliation picker; "Possible duplicates" with **Merge** (owner-signed `entity.same_as` via `entity.merge` (task_39)), **Keep separate** (owner-signed `entity.different_from`), and **Undo merge** (supersede the same-as atom); vitest coverage.
- [x] 41.6 e2e: file two CBJ-shaped articles (task_40 fixtures) mentioning the same new person and an org an existing contact is affiliated with; run the auditor `briefing` op; assert one `auditor.briefing` atom, a `briefings/<date>.md` file with `[[target|display]]` wikilinks, one promotion candidate, one follow-up.
- [x] 41.7 Docs: first-use-guide briefing section (including what "Needs your eye" and "Possible duplicates" ask of the owner and that merges are undoable); path-library overview gains `briefings/`; `skills/auditor/SKILL.md` documents the new op and wire shape.
- [x] 41.8 Resolver-facing derivations: `needs_your_eye[]` from quarantined proposals with resolver outcome `ambiguous` (via `ingest.list_pending`), `possible_duplicates[]` from alias overlap across live entities in the same family excluding pairs already related by `entity.same_as` or `entity.different_from`; both capped by the documented ceiling and ordered by overlap count.

## Implementation Details

**As built (2026-09-20).** Two findings reshaped the work and are recorded here and in ADR-037.

- *RPC check (requirement 1).* A windowed list-by-predicate could not be expressed: `atom.list` demanded an `entity`, `audit.query` lists only the auditor's own atoms, and `entity.search` is ranked and limited. Rather than a new method, `atom.list` gained an entity-less form `{predicate, since?, limit?}` (default 1000, ceiling 5000, newest first, capability-filtered) and every `atom.list` / `audit.query` row now carries its content `hash` (additive). The JSON-RPC method set gained `audit.run {op: tick|briefing, window_days?}` for on-demand runs, mirroring `courier.run`.
- *Skills could not query the substrate in production.* The daemon installed `RefuseAllProxy` for the skills host, so the auditor's `tick` had never published from the binary either. `ffs_daemon::DispatcherProxy` (ADR-037) routes skill queries through the in-process dispatcher over a fixed allow-list (reads, `audit.publish_summary`, `ingest.submit`, `working_set.*`); accept, merge, retract, grant, and federation stay unreachable from bundles. The scheduler (`crates/ffs-daemon/src/scheduler.rs`) drives `tick` (24h) and `briefing` (7d) from `FFS_AUDITOR_TICK_INTERVAL` / `FFS_AUDITOR_BRIEFING_INTERVAL`; first run one interval after boot; `0`/`off` disables.
- *Flat path layout.* `briefings/<date>.md` needs a family whose name field is a date, which the `by-name/<letter>/` layout cannot file. `[path]` gained `layout = "by_name" | "flat"` (default `by_name`); `FamilyEntry`, `PathFamily`, `path_for_basename`, and `parse` honor it; `recent/` works for both. The plugin's `paths.ts` still assumes `by-name` for editing classification; a flat page is read-only so this only means an edit to it routes to ingest as a correction (no reverse-map rules), and the plugin's folder view for `briefings/` shows the `recent/` listing.
- *Wikilinks without field names in code.* The renderer adds `claim_resolved` to every template context: a deep copy of the claim in which every object carrying a string `entity` gains `basename` (from the path index) and a `display` when missing. The briefing template writes `[[basename|display]]` from that, so it survives renames (tested) and the substrate names no briefing field.
- *Claim contract.* Every entity reference is `{entity, display}`; sections as the requirement lists plus `recent_merges[]` (for Undo merge on the next briefing) and `filing {auto_filed_count, reviewed_count}` computed from provenance kinds over the window's atoms. Documented in `skills/auditor/SKILL.md`.
- *Deviations, deliberate.* `as_reported` for a departure is the affiliation's `valid_to` date (the requirement's `valid_from` would label the start date as the report date). Duplicate exclusion ignores superseded `same_as` / `different_from` atoms so an undone merge makes the pair a candidate again. The plugin's per-briefing suppression (merged, kept-separate, promoted, resolved) is in memory keyed by briefing hash; only dismissals persist, as required. "Undo merge" is offered in the briefing section (from `recent_merges[]` and session merges), not additionally in the daily summary's auto-filed section.

Current structure: `skills/auditor/audit.py` exposes `handle({"op": "tick", "window_hours": N})` → `aggregate_metrics` (via `health.summary`) → `evaluate_flags` → `build_claim` → `publish` (`audit.publish_summary`). `crates/ffs-daemon/src/dispatch.rs::audit_query` lists atoms for the fixed entity `auditor` and predicate `auditor.daily_summary`, capability-filters, and sorts newest first; `AuditQueryParams` has only `since`. The daemon's `main.rs` currently wires the scribe and the ingest watcher; there is no periodic auditor driver visible in the binary (task_13's tick is invoked by tests and the plugin path) — 41.3 must confirm this and add the driver if absent. The plugin's `SummaryPanelModel` (`obsidian-plugin/src/summary.ts`) owns pending-proposal cards and `accept` / `reject`; the briefing section is a sibling model fed by `audit.query` with `kind: "briefing"`.

"Changes" for people are derived from `affiliation` atoms (ADR-031), not from person scalars: an affiliation atom committed in the window with no `supersedes` is a join; one whose parent had no `valid_to` and whose head sets `valid_to` is a departure; one whose `title` differs from its parent's is a retitle. Organization changes still come from scalar supersession on `org.company` (name, location). Array-field growth on a person (a new `mentions[]` entry) is not a change, it is a mention. This lines up with ADR-029's routing: a join is additive and may auto-file, a departure or retitle supersedes an existing atom and was reviewed by the owner, so the briefing's "Changes" section is by construction the set of things the owner already confirmed plus the joins the policy filed. "New people" is the set of entity ids whose first atom has `tx_time` inside the window, which is well defined only because ADR-030 makes ids opaque and permanent; a renamed person keeps her id and is not "new".

The resolver surfaces two kinds of open question, and the briefing is where they belong rather than the five-item health panel: `ambiguous` proposals (ADR-030's middle band, always routed to review by ADR-029) and alias-overlap pairs the resolver noticed but had no proposal to attach to. Rendering them in the briefing with the picker and the merge / keep-separate actions closes the loop the entity-resolution memo describes: every owner decision either grows an alias table or writes a `different_from` that the resolver honors forever after.

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
- [ADR-030](adrs/adr-030.md): opaque permanent ids (what "new" means), `ambiguous` outcomes and the reconciliation picker, `entity.same_as` / `entity.different_from` written from the briefing's merge and keep-separate actions.
- [ADR-031](adrs/adr-031.md): `affiliation` as its own role atom; joins, departures, and retitles are read from affiliation activity; participants are entity references.
- [ADR-028] — registry-declared path families (the `briefings/` family) and the business-graph predicates the briefing reads.
- [ADR-029] — auto-file policy; `auto_filed_count` / `reviewed_count` and the conflict-routing that makes "changes" meaningful.
- [ADR-027: Adopt the OKF Agent Memory Convention](adrs/adr-027.md) — the briefing is the "review knowledge after substantial work" step, performed by the substrate on the owner's behalf; promotions stay proposals.
- [ADR-013: MCP server in MVP](adrs/adr-013.md) — `ffs_audit_query` remains a thin pass-through; `kind` is an additive parameter.
- [ADR-009: Claw integration](adrs/adr-009.md) — auditor stays stdlib-only.
- PRD § Open Questions — "promotion flow from `person.generic` to `contact.person`" is answered here as a quarantined proposal from the briefing panel.

## Deliverables
- `auditor.briefing` predicate, template, `briefings/` path family **(REQUIRED)**.
- Auditor `briefing` op with all nine derivations (including `needs_your_eye` and `possible_duplicates`) and configurable cadence **(REQUIRED)**.
- Daemon scheduler for `tick` + `briefing`; `audit.query` `kind`; `ffs_audit_query` and `ffs health --briefing` **(REQUIRED)**.
- Plugin Briefing section with **Promote to contact** (quarantined), **Dismiss**, the reconciliation picker, **Merge** / **Keep separate** / **Undo merge** **(REQUIRED)**.
- Onboarding docs and `SKILL.md` updates.
- Unit tests with 80%+ coverage on new Python, Rust, and TypeScript code **(REQUIRED)**.

## Tests
- Unit tests:
  - [x] `test_briefing_new_people_from_first_seen_entity_ids`: fixture atoms: an entity id whose first atom is in-window appears in `new_people` with its first article; a renamed person (superseded `display_name`, same id) does not.
  - [x] `test_briefing_detects_joined_from_new_affiliation_atom`: a root `affiliation` atom in-window yields one `changes[]` entry of kind `joined` with the organization and source article.
  - [x] `test_briefing_detects_left_from_affiliation_valid_to`: a supersession that sets `valid_to` yields kind `left`.
  - [x] `test_briefing_detects_retitle_from_affiliation_title_supersession`: a supersession changing `title` yields kind `retitled` with from/to.
  - [x] `test_briefing_person_scalar_supersession_is_not_a_change`: a superseded `location` on a person yields no `changes[]` entry.
  - [x] `test_briefing_lists_ambiguous_proposals_as_needs_your_eye`: a pending proposal with resolver outcome `ambiguous` appears with its candidate list; `existing` and `new` proposals do not.
  - [x] `test_briefing_lists_alias_overlap_pairs_as_possible_duplicates`: two live people sharing an alias appear once as a pair; a pair already related by `same_as` or `different_from` does not.
  - [x] `test_briefing_trending_requires_growth_over_prior_window` — org with 3 mentions vs 1 prior is listed; 1 vs 1 is not.
  - [x] `test_briefing_promotion_threshold_by_article_count` — 3 articles → candidate; 2 → not (default threshold).
  - [x] `test_briefing_promotion_by_shared_org_with_existing_contact`.
  - [x] `test_briefing_follow_ups_for_contacts_whose_org_made_news`.
  - [x] `test_briefing_window_starts_at_previous_briefing_end` / `test_first_briefing_window_is_seven_days`.
  - [x] `test_briefing_truncates_over_ceiling_and_says_so_in_narrative`.
  - [x] Rust: `auditor_briefing_template_renders_all_sections_with_target_display_wikilinks` (every entity link is `[[basename|display]]` resolved from the id); `audit_query_kind_filters_briefing_from_daily_summary`; `audit_publish_summary_rejects_non_auditor_predicate`.
  - [x] MCP: `ffs_audit_query_passes_kind_filter`.
  - [x] Plugin (vitest): `briefing_section_renders_latest_briefing`; `promote_to_contact_submits_quarantined_proposal_with_briefing_provenance`; `dismiss_suppresses_candidate_for_thirty_days`; `needs_your_eye_picker_resolves_ambiguous_proposal_to_chosen_candidate_or_new`; `merge_possible_duplicate_writes_same_as_and_is_undoable`; `keep_separate_writes_different_from_and_pair_is_not_relisted`.
- Integration tests:
  - [x] `briefing_after_two_articles_yields_atom_file_candidate_and_follow_up` — `auditor_integration`.
  - [x] `briefing_after_join_and_departure_lists_both_changes_from_affiliation_atoms`: `auditor_integration`.
  - [x] `promote_to_contact_lands_in_quarantine_not_in_store` — `summary_panel_integration`.
  - [x] `merge_from_briefing_redirects_losing_entity_and_undo_restores_it`: `summary_panel_integration`; after merge, `entity.search` for the losing alias returns the winner; after undo, both entities are live again with no atoms moved.
  - [x] Scheduler smoke: with `FFS_AUDITOR_BRIEFING_INTERVAL=1s` in a test daemon, a briefing atom appears within the test budget.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- After a week of courier drops, opening Obsidian shows `briefings/<date>.md` answering, in order, what needs the owner's eye, who is new, who moved (from affiliation atoms), which organizations are trending, what happened, who to promote, who to call, and which files might be the same person: every name a clickable `[[target|display]]` wikilink that still resolves after a rename.
- Promoting a candidate produces a `contact.person` card in the daily summary panel, not an atom; nothing reaches the store without the owner's accept. Merging a possible duplicate writes one owner-signed `entity.same_as` atom, moves no data, and is undone by one click.
- An MCP agent can read the latest briefing with `ffs_audit_query {kind: "briefing"}` and the daily summary is unchanged for callers that omit `kind`.
- The auditor remains stdlib-only and completes a briefing over a 500-atom window inside its timeout.

## Result (2026-09-20)

- Python: `skills/auditor/briefing.py` (pure derivations over dicts; `collect()` is the only RPC layer), 27 tests in `test_briefing.py`; `.venv/bin/python -m pytest skills/` → 258 passed. 500 atoms per predicate compute in 11 ms.
- Rust: flat layout in `ffs-core` (`projection_task41.rs`), `claim_resolved` deep links, `auditor.briefing` spec + template, `atom.list` windowed form, `audit.query kind`, `audit.publish_summary predicate` (validates briefings against the spec; fresh entity each), `audit.run`, `DispatcherProxy`, scheduler, `ffs health --briefing`, `ffs_audit_query kind`. Tests: `briefing_task41.rs` (7), `briefing_e2e_task41.rs` (real auditor and scribe under a `SkillsHost` over the proxy: one briefing atom, new person, `joined` and `left` from affiliation atoms, trending org, one promotion candidate, one follow-up, the page with `[[basename|display]]` links, promotion lands in quarantine and not the store), `briefing_binary_task41.rs` (the binary with `FFS_AUDITOR_BRIEFING_INTERVAL=2s` publishes and files `briefings/<date>.md`; `audit.run` works), MCP `audit_kind_task41.rs`.
- Plugin: `obsidian-plugin/src/briefing.ts` (`BriefingPanelModel`), section in `main.ts`, `settings.briefing.dismissed`; 12 tests in `briefing.test.ts`; `npm test` → 86 passed; `npm run build` ok.
- Docs: first-use guide "The morning briefing" section and `briefings/` rows; ARCHITECTURE starter-set sentence; ADR-037; SKILL.md.
- Not done, by scope: the plugin's `paths.ts` flat-layout awareness (see Implementation Details); Undo merge in the daily summary's auto-filed section (offered in the briefing section instead).

