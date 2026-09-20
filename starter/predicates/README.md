# Starter predicate-spec library

The TOML files in this directory define the substrate's out-of-the-box
vocabulary. ADR-011 started the path library at three families
(contacts, people, notes); ADR-028 made families registry-declared and
added the business-graph set (orgs, articles, events) plus the
folder-less predicates that support it (`affiliation`,
`entity.same_as`, `entity.different_from`). They are bundled with the
installer (task_22) and copied to `~/.ffs/config/predicates/` on first
daemon startup, together with `starter/config/resolution.toml`.

Each spec follows the format documented in [ADR-021](../../.compozy/tasks/ffs-mvp/adrs/adr-021.md):

- `name` + `version` — predicate identity; see the version rule below.
- `claim_schema` — JSON Schema (Draft 2020-12) for atom claim payloads.
- `rendering` — Tera template + frontmatter / body / additive section conventions.
- `reverse_map` — rules driving the fast-path edit classifier (see ADR-014).
- `pagination` — listing strategy for path families (`alphabetical_first_letter`, `recency`, `by_org`).
- `path` — the projection family the predicate's entities live under (ADR-028).
- `ontology` — informative BFO / CCO / IAO alignment (ADR-031).

## The `[path]` table (ADR-028)

```toml
[path]
family = "orgs"            # folder root under $FFS_DATA_DIR
name_field = "display_name" # claim field that becomes the file basename
```

A spec with `[path]` gets a folder: `<family>/recent/`,
`<family>/by-name/<letter>/`, and one file per entity. A spec without
`[path]` has no folder; its atoms render inside other files (an
`affiliation` inside a person and an org) or are inspected with
`atom.get`. `name_field` must agree with `pagination.group_field` when
both are present; the loader rejects a mismatch. The registry exposes
the family table (`path.families` RPC), and the renderer, materializer,
fast-path watcher, entity search, and the Obsidian plugin all read it,
so adding a family needs only a spec and a template.

File basenames are derived from the head atom's `name_field` value,
never from the entity id (ADR-030): ids are opaque and permanent. Two
entities that would share a basename get a parenthetical qualifier on
the second and later (`Sara_Chen_(Acme).md`), taken from
`organization`, then `role`, then the year of `valid_from`. A rename
moves the file and leaves a redirect stub.

## The `[ontology]` table (ADR-031)

```toml
[ontology]
bfo = "role"
cco = "cco:OccupationRole"
iao = ""
note = "bearer = claim.person, context = claim.organization"
```

All four fields are free strings and informative only. The vocabulary
and one row per starter predicate live in
[`docs/ontology-alignment.md`](../../docs/ontology-alignment.md); keep
the spec's table and the doc's row in step.

## Object-valued lists

`source.article.mentions[]` and `event.business.participants[]` are
arrays of objects, not strings:

```json
{"entity": "z6Mk...", "display": "Sara Chen", "context": "named as incoming CEO"}
{"entity": "z6Mk...", "display": "Acme Corp", "role": "acquirer"}
```

`display` is required and is the name as printed; `entity` is the
resolved id when the resolver (task_45) found one and absent
otherwise. Templates link resolved items as `[[basename|display]]` and
print unresolved ones as plain text. The fast-path classifier's edit
kinds cannot express an object list item, so these sections carry no
reverse-map rule; an edit under `## Mentions` or `## Participants`
routes to ingest as a correction (ADR-014).

## Version rule

Additive changes (a new optional field, a new section) keep the
predicate name and bump `version`; the loader stores `version` and
nothing reads it, so old claims validate unchanged. `person.generic`
v2 is the worked example: a v1 claim with only `display_name` still
validates. A breaking change (a removed or retyped field) needs a new
predicate name until a migration mechanism exists.

## resolution.toml

`starter/config/resolution.toml` carries the entity-resolution
agreement weights and the two thresholds (`auto_link`,
`review_floor`) from ADR-030. It is installed to
`$FFS_DATA_DIR/config/resolution.toml`, loaded and validated by
`ffs_core::resolution`, and consumed by the resolver in task_45.

The reverse-map rules are the load-bearing input to the fast-path
classifier (`crates/ffs-fastpath`). When a user edits a projection
file on disk, the classifier diffs old-vs-new, finds a matching
rule by output shape, and authors a supersession atom in
sub-200ms. Three edit categories are recognized per ADR-014:

- `single_line_text` — free-form text fields (names, emails, roles).
- `frontmatter_value` — constrained-vocab fields (tier, pronouns,
  status).
- `additive_section` — list items appended to a `## Section` body.

## contact.person

The substrate's primary contact-graph predicate. Atoms represent
people you have a relationship with: display name, work/personal
email, phone, organization, role, free-form notes, tags.

Tier classification lives at the atom level (`classification`
field), not the predicate level — a `contact.person` atom can be
classified `existence`, `work_email`, `personal_email`, etc. so
federation capabilities scope sharing per ADR-020.

