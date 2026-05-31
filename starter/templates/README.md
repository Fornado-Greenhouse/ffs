# Starter Tera template library

The three `.md.tera` files in this directory define how the substrate's
MVP predicate atoms (`contact.person`, `person.generic`, `note`) render
into projection markdown — the format any editor opens. Each template
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

10 frontmatter fields (display_name, work_email, personal_email,
phone, organization, role, tier, pronouns — emitted only when
present) + two additive sections (`## Notes`, `## Tags`).

### `person-generic.md.tera`

5 frontmatter fields (display_name, role, team, location, pronouns)
+ one additive section (`## Bio`).

### `note.md.tera`

3 frontmatter fields (title, author, status) + a free-form `## Body`
section (rendered verbatim from `claim.body`) + two additive
sections (`## Tags`, `## References`).
