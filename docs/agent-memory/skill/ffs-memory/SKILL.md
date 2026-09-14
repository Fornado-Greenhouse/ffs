---
name: ffs-memory
description: Use FFS (the Foley File System substrate) as persistent memory. Use whenever the user asks you to remember, recall, look up, or record a person, contact, decision, note, or fact; whenever you would otherwise store durable knowledge in a scratch file; and before proposing content to the substrate. Teaches search-before-write, progressive disclosure, honest provenance, and the proposal-not-saved rule from the FFS Agent Memory Convention.
---

# FFS Memory Skill

This skill teaches an agent how to use an FFS substrate as memory through the `ffs-mcp` tools, with the `ffs` CLI as a fallback. The full behavioral rules are in the [FFS Agent Memory Convention](../../CONVENTION.md). This file is the operational summary.

---

## 1. Minimal Agent Contract

1. **Persistent knowledge belongs in the substrate.** Conversations reset. Propose durable facts through `ffs_author_atom`; do not keep them in scratch files.
2. **Search before proposing.** Call `ffs_search` with the entity's name or the note's title first. Extend what exists rather than creating a near-duplicate.
3. **Read listings and projections before atoms.** `ffs_list_path` and `ffs_search` give lightweight hits; `ffs_render_projection` gives one rendered entity; `ffs_query` gives the atoms behind it. Never dump the substrate or walk `~/.ffs/` on disk.
4. **Do not propose noise.** No conversation, no reasoning, no transient state, no copies of existing content.
5. **Set `source_uri` to the real origin** when there is one. Otherwise the server stamps your identity, which is correct.
6. **Mark inferences as inferences** in the content you propose: "Based on A and B, the agent infers C."
7. **Never overwrite.** Atoms are immutable; change is supersession. State conflicts in the content and let the owner resolve them.
8. **Never re-propose what the owner rejected** without new evidence, and say what the evidence is.
9. **A capability denial is the answer.** Do not retry, do not route around it, do not guess at content you cannot see.
10. **A `submission_id` means proposed, not saved.** Say "proposed, awaiting review in the daily summary." Never say "saved", "stored", or "recorded".
11. **Review knowledge after substantial work** (Section 5) and propose only what passes the review.

---

## 2. Tooling Reference

Prefer the MCP tools. They run in-process, return structured JSON, and pass the capability evaluator at the daemon. Use the CLI only when the MCP server is not available.

| Task | Preferred: MCP tool | Fallback: `ffs` CLI |
|---|---|---|
| Find an entity by name or title | `ffs_search(query="Sara Chen", limit=10)` | none in MVP |
| Browse a listing | `ffs_list_path(path="contacts/by-name/S/")` | `ffs ls ffs://_root_/contacts/by-name/S/` |
| Read one rendered entity | `ffs_render_projection(path="contacts/by-name/S/Sara_Chen.md")` | `ffs cat ffs://_root_/contacts/by-name/S/Sara_Chen.md` |
| Read the atoms behind an entity | `ffs_query(entity="Sara_Chen")` | `ffs get ffs://_root_/by-entity/Sara_Chen` |
| Resolve any `ffs://` URL | `ffs_resolve_url(url="ffs://_root_/...")` | `ffs cat <url>` or `ffs get <url>` |
| Learn a predicate's shape | `ffs_inspect_predicate(name="contact.person")` | `ffs predicate inspect contact.person` |
| Propose content | `ffs_author_atom(content="...", source_uri="...")` | write a `.md` file into `~/.ffs/ingest/` |
| See what the auditor flagged | `ffs_audit_query(since="2026-09-01T00:00:00Z")` | none in MVP |
| Check the daemon | none | `ffs health` |

Notes:

- `ffs_search` matches the canonical name field only (`display_name` for people, `title` for notes). It is case-insensitive substring match. Default limit is 10, maximum 50.
- `ffs_list_path` returns the listing's Markdown. Paths are projection paths relative to the substrate root: `contacts/by-name/<letter>/`, `contacts/recent/`, `people/by-name/<letter>/`, `notes/recent/`, `notes/by-name/<letter>/`.
- Every tool result is capability-filtered. A denial comes back as `isError: true` with `details.kind = "capability_denied"` and a reason.