**12 reverse-map rules** covering all three edit categories:

| Output | Atom field | Edit kind |
|---|---|---|
| `frontmatter.display_name` | `claim.display_name` | `single_line_text` |
| `frontmatter.work_email` | `claim.work_email` | `single_line_text` |
| `frontmatter.personal_email` | `claim.personal_email` | `single_line_text` |
| `frontmatter.phone` | `claim.phone` | `frontmatter_value` |
| `frontmatter.organization` | `claim.organization` | `single_line_text` |
| `frontmatter.role` | `claim.role` | `single_line_text` |
| `frontmatter.tier` | `claim.tier` | `frontmatter_value` |
| `frontmatter.pronouns` | `claim.pronouns` | `frontmatter_value` |
| `section.Notes.list_item` | `claim.notes[]` | `additive_section` |
| `section.Tags.list_item` | `claim.tags[]` | `additive_section` |
| `section.Organizations.list_item` | `claim.organizations[]` | `additive_section` |
| `section.History.list_item` | `claim.history[]` | `additive_section` |

`aliases` is a list-valued frontmatter field (rendered as
`aliases: [a, b]`) with no reverse-map rule, and `## Affiliations` is a
reverse lookup over `affiliation` atoms with no rule either; edits to
either route to ingest.

Note on `organization` vs `organizations`: the singular
`frontmatter.organization` carries the contact's *current primary*
affiliation; the plural `claim.organizations[]` list section
accumulates the full set across time (past employers, volunteer
roles). The `## History` section is a free-form interaction log.

Pagination: `alphabetical_first_letter` grouped on `display_name`
— populates `contacts/by-name/<letter>/`.

## person.generic

The substrate's lighter-weight person reference. Atoms represent
people who appear in narrative content (meeting notes, decisions,
project records) but aren't full contact-graph entries. No
email/phone — just enough structure to disambiguate "Sara from
product" from "Sara from legal" when an LLM agent extracts
entities from a document.

Promotion from `person.generic` to `contact.person` is a separate
user action; the auditor surfaces candidates in the daily-health
summary.

Version 2 (ADR-028, ADR-031) adds `organization` (the current primary
affiliation, a rendering convenience the scribe writes and the
briefing may correct), `aliases[]` (variant names for the resolver;
no rule), and `mentions[]` (a dated log of press mentions,
`"YYYY-MM-DD: what (article title)"`). Roles over time are
`affiliation` atoms rendered into `## Affiliations` by reverse lookup.

