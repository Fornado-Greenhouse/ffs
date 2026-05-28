# Starter predicate-spec library

The three TOML files in this directory define the substrate's
out-of-the-box vocabulary per ADR-011 (path library starts at
three: contacts, people, notes). They are bundled with the
installer (task_22) and copied to `~/.ffs/config/predicates/` on
first daemon startup.

Each spec follows the format documented in [ADR-021](../../.compozy/tasks/ffs-mvp/adrs/adr-021.md):

- `name` + `version` — predicate identity and bump-on-breaking-change version.
- `claim_schema` — JSON Schema (Draft 2020-12) for atom claim payloads.
- `rendering` — Tera template + frontmatter / body / additive section conventions.
- `reverse_map` — rules driving the fast-path edit classifier (see ADR-014).
- `pagination` — listing strategy for path families (`alphabetical_first_letter`, `recency`, `by_org`).

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

**10 reverse-map rules** covering all three edit categories:

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

**6 reverse-map rules** covering all three edit categories.

Pagination: `alphabetical_first_letter` grouped on `display_name`.

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

## Totals

| Predicate | Reverse-map rules |
|---|---|
| `contact.person` | 10 |
| `person.generic` | 6 |
| `note` | 5 |
| **Total** | **21** |

Within the 15-25 range ADR-014 estimates for an MVP starter
library.

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
