---
status: pending
title: Filing cabinet — registry-declared path families + business-graph predicates and wikilinked templates (ADR-028)
type: backend
complexity: high
dependencies:
  - task_20
  - task_21
  - task_25
  - task_37
---

# Task 38: Filing cabinet — registry-declared path families + business-graph predicates and wikilinked templates (ADR-028)

## Overview
The morning-briefing goal (read the local business press end to end, keep files on the movers and shakers current) needs four drawers: people, organizations, articles, events. The substrate has three, and they are hardcoded: `PathFamily` in `crates/ffs-core/src/projection/path.rs` is a closed enum, `family_for_predicate` maps exactly three predicate names, the materializer silently drops any atom outside them, `dispatch.rs::entity_search` special-cases `"note" => "title"`, and the Obsidian plugin carries its own `PROJECTION_FAMILIES` constant. Registering a predicate today gives you a schema and a template but no folder. ADR-028 decides that the predicate spec declares its own family via a `[path]` table, that the registry becomes the source of truth every consumer reads, that four business-graph predicate changes ship in `starter/` (`org.company`, `source.article`, `event.business`, and `person.generic` v2), and that templates emit Obsidian wikilinks so the vault's graph view is the business graph. This task implements all three parts with the three-family byte-identity check as the gate before any new spec lands.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST add an optional `[path]` table (`family`, `name_field`) to the predicate-spec loader (`RawSpec` and `PredicateSpec` in `crates/ffs-core/src/predicate/mod.rs`; the loader is `deny_unknown_fields`, so the table has to be declared there). A spec whose `path.name_field` disagrees with `pagination.group_field` MUST fail to load with a clear error. Specs without `[path]` MUST still load and have no projection family.
- MUST make path families registry-backed: `PathFamily::try_parse`, `primary_predicate`, `as_str`, `family_for_predicate`, and `path_for_entity` in `path.rs` resolve through the registry (or a family table snapshot derived from it) instead of a closed enum. The renderer (`projection/render.rs`), materializer (`ffs-daemon/src/materializer.rs`), fast-path watcher (`ffs-fastpath/src/watcher.rs`), and `entity_search` (`ffs-daemon/src/dispatch.rs`, which MUST derive the name field from the spec's `path.name_field` and drop the hardcoded `"note"` match) MUST all consume the registry-backed family.
- MUST keep behavior byte-identical for `contacts/`, `people/`, `notes/`: the three starter specs gain `[path]` tables declaring their existing families, every existing test in `path.rs`, `render.rs`, `materializer.rs`, and the fastpath crate passes with assertions unmodified, and materialized output for the existing fixtures is unchanged.
- MUST add a `path.families` JSON-RPC method to the daemon returning `[{family, predicate, name_field}]` for every spec that declares `[path]`, and MUST make the Obsidian plugin enumerate families from it (`paths.ts` `PROJECTION_FAMILIES` and `main.ts` `familyForPredicate` replaced by a runtime table; `folder.ts` enumeration and `isProjectionPath` consult it). The plugin MUST tolerate the daemon being unreachable at load (empty or last-known table, no crash).
- MUST ship four starter spec changes, each with `claim_schema`, `[rendering]`, `[[reverse_map]]` rules, `[pagination]`, and `[path]`: new `starter/predicates/org.company.toml` (family `orgs`), new `source.article.toml` (family `articles`, pagination `recency`), new `event.business.toml` (family `events`, pagination `recency`, `kind` enum per ADR-028), and `person.generic.toml` bumped to `version = 2` adding `organization`, `aliases[]`, `mentions[]` with reverse-map rules for each.
- MUST ship the matching Tera templates in `starter/templates/`: new `org-company.md.tera`, `source-article.md.tera`, `event-business.md.tera`, and an updated `person-generic.md.tera`. Templates MUST emit Obsidian wikilinks (`[[<slug>]]`) for a person's `organization`, an article's `mentions[]`, an event's `parties[]` and `source`, where the slug is the entity id. Templates MUST keep the existing determinism and empty-section-suppression constraints from `starter/templates/README.md`.
- MUST enforce ADR-028's slug rule: entity slugs are unique across families; when the scribe derives an id that already exists in another family it appends the family token. This lives in the daemon's entity-id derivation (task_32's code in `crates/ffs-daemon/src/scribe.rs`), not in templates.
- MUST verify the fast-path classifier (`crates/ffs-fastpath/src/classifier.rs`) recognizes the new additive sections (`Notes`, `Tags`, `History` on orgs; `Mentions` on people and articles; `Parties` on events) via the specs' reverse-map rules with no classifier code changes — a fixture-driven test per new spec.
- MUST update docs: `starter/predicates/README.md` (the `[path]` table, the four new/updated specs with their reverse-map tables, the version-bump rule for additive vs breaking changes), `starter/templates/README.md` (wikilink convention and the slug rule), and `ARCHITECTURE.md` ("Three folder spaces" prose → families are registry-declared; the stability-commitments line for the TOML predicate-spec format names the `[path]` table).
- MUST NOT change the atom envelope, the capability evaluator, or the skills-host wire protocol. MUST NOT require a migration for existing `person.generic` atoms (the v2 change is additive; a loader test proves a v1 claim validates under v2).
- SHOULD add the four new families to `ffs_list_path`'s tool description examples in `crates/ffs-mcp/src/tools.rs` (description text only; signature unchanged).
</requirements>

## Subtasks
- [ ] 38.1 Loader: `[path]` table on `RawSpec`/`PredicateSpec` with `name_field` vs `group_field` consistency check; unit tests for present, absent, and conflicting tables.
- [ ] 38.2 Registry family table: `names()`-style accessor returning `(family, predicate, name_field)` triples; hot-reload keeps the table current; test that a fixture TOML with `[path] family = "widgets"` yields a `widgets/` family.
- [ ] 38.3 `path.rs` registry-backed: `PathFamily` resolved by lookup; `parse`, `path_for_entity`, `family_for_predicate` take the family table; all existing `path.rs` tests green with assertions unmodified against the three starter specs.
- [ ] 38.4 Consumers: `render.rs`, `materializer.rs`, `watcher.rs`, `dispatch.rs::entity_search` (name field from spec, hardcoded `"note"` removed); existing tests green; materializer test writes `orgs/by-name/A/Acme.md` for an `org.company` atom.
- [ ] 38.5 `path.families` RPC + plugin: runtime family table replaces `PROJECTION_FAMILIES` and `familyForPredicate`; vitest for enumeration from a mocked `path.families` response and for the unreachable-daemon fallback.
- [ ] 38.6 Starter specs: `org.company`, `source.article`, `event.business`, `person.generic` v2, each with `[path]`; spec-loader tests; v1 `person.generic` claim validates under v2.
- [ ] 38.7 Templates: four `.md.tera` files emitting wikilinks; render tests assert the exact `[[slug]]` text and empty-section suppression; render-hash determinism test for each.
- [ ] 38.8 Slug uniqueness across families in scribe entity-id derivation; test with a person and an org sharing a display name.
- [ ] 38.9 Docs: starter READMEs, ARCHITECTURE.md path-library prose and stability line, `ffs_list_path` description examples; live check: drop a hand-written org note into `ingest/`, accept it, confirm `orgs/by-name/…` exists and a person file's `[[org]]` link resolves in Obsidian.

## Implementation Details
Current structure: `PathFamily` (closed enum, `path.rs`) → `ParsedPath` → `render.rs` (`primary_predicate()` picks the store query) and `materializer.rs` (`family_for_predicate` → `path_for_entity`). The plugin mirrors the enum in `paths.ts` and `main.ts`. The spec loader (`predicate/mod.rs`) already parses `[pagination] group_field`; the `[path]` table sits beside it and reuses the name field.

Shape sketch for the registry-backed family (illustrative only):

```rust
pub struct PathFamily { folder: String, predicate: PredicateName, name_field: String }
// resolved via registry.family_for_folder("orgs") / registry.family_for_predicate(&pred)
```

Version bump: the loader stores `version` and nothing reads it (no migration hook; `validate_claim` keys on name). Additive changes are safe; the starter README documents that a breaking change needs a new predicate name until a migration mechanism exists.

### Relevant Files
- `crates/ffs-core/src/predicate/mod.rs`, `registry.rs` — `[path]` table, family table.
- `crates/ffs-core/src/projection/path.rs`, `render.rs` — registry-backed families.
- `crates/ffs-daemon/src/materializer.rs`, `dispatch.rs` (`entity_search`, new `path.families`), `scribe.rs` (slug rule), `api.rs` (new result type).
- `crates/ffs-fastpath/src/watcher.rs` — family → predicate via registry.
- `obsidian-plugin/src/paths.ts`, `main.ts`, `folder.ts`, `client.ts`; tests in `obsidian-plugin/tests/paths.test.ts`, `folder.test.ts`.
- `starter/predicates/*.toml`, `starter/templates/*.tera`, both READMEs.
- `crates/ffs-mcp/src/tools.rs` — description text for `ffs_list_path`.

### Dependent Files
- `ARCHITECTURE.md` — path-library prose and stability commitments.
- `.compozy/tasks/ffs-mvp/task_36.md` — extraction into the new predicates depends on these specs existing.
- `crates/ffs-daemon/tests/ingest_pipeline_e2e.rs` — add an org-note fixture once the specs land.

### Related ADRs
- [ADR-028: Path families are declared in predicate specs; the business-graph predicate set](adrs/adr-028.md) — this task's foundation.
- [ADR-011] — the three families being generalized.
- [ADR-021] — the spec format gaining `[path]`.
- [ADR-014] — reverse-map rules the new specs carry.
- [ADR-027] — `ffs_list_path` / `ffs_search` disclose the new families.

## Deliverables
- `[path]` table in the spec loader + registry family table **(REQUIRED)**.
- Registry-backed `PathFamily` with byte-identical behavior for the three MVP families **(REQUIRED)**.
- `path.families` RPC and plugin enumeration from it.
- Four starter specs and four templates with wikilinks **(REQUIRED)**.
- Cross-family slug uniqueness in entity-id derivation.
- Updated starter READMEs and ARCHITECTURE.md.
- Unit, integration, and vitest coverage listed below **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] Loader accepts `[path]`, rejects `name_field` ≠ `group_field`, and loads specs without `[path]` as family-less.
  - [ ] Fixture TOML with `[path] family = "widgets"` produces a `widgets/` family from the registry.
  - [ ] Every pre-existing `path.rs` test passes unmodified against the three starter specs.
  - [ ] `entity_search` picks `title` for `note` and `display_name` for the others from the spec, with no predicate-name match in code.
  - [ ] `person.generic` v1 claim validates under the v2 spec.
  - [ ] Each new template renders the expected `[[slug]]` text, suppresses empty sections, and is render-hash stable.
  - [ ] Scribe slug derivation appends the family token on a cross-family collision.
- Integration tests:
  - [ ] Materializer writes `orgs/by-name/A/Acme.md` for an `org.company` atom and `events/by-name/…` (or the recency listing) for an `event.business` atom.
  - [ ] Fast-path classifier fixture per new spec: an added bullet under `## Mentions` / `## Parties` classifies as `additive_section` with no classifier code change.
  - [ ] `ingest_pipeline_e2e`: an org note dropped in `ingest/` and accepted lands under `orgs/`.
  - [ ] Plugin vitest: family enumeration from a mocked `path.families`; `isProjectionPath("orgs/by-name/A/")` true after load; unreachable daemon yields no crash.
- Test coverage target: >=80% on new code
- All tests must pass

## Success Criteria
- All tests passing; `cargo fmt`, `cargo clippy -D warnings`, and `npm test` in `obsidian-plugin/` clean.
- Materialized output for the existing contact/person/note fixtures is byte-identical before and after the change.
- Dropping a hand-written org note into `ingest/` and accepting it yields `orgs/by-name/<L>/<Org>.md`, and a person file whose `organization` names that org shows a resolving `[[Org]]` link in Obsidian's graph view.
- Adding a fifth family requires only a new TOML spec and a template — no Rust or TypeScript changes (demonstrated by the `widgets` fixture test).
