---
status: completed
title: Filing cabinet — registry-declared path families + business-graph predicates and wikilinked templates (ADR-028, amended by ADR-030/031)
type: backend
complexity: high
dependencies:
  - task_20
  - task_21
  - task_25
  - task_36
  - task_37
---

# Task 38: Filing cabinet — registry-declared path families + business-graph predicates and wikilinked templates (ADR-028, amended by ADR-030/031)

## Overview
The morning-briefing goal (read the local business press end to end, keep files on the movers and shakers current) needs four drawers: people, organizations, articles, events. The substrate has three, and they are hardcoded: `PathFamily` in `crates/ffs-core/src/projection/path.rs` is a closed enum, `family_for_predicate` maps exactly three predicate names, the materializer silently drops any atom outside them, `dispatch.rs::entity_search` special-cases `"note" => "title"`, and the Obsidian plugin carries its own `PROJECTION_FAMILIES` constant. Registering a predicate today gives you a schema and a template but no folder. ADR-028 decides that the predicate spec declares its own family via a `[path]` table, that the registry becomes the source of truth every consumer reads, that the business-graph predicate set ships in `starter/`, and that templates emit Obsidian wikilinks so the vault's graph view is the business graph. This task implements all of it with the three-family byte-identity check as the gate before any new spec lands.

Amended 2026-09-14 (ADR-030, ADR-031). Two research tracks on "how does the same person across fifty articles stay one file" changed the shapes before implementation started. Entity ids become opaque and permanent (ADR-030); the human-readable name is data on the head atom, the file name is a projection concern, collisions get a parenthetical qualifier on the file name, and renames or merges leave a redirect stub. Roles become their own `affiliation` atoms rendered into person and org files by reverse lookup; event participants and article mentions become entity references, not strings; predicate specs may carry an informative `[ontology]` table (ADR-031). Two identity predicates, `entity.same_as` and `entity.different_from`, join the starter set, and a starter `config/resolution.toml` carries the weights and thresholds task_36's resolver consumes. The family-token collision suffix from the original ADR-028 text is withdrawn.

