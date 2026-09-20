# Starter Tera template library

The `.md.tera` files in this directory define how the substrate's
starter predicate atoms (`contact.person`, `person.generic`, `note`,
`org.company`, `source.article`, `event.business`, plus the
folder-less `affiliation`, `entity.same_as`, `entity.different_from`)
render into projection markdown — the format any editor opens. Each template
is referenced by its matching predicate spec's `rendering.template`
field (`starter/predicates/<name>.toml`); the installer (task_22)
copies both into `~/.ffs/config/` on first daemon startup.

Output shape per template aligns with the predicate spec's
`rendering.frontmatter_fields`, `rendering.body_sections`, and
`rendering.additive_sections` — that alignment is what lets the
fast-path edit classifier (ADR-014) translate a user's edit back
into an atom mutation cleanly.

## Design constraints

Three constraints apply to every template here:

- **Deterministic output.** Same atom → byte-identical markdown.
  Render hashes (`BLAKE3` of the rendered bytes) stay stable across
  reruns; the librarian's drift detector (task_12) uses that
  stability to decide whether a projection needs refresh.
- **Empty optional fields don't bleed into output.** A `## Notes`
  header with no bullets confuses both humans and the classifier;
  templates suppress sections whose backing array is empty or
  missing. Frontmatter lines for missing scalar fields are omitted
  too — no `phone: ` line with an empty value.
- **Field order is fixed.** Same order as the predicate spec's
  `frontmatter_fields` declaration so a user editing one field
  always produces the same diff shape.

## Tera syntax notes

- `{%- ... -%}` strips whitespace around control statements so
  conditionally-emitted lines don't leave stray blank lines.
- `{% if claim.foo %}` treats undefined / missing fields as falsy
  cleanly (Tera does not error on undefined accesses inside `if`).
- `{% if claim.notes and claim.notes | length > 0 %}` is required
  for arrays — Tera treats `[]` as truthy, so we explicitly check
  length to suppress empty sections.

## Per-template

### `contact-person.md.tera`

8 frontmatter fields (display_name, work_email, personal_email,
phone, organization, role, tier, pronouns — emitted only when
present) + four additive sections (`## Notes`, `## Tags`,
`## Organizations`, `## History`).

### `person-generic.md.tera`

5 frontmatter fields (display_name, role, team, location, pronouns)
+ one additive section (`## Bio`).

### `note.md.tera`

3 frontmatter fields (title, author, status) + a free-form `## Body`
section (rendered verbatim from `claim.body`) + two additive
sections (`## Tags`, `## References`).

### `org-company.md.tera`

5 frontmatter fields (display_name, aliases, industry, location,
website) + `## Description` (verbatim from `claim.description`) +
three additive sections (`## Notes`, `## Tags`, `## History`) + the
`## People` reverse-lookup section.

### `source-article.md.tera`

6 frontmatter fields (title, url, publication, byline, published_at,
content_hash) + `## Summary` (verbatim) + `## Mentions` (linked
is-about list from `mentions_resolved`) + `## Tags` (additive).

### `event-business.md.tera`

5 frontmatter fields (title, kind, date, amount, source) + `## Summary`
+ `## Participants` (linked list from `participants_resolved`, role in
parentheses) + `## Tags` (additive).

### `affiliation.md.tera`, `entity-same-as.md.tera`, `entity-different-from.md.tera`

Minimal standalone renders for `atom.get` inspection. Affiliations
normally appear inside person and org files (below); the identity
predicates never render as files of their own.

## Links, aliases, reverse lookups, and stubs (ADR-028, ADR-030, ADR-031)

**Link form.** A reference to another entity renders as an Obsidian
wikilink with the file basename as the target and the printed name as
the alias: `[[Sara_Chen_(Acme)|Sara Chen]]`. The renderer resolves
each referenced entity id through the path-to-entity index and hands
templates pre-resolved objects: `organization_link` (`{basename,
display}` or null), `mentions_resolved[]` and `participants_resolved[]`
(each item carries `basename`, null when unresolved), and
`affiliations[]` (`{entity, display, basename, title, kind,
valid_from, valid_to, side}`). A template never derives a basename
itself. An unresolved item prints its `display` as plain text.

**Aliases line.** Templates emit `aliases: [a, b]` in frontmatter only
when `claim.aliases` is a non-empty array, so fixtures without aliases
render byte-identically to before.

**Reverse-lookup sections.** `## Affiliations` on person and contact
files and `## People` on org files are rendered from `affiliation`
atoms, one line each: `- [[basename|display]]: title (valid_from to
valid_to-or-present)`. They are emitted only when non-empty and carry
no reverse-map rule; an edit inside them routes to ingest (ADR-014).

**Redirect stub.** When an entity is renamed, the old basename is left
as a file whose only line is `Moved to [[new_basename|Display Name]]`.
The fast-path watcher ignores it; it is never an atom.

**Merged stub.** When an `entity.same_as` head exists for an entity,
its projection is a file whose only line is `Merged into
[[target_basename|Display Name]]`; the materializer writes no full
file for it.

## Tera notes for the new context

The renderer may not provide `organization_link`, `affiliations`,
`mentions_resolved`, or `participants_resolved` for every render
(older renderers, or predicates that do not need them). Templates
therefore guard with nested `{% if x %}{% if x | length > 0 %}` so an
undefined variable is falsy and never errors.