**7 reverse-map rules** covering all three edit categories. There is
deliberately no rule for `frontmatter.organization`: it holds an entity
id rendered as a quoted wikilink (Obsidian's property-link form), so an
edit to that line routes to ingest as a correction rather than
reverse-mapping a bracketed link into the claim (ADR-031).

| Output | Atom field | Edit kind |
|---|---|---|
| `frontmatter.display_name` | `claim.display_name` | `single_line_text` |
| `frontmatter.role` | `claim.role` | `single_line_text` |
| `frontmatter.team` | `claim.team` | `single_line_text` |
| `frontmatter.location` | `claim.location` | `frontmatter_value` |
| `frontmatter.pronouns` | `claim.pronouns` | `frontmatter_value` |
| `section.Bio.list_item` | `claim.bio[]` | `additive_section` |
| `section.Mentions.list_item` | `claim.mentions[]` | `additive_section` |

Pagination: `alphabetical_first_letter` grouped on `display_name`.
Family: `people`.

## note

The substrate's catch-all narrative-text predicate. Holds
unstructured-but-tagged markdown: meeting notes, reading notes,
daily reflections, anything that doesn't fit a more specific
predicate. The scribe falls back to `note` whenever a markdown
input has body content but no structural signal for a contact or
person extraction (see `skills/scribe/extraction.py`).

`status` is a constrained vocabulary (`draft`, `published`,
`archived`) so future filters on `notes/recent/` can scope to a
publication state.

**5 reverse-map rules** covering all three edit categories.

Pagination: `recency` — `notes/recent/` is the primary surface.

## org.company

An organization in the movers-and-shakers cabinet. People relate to
it through `affiliation` atoms; the rendered `## People` section is a
reverse lookup and carries no rule. `aliases[]` feeds the resolver.

**8 reverse-map rules:**

| Output | Atom field | Edit kind |
|---|---|---|
| `frontmatter.display_name` | `claim.display_name` | `single_line_text` |
| `frontmatter.industry` | `claim.industry` | `single_line_text` |
| `frontmatter.location` | `claim.location` | `frontmatter_value` |
| `frontmatter.website` | `claim.website` | `single_line_text` |
| `section.Description` | `claim.description` | `single_line_text` |
| `section.Notes.list_item` | `claim.notes[]` | `additive_section` |
| `section.Tags.list_item` | `claim.tags[]` | `additive_section` |
| `section.History.list_item` | `claim.history[]` | `additive_section` |

Pagination: `alphabetical_first_letter` on `display_name`. Family: `orgs`.

## source.article

A published piece the owner read or was pointed at: metadata plus a
summary, never the body (ADR-035). Normalized `url` is the dedup key;
`content_hash` identifies the fetched bearer. `mentions[]` are
object-valued is-about links (no rule; see Object-valued lists).
`## Summary` is body text with no rule.

**7 reverse-map rules:**

| Output | Atom field | Edit kind |
|---|---|---|
| `frontmatter.title` | `claim.title` | `single_line_text` |
| `frontmatter.url` | `claim.url` | `single_line_text` |
| `frontmatter.publication` | `claim.publication` | `single_line_text` |
| `frontmatter.byline` | `claim.byline` | `single_line_text` |
| `frontmatter.published_at` | `claim.published_at` | `frontmatter_value` |
| `frontmatter.content_hash` | `claim.content_hash` | `frontmatter_value` |
| `section.Tags.list_item` | `claim.tags[]` | `additive_section` |

Pagination: `recency`. Family: `articles`, name field `title`.

## event.business

Something that happened: `kind` is one of `funding`, `acquisition`,
`hire`, `departure`, `expansion`, `opening`, `closing`, `award`,
`partnership`, `other`; `date` is the temporal region; `title` is a
short headline and the name field. `participants[]` are object-valued
(`entity`, `display`, `role` from `acquirer`, `target`, `investor`,
`investee`, `hire`, `employer`, `departing`, `landlord`, `tenant`,
`developer`, `winner`, `partner`, `other`) with no rule.

**6 reverse-map rules:**

| Output | Atom field | Edit kind |
|---|---|---|
| `frontmatter.title` | `claim.title` | `single_line_text` |
| `frontmatter.kind` | `claim.kind` | `frontmatter_value` |
| `frontmatter.date` | `claim.date` | `frontmatter_value` |
| `frontmatter.amount` | `claim.amount` | `single_line_text` |
| `frontmatter.source` | `claim.source` | `single_line_text` |
| `section.Tags.list_item` | `claim.tags[]` | `additive_section` |

Pagination: `recency`. Family: `events`, name field `title`.

## affiliation

A role a person holds at an organization (ADR-031). The atom's entity
is the role particular; `person` and `organization` are entity ids;
the atom's `valid_from` / `valid_to` are the role's lifetime. A new
affiliation is additive under ADR-029 (auto-files); a supersession
(title change, ending) routes to review. No `[path]`: it renders
inside the person's `## Affiliations` and the org's `## People`.

**5 reverse-map rules:**

| Output | Atom field | Edit kind |
|---|---|---|
| `frontmatter.person` | `claim.person` | `frontmatter_value` |
| `frontmatter.organization` | `claim.organization` | `frontmatter_value` |
| `frontmatter.title` | `claim.title` | `single_line_text` |
| `frontmatter.kind` | `claim.kind` | `frontmatter_value` |
| `section.Notes.list_item` | `claim.notes[]` | `additive_section` |

## entity.same_as and entity.different_from

Identity assertions (ADR-030). `entity.same_as`: the atom's entity is
the losing id, `target` the winner; readers follow chains, the loser's
atoms stay in place, undo is supersession, and the loser's projection
becomes a `Merged into [[target|display]]` stub. `entity.different_from`:
symmetric, a hard block for the resolver, written only on the owner's
click. Neither has a folder.

**3 and 2 reverse-map rules** respectively (`target` / `other` and
`criterion` as `frontmatter_value`, `reason` as `single_line_text`).

## Totals

| Predicate | Reverse-map rules |
|---|---|
| `contact.person` | 12 |
| `person.generic` | 7 |
| `note` | 5 |
| `org.company` | 8 |
| `source.article` | 7 |
| `event.business` | 6 |
| `affiliation` | 5 |
| `entity.same_as` | 3 |
| `entity.different_from` | 2 |
| **Total** | **55** |

The original three stay within the 15-25 envelope ADR-014 estimated
for the MVP library (25); the business-graph set adds 31.

## Adding new predicates

Drop a `<name>.toml` into `~/.ffs/config/predicates/`. The
daemon's filesystem watcher picks it up and the registry hot-
reloads (`crates/ffs-core/src/predicate/registry.rs`). Sub-
predicates inherit a parent via the optional `parent_predicate`
field; the loader resolves parent links in topological order so
the parent's schema + rendering compose into the child's.

Reverse-map rules must reference outputs the `rendering`
convention defines — `frontmatter.X` requires `X` in
`frontmatter_fields`; `section.X.list_item` requires `X` in
`additive_sections`. The loader rejects specs that violate this
contract.