Ordered after task_36 on purpose (2026-09-14): the tracer bullet's real extraction output informs the spec shapes and the resolver config; the resolver itself is task_45.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST add an optional `[path]` table (`family`, `name_field`) and an optional `[ontology]` table (`bfo`, `cco`, `iao`, `note`, all strings) to the predicate-spec loader (`RawSpec` and `PredicateSpec` in `crates/ffs-core/src/predicate/mod.rs`; the loader is `deny_unknown_fields`, so both tables have to be declared there). A spec whose `path.name_field` disagrees with `pagination.group_field` MUST fail to load with a clear error. Specs without `[path]` MUST still load and have no projection family. `[ontology]` MUST be informative only: unknown or empty values MUST NOT fail the load, and nothing in the daemon MUST branch on it.
- MUST make path families registry-backed: `PathFamily::try_parse`, `primary_predicate`, `as_str`, `family_for_predicate`, and `path_for_entity` in `path.rs` resolve through the registry (or a family table snapshot derived from it) instead of a closed enum. The renderer (`projection/render.rs`), materializer (`ffs-daemon/src/materializer.rs`), fast-path watcher (`ffs-fastpath/src/watcher.rs`), and `entity_search` (`ffs-daemon/src/dispatch.rs`, which MUST derive the name field from the spec's `path.name_field` and drop the hardcoded `"note"` match) MUST all consume the registry-backed family.
- MUST keep behavior byte-identical for `contacts/`, `people/`, `notes/`: the three starter specs gain `[path]` tables declaring their existing families, every existing test in `path.rs`, `render.rs`, `materializer.rs`, and the fastpath crate passes with assertions unmodified, and materialized output for the existing fixtures is unchanged except for the new `aliases:` frontmatter line, which MUST be suppressed when the claim has no aliases so existing fixtures stay byte-identical.
- MUST add a `path.families` JSON-RPC method to the daemon returning `[{family, predicate, name_field}]` for every spec that declares `[path]`, and MUST make the Obsidian plugin enumerate families from it (`paths.ts` `PROJECTION_FAMILIES` and `main.ts` `familyForPredicate` replaced by a runtime table; `folder.ts` enumeration and `isProjectionPath` consult it). The plugin MUST tolerate the daemon being unreachable at load (empty or last-known table, no crash).
- MUST mint opaque entity ids per ADR-030: a helper in `ffs-core` (`EntityId::mint()`: 16 random bytes, base58btc multibase, `z` prefix) replaces `slug_for_proposal` as the id source in `dispatch.rs::ingest_accept` for every new entity. Existing slug-form ids MUST remain valid (they are just strings; nothing may assume the new form). The `from-<submission-id>` fallback MUST be deleted.
- MUST move the human-readable name to the projection layer: the materializer and working set MUST keep a persisted path-to-entity index (`working_set.rs` plus the SQLite working-set table) so that `path_for_entity` derives the basename from the head atom's name field, a rename supersedes the name and moves the file, and the old basename is left as a one-line redirect stub (`Moved to [[<new basename>|<display>]]`) that the fast-path watcher ignores. `parse` of a projection path MUST resolve the basename through the index to the entity id.
- MUST apply ADR-030's collision rule on the file name only: when two entities in any family would share a basename, the second and later get a parenthetical qualifier taken from `organization`, then `role`, then the year of `valid_from` (`Sara_Chen_(Acme).md`, `Sara_Chen_(City_Council).md`). The rule lives in one place (the path-to-entity index) and MUST be deterministic for a given set of head atoms. The family-token suffix rule from the original ADR-028 text MUST NOT be implemented.
- MUST ship the starter spec changes, each with `claim_schema`, `[rendering]`, `[[reverse_map]]` rules, `[pagination]` (where the family has listings), `[path]` (where the predicate has a folder), and `[ontology]`:
  - new `org.company.toml` (family `orgs`; `display_name` required, `aliases[]`, `industry`, `location`, `website`, `description`, `notes[]`, `tags[]`, `history[]`);
  - new `source.article.toml` (family `articles`, pagination `recency`; `title` and `url` required, `publication`, `byline`, `published_at`, `summary`, `content_hash`, `mentions[]` of `{entity, display, context}` objects with `display` required, `tags[]`);
  - new `event.business.toml` (family `events`, pagination `recency`; `kind` enum per ADR-028, `date`, `participants[]` of `{entity, display, role}` objects with `display` required and `role` from ADR-031's enum, `amount`, `summary`, `source`, `tags[]`);
  - new `affiliation.toml` (no `[path]`; `person` and `organization` entity ids required, `title`, `kind` enum per ADR-031, `source`, `notes[]`);
  - new `entity.same_as.toml` (no `[path]`; `target` required, `reason`, `criterion`) and `entity.different_from.toml` (no `[path]`; `other` required, `criterion`);
  - `person.generic.toml` bumped to `version = 2` adding `organization` (current primary affiliation, rendering convenience only), `aliases[]`, `mentions[]` with reverse-map rules for each.
- MUST ship the matching Tera templates in `starter/templates/`: new `org-company.md.tera`, `source-article.md.tera`, `event-business.md.tera`, and an updated `person-generic.md.tera`. Every template MUST emit `aliases:` in frontmatter when the claim has aliases. Links MUST take the form `[[<target basename>|<display>]]` where the target is resolved through the path-to-entity index from the referenced entity id: a person's `organization`, an article's `mentions[].entity`, an event's `participants[].entity` and `source`. A mention or participant without an `entity` renders its `display` as plain text. Templates MUST keep the determinism and empty-section-suppression constraints from `starter/templates/README.md`.
- MUST render affiliations by reverse lookup (ADR-031): `render_single_entity` MUST, for a family whose template declares it needs them, call `list_by_predicate(affiliation)`, keep the head of each chain at `as_of` whose `claim.person` (people, contacts) or `claim.organization` (orgs) equals the entity, pass them to Tera as `affiliations`, and add their hashes to `source_atoms`. `person-generic.md.tera` and `contact-person.md.tera` render an `## Affiliations` section, `org-company.md.tera` a `## People` section, each line `[[<target>|<display>]]: <title> (<valid_from> to <valid_to or present>)`. Fast-path edits inside those sections MUST route to ingest as corrections per ADR-014, not reverse-map.
- MUST render a merged entity per ADR-030: when an `entity.same_as` head exists for an entity, its projection is a stub `Merged into [[<target basename>|<display>]]` and the materializer MUST NOT write a full file for it; the target's projection is unchanged by this task (folding the loser's atoms into the winner's render is task_39's UI work).
- MUST ship `starter/config/resolution.toml` (installed to `$FFS_DATA_DIR/config/resolution.toml`): Fellegi-Sunter style agreement weights per compared field (`display_name`, `alias`, `organization`, `role`, `location`, `surname`) and two thresholds (`auto_link`, `review_floor`), with conservative starting values and comments. This task only ships and validates the file (a loader test in `ffs-core`); task_45 consumes it.
- MUST enforce that the fast-path classifier (`crates/ffs-fastpath/src/classifier.rs`) recognizes the new additive sections (`Notes`, `Tags`, `History` on orgs; `Mentions` on people and articles) via the specs' reverse-map rules with no classifier code changes, a fixture-driven test per new spec, and that edits in `## Affiliations`, `## People`, and `## Participants` classify as route-to-ingest.
- MUST write `docs/ontology-alignment.md`: the `[ontology]` vocabulary, one row per starter predicate (`contact.person` and `person.generic` object; `org.company` object aggregate; `affiliation` role; `event.business` process; `source.article` and `note` information content entity; `capability.grant`, `entity.same_as`, `entity.different_from` information content entity), what each buys, and what is deliberately not adopted, per ADR-031.
- MUST update docs: `starter/predicates/README.md` (the `[path]` and `[ontology]` tables, the new and updated specs with their reverse-map tables, the object shapes for mentions and participants, the version-bump rule for additive vs breaking changes), `starter/templates/README.md` (the `[[target|display]]` link form, `aliases:` frontmatter, the reverse-lookup sections, the redirect stub), and `ARCHITECTURE.md` ("Three folder spaces" prose → families are registry-declared; the stability-commitments line for the TOML predicate-spec format names the `[path]` table; the atom-envelope section notes that entity ids are opaque and names are claim data).
- MUST NOT change the atom envelope, the capability evaluator, or the skills-host wire protocol. MUST NOT require a migration for existing `person.generic` atoms (the v2 change is additive; a loader test proves a v1 claim validates under v2) or for existing slug-form entity ids.
- SHOULD add the four new families to `ffs_list_path`'s tool description examples in `crates/ffs-mcp/src/tools.rs` (description text only; signature unchanged).
</requirements>

## Result (2026-09-20)

Implemented in four concurrent slices over a foundation laid first: `[path]` and `[ontology]` tables in the spec loader with the `name_field` versus `group_field` check; `SpecRegistry::families()`; `EntityId::mint()` (16 random bytes, base58btc); `ffs_core::resolution::ResolutionConfig` loading `starter/config/resolution.toml`. Then: `PathFamily` became a registry-backed struct with a `FamilyTable` snapshot per operation; a `PathIndex` (in-memory, working set, and the `path_index` SQLite table at schema v4) assigns basenames from the head atom's name field with the parenthetical qualifier order organization, role, year; the renderer resolves links through the index, renders `## Affiliations` and `## People` by reverse lookup, and renders the merged stub for `entity.same_as` heads; the materializer writes redirect stubs on rename and never a full file for merged entities; the fast-path watcher ignores stubs and resolves basenames through the index; `entity_search` reads each family's `name_field` with no predicate name in code and hits carry `basename`; `path.families` RPC; the plugin loads its family table from it, persists the last-known table, and enumerates nothing when empty. Nine starter specs (six new, `person.generic` v2 additive, `contact.person` gains aliases), six new templates plus reverse-lookup sections, `docs/ontology-alignment.md`, starter READMEs, ARCHITECTURE prose, `ffs_list_path` examples, installers copy `resolution.toml`.

One scribe change rode along because the live check needed it and it is generic: a registered `predicate:` hint whose frontmatter supplies the schema's required fields builds the claim from the frontmatter keys the schema declares (no predicate named in code). Registering `source.article` and `org.company` flipped two corpus fixtures from note fallback to real proposals with no other scribe change, which is the task's extensibility criterion demonstrated. Two fixes from the live check: the organization wikilink in frontmatter is quoted (strict YAML otherwise reads `[[..]]` as a nested list) and `person.generic` carries no reverse-map rule for `organization`, so an edit there routes to ingest (55 rules total).

Verification: cargo nextest 482 passed; fmt clean; clippy 0 warnings; pytest 144 passed; vitest 71 passed; byte-identity gate for contacts, people, notes held with assertions unchanged in meaning. Live check on a scratch substrate: an org note accepted became `orgs/by-name/A/Acme_Widgets.md` with entity id `zUnEK...`; a person note naming that org rendered `organization: "[[Acme_Widgets|Acme Widgets]]"` under `people/`; `path.families` listed six families; the search hit carried `basename`. The e2e org-note test runs through the dispatcher with a stub scribe rather than `ingest_pipeline_e2e.rs`, since the heuristic engine cannot emit `org.company` from prose; the frontmatter-hint path covers the hand-written case live.

Follow-ups: `entity.search` and the affiliation reverse lookup scan `list_by_predicate` per family (fine at personal scale; index later); search hits carry `basename` only after materialization; folding a merged entity's atoms into the winner's render is task_39.

## Subtasks
- [x] 38.1 Loader: `[path]` and `[ontology]` tables on `RawSpec`/`PredicateSpec`; `name_field` vs `group_field` consistency check; unit tests for present, absent, conflicting `[path]`, and an unknown `[ontology]` value that loads without error.
- [x] 38.2 Registry family table: accessor returning `(family, predicate, name_field)` triples; hot-reload keeps the table current; test that a fixture TOML with `[path] family = "widgets"` yields a `widgets/` family.
- [x] 38.3 Opaque ids: `EntityId::mint()` in `ffs-core`; `dispatch.rs::ingest_accept` uses it for new entities and the `from-<submission-id>` fallback is removed; tests that minted ids are unique, multibase-valid, and that slug-form ids still round-trip through the store.
- [x] 38.4 Path-to-entity index in `working_set.rs` and its SQLite table: basename derived from the head atom's name field; parenthetical collision qualifier (organization, role, year); rename moves the file and writes a redirect stub; `parse` resolves basenames through the index; tests for the two-Sara-Chen case, a rename, and determinism.
- [x] 38.5 `path.rs` registry-backed: `PathFamily` resolved by lookup; `parse`, `path_for_entity`, `family_for_predicate` take the family table and the index; all existing `path.rs` tests green with assertions unmodified against the three starter specs.
- [x] 38.6 Consumers: `render.rs` (including the affiliation reverse lookup and the merged-entity stub), `materializer.rs` (redirect stubs, no full file for merged entities), `watcher.rs` (ignore redirect stubs), `dispatch.rs::entity_search` (name field from spec, hardcoded `"note"` removed); existing tests green; materializer test writes `orgs/by-name/A/Acme.md` for an `org.company` atom.
- [x] 38.7 `path.families` RPC + plugin: runtime family table replaces `PROJECTION_FAMILIES` and `familyForPredicate`; `openHit` resolves through the same index (basename, not display name); vitest for enumeration from a mocked `path.families` response and for the unreachable-daemon fallback.
- [x] 38.8 Starter specs: `org.company`, `source.article`, `event.business`, `affiliation`, `entity.same_as`, `entity.different_from`, `person.generic` v2, each with `[ontology]`; `starter/config/resolution.toml` and its loader test; spec-loader tests; v1 `person.generic` claim validates under v2.
- [x] 38.9 Templates: four `.md.tera` files plus the two reverse-lookup sections in `contact-person.md.tera`; `[[target|display]]` links, `aliases:` frontmatter, plain-text fallback for unresolved mentions and participants; render tests assert the exact link text, the Affiliations section, empty-section suppression, and render-hash stability.
- [x] 38.10 Fast-path: classifier fixture per new spec (additive sections) and route-to-ingest for the reverse-lookup sections; installer copies `config/resolution.toml`.
- [x] 38.11 Docs: `docs/ontology-alignment.md`, starter READMEs, ARCHITECTURE.md path-library, stability, and entity-id prose, `ffs_list_path` description examples; live check: drop a hand-written org note and a person note naming that org into `ingest/`, accept both, confirm `orgs/by-name/…` exists, the person file's `[[Org|Org Name]]` link resolves in Obsidian, and its Affiliations section is empty until an affiliation atom exists.

## Implementation Details
Current structure: `PathFamily` (closed enum, `path.rs`) → `ParsedPath` → `render.rs` (`primary_predicate()` picks the store query; the Tera context is `entity`, `claim`, `classification`) and `materializer.rs` (`family_for_predicate` → `path_for_entity`). The plugin mirrors the enum in `paths.ts` and `main.ts`. The spec loader (`predicate/mod.rs`) already parses `[pagination] group_field`; the `[path]` and `[ontology]` tables sit beside it. Entity ids today are `slug_for_proposal` in `dispatch.rs` (task_32), which is the Wikipedia-title anti-pattern ADR-030 retires.

Shape sketch for the registry-backed family and the reverse lookup (illustrative only):

```rust
pub struct PathFamily { folder: String, predicate: PredicateName, name_field: String }
// resolved via registry.family_for_folder("orgs") / registry.family_for_predicate(&pred)

// render_single_entity, after the head atom and capability check:
let affiliations = store.list_by_predicate(&PredicateName::new("affiliation"), None, 1000)?
    .into_iter().filter(|a| a.claim["person"] == entity.as_str())  // or ["organization"] for orgs
    /* head-of-chain and as_of filtering */;
ctx.insert("affiliations", &affiliations);
```

Version bump: the loader stores `version` and nothing reads it (no migration hook; `validate_claim` keys on name). Additive changes are safe; the starter README documents that a breaking change needs a new predicate name until a migration mechanism exists.

Redirect stubs: a stub is a projection file whose whole body is one line; the materializer records its hash in the suppression registry like any other write, and the watcher treats a file matching the stub shape as not-an-edit. Stubs are never atoms.

### Relevant Files
- `crates/ffs-core/src/predicate/mod.rs`, `registry.rs` — `[path]`, `[ontology]`, family table, `resolution.toml` loader.
- `crates/ffs-core/src/atom.rs` — `EntityId::mint()`.
- `crates/ffs-core/src/projection/path.rs`, `render.rs` — registry-backed families, reverse lookup, merged-entity stub.
- `crates/ffs-core/src/working_set.rs`, `store/schema.rs`, `store/migrations.rs` — path-to-entity index and collision qualifier.
- `crates/ffs-daemon/src/materializer.rs`, `dispatch.rs` (`entity_search`, `ingest_accept`, new `path.families`), `api.rs` (new result type).
- `crates/ffs-fastpath/src/watcher.rs`, `classifier.rs` — family → predicate via registry; stub detection; route-to-ingest sections.
- `obsidian-plugin/src/paths.ts`, `main.ts`, `folder.ts`, `client.ts`; tests in `obsidian-plugin/tests/paths.test.ts`, `folder.test.ts`.
- `starter/predicates/*.toml`, `starter/templates/*.tera`, `starter/config/resolution.toml`, both READMEs, `installer/install.sh` and `install.ps1` (copy `config/resolution.toml`).
- `docs/ontology-alignment.md` (new).
- `crates/ffs-mcp/src/tools.rs` — description text for `ffs_list_path`.

### Dependent Files
- `ARCHITECTURE.md` — path-library prose, stability commitments, entity-id prose.
- `.compozy/tasks/ffs-mvp/task_36.md` — extraction into the new predicates, affiliation proposals, and the resolver that reads `resolution.toml` depend on these specs existing.
- `.compozy/tasks/ffs-mvp/task_39.md` — the additive/conflict classifier reads `additive_sections` from these specs; merged-entity render folding is its UI work.
- `.compozy/tasks/ffs-mvp/task_40.md` — `content_hash` and the URL-as-bearer dedup rule land on `source.article`.
- `crates/ffs-daemon/tests/ingest_pipeline_e2e.rs` — add an org-note fixture once the specs land.

### Related ADRs
- [ADR-028: Path families are declared in predicate specs; the business-graph predicate set](adrs/adr-028.md) — this task's foundation, as amended.
- ADR-030 — opaque ids, file naming, collision qualifier, redirect stubs, `entity.same_as` / `entity.different_from`, `resolution.toml`.
- [ADR-031: Ontological alignment of the predicate set](adrs/adr-031.md) — `affiliation`, participants and mentions as entity references, `[ontology]`.
- [ADR-011] — the three families being generalized.
- [ADR-014] — reverse-map rules the new specs carry; route-to-ingest for reverse-lookup sections.
- [ADR-021] — the spec format gaining `[path]` and `[ontology]`.
- [ADR-027] — `ffs_list_path` / `ffs_search` disclose the new families.

## Deliverables
- `[path]` and `[ontology]` tables in the spec loader + registry family table **(REQUIRED)**.
- Registry-backed `PathFamily` with byte-identical behavior for the three MVP families **(REQUIRED)**.
- Opaque entity ids (`EntityId::mint()`) with slug ids still accepted **(REQUIRED)**.
- Persisted path-to-entity index with parenthetical collision qualifier and redirect stubs **(REQUIRED)**.
- `path.families` RPC and plugin enumeration from it.
- Seven starter spec changes, four new templates plus reverse-lookup sections, `starter/config/resolution.toml` **(REQUIRED)**.
- Affiliation reverse lookup in the renderer and merged-entity stub.
- `docs/ontology-alignment.md`, updated starter READMEs, and ARCHITECTURE.md.
- Unit, integration, and vitest coverage listed below **(REQUIRED)**.

## Tests
- Unit tests:
  - [x] Loader accepts `[path]`, rejects `name_field` ≠ `group_field`, loads specs without `[path]` as family-less, and loads a spec with an unknown `[ontology] bfo` value without error.
  - [x] Fixture TOML with `[path] family = "widgets"` produces a `widgets/` family from the registry.
  - [x] Every pre-existing `path.rs` test passes unmodified against the three starter specs.
  - [x] `EntityId::mint()` yields unique, multibase-valid ids; a slug-form id round-trips through `list_by_entity` and `head_of_chain`.
  - [x] Two people named Sara Chen at different orgs materialize as `Sara_Chen_(Acme).md` and `Sara_Chen_(City_Council).md`; the qualifier order (organization, role, year) is exercised.
  - [x] Renaming an entity moves the file and leaves a redirect stub whose only line is `Moved to [[<new>|<display>]]`; the stub is ignored by the watcher.
  - [x] `entity_search` picks `title` for `note` and `display_name` for the others from the spec, with no predicate-name match in code.
  - [x] `person.generic` v1 claim validates under the v2 spec.
  - [x] A person file renders its `## Affiliations` section from `affiliation` atoms (two current, one ended) with the exact `[[target|display]]: title (from to to)` lines; an org file renders `## People` the same way; the affiliation hashes appear in `source_atoms`.
  - [x] An entity with an `entity.same_as` head renders as the merged stub.
  - [x] Each new template renders the expected link text, plain `display` for an unresolved mention or participant, `aliases:` only when present, suppresses empty sections, and is render-hash stable.
  - [x] `starter/config/resolution.toml` loads and its thresholds satisfy `review_floor < auto_link`.
- Integration tests:
  - [x] Materializer writes `orgs/by-name/A/Acme.md` for an `org.company` atom and the recency listing for an `event.business` atom; writes no full file for a merged entity.
  - [x] Fast-path classifier fixture per new spec: an added bullet under `## Mentions` / `## Notes` classifies as `additive_section` with no classifier code change; an edit under `## Affiliations` or `## People` routes to ingest.
  - [x] `ingest_pipeline_e2e`: an org note dropped in `ingest/` and accepted lands under `orgs/` with an opaque entity id.
  - [x] Plugin vitest: family enumeration from a mocked `path.families`; `isProjectionPath("orgs/by-name/A/")` true after load; `openHit` opens `Sara_Chen_(Acme).md` for the qualified basename; unreachable daemon yields no crash.
- Test coverage target: >=80% on new code
- All tests must pass

## Success Criteria
- All tests passing; `cargo fmt`, `cargo clippy -D warnings`, and `npm test` in `obsidian-plugin/` clean.
- Materialized output for the existing contact/person/note fixtures is byte-identical before and after the change.
- Dropping a hand-written org note into `ingest/` and accepting it yields `orgs/by-name/<L>/<Org>.md` with an opaque entity id, and a person file whose `organization` references that org shows a resolving `[[Org|Org Name]]` link in Obsidian's graph view.
- Two entities with the same display name coexist as two files with recognizable parenthetical qualifiers, and a rename leaves a working redirect.
- Adding a fifth family requires only a new TOML spec and a template — no Rust or TypeScript changes (demonstrated by the `widgets` fixture test).
- Every starter spec carries an `[ontology]` row that matches `docs/ontology-alignment.md`.