---

## 3. Workflow

### 3.1 Discover

Start narrow.

```json
// ffs_search
{ "query": "sara chen", "limit": 5 }
```

Read the hits (`entity`, `predicate`, `display_name`). If none match, and the question is "what do I have in this area", browse a listing:

```json
// ffs_list_path
{ "path": "contacts/by-name/S/" }
```

Do not enumerate every letter to answer a question one search can answer.

### 3.2 Inspect

Render only the entity you need:

```json
// ffs_render_projection
{ "path": "contacts/by-name/S/Sara_Chen.md" }
```

Query atoms only when you need provenance, history, or an `as_of` view:

```json
// ffs_query
{ "entity": "Sara_Chen", "predicate": "contact.person", "as_of": "2026-04-15T00:00:00Z" }
```

### 3.3 Propose

Learn the predicate's shape once per session if you are unsure:

```json
// ffs_inspect_predicate
{ "name": "contact.person" }
```

Then propose Markdown whose frontmatter matches the predicate's `frontmatter_fields` and whose sections match its `body_sections`. For a contact:

```json
// ffs_author_atom
{
  "source_uri": "file:///Users/me/inbox/2026-09-12-intro-email.md",
  "content": "---\ndisplay_name: Sara Chen\nwork_email: sara@acme.com\norganization: Acme\ntier: introducible\n---\n\n## Notes\n- Introduced by Wes on 2026-09-12; interested in records-shaped tools.\n\n## History\n- 2026-09-12: intro email. Based on the email signature, the agent infers she leads the platform team. Unconfirmed.\n"
}
```

For a plain note when no predicate fits:

```json
// ffs_author_atom
{
  "content": "---\ntitle: Decision: keep the Acme pilot to one team\nstatus: draft\n---\n\n## Body\nOwner decided on 2026-09-12 to limit the pilot to the platform team until Q1.\n\n## References\n- ffs://_root_/contacts/by-name/S/Sara_Chen.md\n"
}
```

If the entity already exists, propose only the addition (a new `History` or `Notes` line under the same `display_name`), not a full re-statement. The scribe and the owner decide whether it lands as a supersession.

### 3.4 Report honestly

The result is a `submission_id`. Report it as a proposal:

> Proposed a contact for Sara Chen (submission `sub-0142`). It will show up in your daily summary for review.

If the call failed, say so and hand the owner the content you meant to submit. Do not claim persistence that did not happen, and do not re-submit the same content under a different `source_uri`.

---

## 4. Discovery Checklist

Before acting on the owner's knowledge:

- [ ] Did I search for the people, organizations, or notes this task touches?
- [ ] Did I read the rendered projection for the ones that matter, and only those?
- [ ] Did I check whether an existing claim already answers the question before proposing anything?
- [ ] If I hit a capability denial, did I stop there and say what was denied?

---

## 5. End-of-Task Knowledge Review

After substantial work, ask:

1. Did I learn a fact about a person, organization, or thing the owner tracks? Propose it under the right predicate.
2. Did the owner make or state a decision? Propose a note with the rationale they gave.
3. Did I discover something non-obvious a future agent would otherwise rediscover? Propose it.
4. Did an existing claim turn out to be wrong or outdated? Propose the correction with the conflict stated; do not pretend the old claim never existed.
5. Did a relationship between entities change? Add a `History` line or a note `References` entry.
6. Did I produce an artifact the owner will want to find later? Propose a note pointing at it.
7. Did I leave anything in scratch files that belongs in the substrate? Move it, or delete it.

If every answer is no, propose nothing. The review is a filter, not a quota.

---

## 6. What This Skill Is Not

This is an instructional skill for MCP-aware agents (Claude Code, claw hosts, framework-agnostic agents). It is not a daemon-hosted FFS skill bundle: it has no `entry_point`, no `definition.atom.json`, and it must not be installed under `~/.ffs/skills/`, where the skills host would reject it. Install it where your agent loads skills from; see [`../../README.md`](../../README.md).
