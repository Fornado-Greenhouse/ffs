# FFS Agent Memory Convention v0.1

**Status:** v0.1 Draft, adopted by ADR-027
**Target:** AI agents that read from or write to an FFS substrate through `ffs-mcp` or the `ffs` CLI
**Substrate foundation:** the FFS atom envelope (ADR-017), predicate specs (ADR-021), capability atoms (ADR-007), and the ingest quarantine (ADR-013)

---

## 1. Purpose

This convention defines how an AI agent should use an FFS substrate as the persistent memory of a person or a project.

A conversation is temporary. The substrate is persistent. An agent that treats FFS as memory must follow a small set of behavioral rules so that the substrate stays accurate, compact, honest about provenance, and under the owner's control.

FFS defines the representation: signed, classified, bitemporal atoms. This convention defines agent behavior around that representation. It does not redefine the atom envelope, the predicate-spec format, or the capability evaluator.

---

## 2. Relationship to the OKF Agent Memory Convention

This document is an adaptation of the [OKF Agent Memory Convention v0.1](https://github.com/okf-memory/okf-agent-memory) by the okf-memory project, carried onto a records-shaped substrate. The behavioral core is theirs: search before write, progressive disclosure, preserve uncertainty, keep generated and verified distinct, review knowledge after substantial work, never claim persistence that did not happen. We are grateful for the clarity of that work.

The OKF file format (Markdown with YAML frontmatter in a `knowledge/` bundle) is **not** adopted. FFS atoms already carry richer provenance, signing, classification, and lifecycle metadata than OKF frontmatter, and the substrate is canonical while any Markdown is a projection. Where OKF stores a `verified` list, FFS has a signed owner atom produced by a human accepting a proposal. Where OKF stores `stale_after`, FFS has a bitemporal validity window and supersession chains. Section 5 maps each OKF concept to the FFS primitive that plays its role.

An agent that already follows the OKF convention should find nothing here that contradicts it. Where the two differ, this document is normative for FFS.

---

## 3. Design Principles

### 3.1 Persistent knowledge survives conversations

An agent MUST assume that a future agent, or the owner, may have no access to the current conversation. Information that matters for future work MUST be proposed to the substrate rather than left in the transcript.

### 3.2 The substrate is the long-term state, not a transcript

The substrate represents what the owner currently knows, including relevant history. It is not a log of conversations and not a store for an agent's reasoning.

### 3.3 Prefer existing knowledge over duplication

Before proposing new content, the agent MUST search for the entity it is about. If the entity exists, the agent SHOULD extend it (a new note, a new history line, a corrected field) rather than propose a second entity with a similar name.

### 3.4 Preserve uncertainty

Writing a guess into the substrate does not make it a fact. Inferences MUST be marked as inferences in the content the agent proposes.

### 3.5 The owner is the gate

Nothing an agent submits becomes an atom until the owner accepts it. The daily summary is the fence. An agent MUST NOT design around this fence and MUST NOT describe a proposal as if it had passed it.

### 3.6 Domain neutrality through predicates

The substrate does not fix a taxonomy. Types are predicates, registered by the owner as TOML specs. An agent MUST NOT refuse to work with a predicate merely because it is unfamiliar; `ffs_inspect_predicate` returns its schema.

---

## 4. The Substrate as Memory

An FFS substrate exposes memory to agents through eight MCP tools (ADR-013, ADR-027) and, as a fallback, the `ffs` CLI.

| Purpose | MCP tool | CLI fallback |
|---|---|---|
| Find an entity by name or title | `ffs_search` | none in MVP |
| Browse a projection listing | `ffs_list_path` | `ffs ls ffs://_root_/<path>` |
| Read a rendered projection | `ffs_render_projection` | `ffs cat ffs://_root_/<path>` |
| Read the atoms behind an entity | `ffs_query` | `ffs get ffs://_root_/by-entity/<id>` |
| Resolve any `ffs://` URL | `ffs_resolve_url` | `ffs cat` / `ffs get` |
| Understand a predicate's shape | `ffs_inspect_predicate` | `ffs predicate inspect <name>` |
| Propose new content | `ffs_author_atom` | write a file into `~/.ffs/ingest/` |
| Review what the auditor flagged | `ffs_audit_query` | none in MVP |

Search results are lightweight hits (entity id, predicate, display name). Listings are projection Markdown. Full atom envelopes come only from `ffs_query` and `ffs_resolve_url`.

---

## 5. Mapping OKF Concepts to FFS Primitives

| OKF concept | FFS primitive |
|---|---|
| Concept (one `.md` file) | An entity: the head atoms across its predicates |
| `type` | The predicate name (`contact.person`, `note`, `person.generic`, any registered predicate) |
| `description` (one sentence) | The predicate's canonical name field: `display_name` or `title` |
| `generated: { by, at }` | The atom's `author` public key, plus a `provenance` entry with `kind: mcp_agent` or `ingest_file` and the source URI |
| `verified: [ human ]` | Owner acceptance from the quarantine. A proposal is "generated". An accepted atom is signed by the owner's key and is "verified" |
| `sources` | `provenance` entries (`kind`, `uri`, `hash`) on the atom |
| `status`, `stale_after` | The bitemporal window (`valid_from`, `valid_to`) and the supersession chain; a superseded atom is history, not deleted |
| Relationships (Markdown links) | The `references` field on notes, entity ids in claims, and the `History` and `Notes` sections of contacts |
| `index.md` progressive disclosure | Projection listings read through `ffs_list_path` (`contacts/by-name/S/`, `notes/recent/`) |
| `log.md` dated history | `tx_time` ordering on atoms and `ffs_audit_query` over auditor daily summaries |
| `governance` and `code_refs` | For the substrate: capability atoms, evaluated at every call. For the FFS repository itself: the ADR governance index described in ADR-027 |
| `okf validate` | Predicate claim-schema validation at ingest and at accept; the auditor's daily summary for drift |

The mapping is intentionally lossy in one direction: FFS records who signed each claim and when the substrate received it, which OKF does not require. Agents SHOULD lean on that extra structure instead of re-encoding provenance in prose.

---

## 6. What Should Be Remembered

An agent SHOULD propose content when a future human or agent would reasonably benefit from it:

- **Facts** about people, organizations, and things the owner cares about.
- **Decisions** the owner made, with rationale when it was stated.
- **Discoveries** that were non-obvious and would otherwise be rediscovered.
- **Interactions** with contacts (a meeting, an introduction, a change of role), which belong in a contact's `History`.
- **Corrections** the owner made to earlier knowledge.
- **Artifacts** the owner produced, when their existence matters later.

Each of these maps to a registered predicate. When no predicate fits, the agent SHOULD propose a `note` and say in the note why no better predicate applied.

## 7. What Should Not Be Remembered

An agent SHOULD NOT propose:

- ordinary conversation, greetings, or acknowledgements;
- its own reasoning, plans, or intermediate steps;
- transient state with no future value;
- copies of content already in the substrate;
- speculation presented as fact;
- content about people that the owner has not asked to track, unless the content came from the owner.

The goal is a useful substrate, not maximum retention.

---

## 8. Knowledge Lifecycle

```
Discover
   ↓
Evaluate              would a future agent or the owner benefit?
   ↓
Search                ffs_search, then ffs_list_path if needed
   ↓
Propose or extend     ffs_author_atom, shaped to a predicate
   ↓
Provenance            source_uri set; inference marked in content
   ↓
Await acceptance      the owner accepts or rejects in the daily summary
   ↓
Revisit               supersede when facts change; never overwrite
```

### 8.1 Discover and evaluate

During a task the agent notices information that may have lasting value and asks whether the owner or a later agent would want it.

### 8.2 Search

The agent MUST call `ffs_search` with the entity's name or the note's likely title before proposing. If the search returns a hit, the agent SHOULD read the projection (`ffs_render_projection`) to see what is already known.

### 8.3 Propose or extend

If the entity does not exist, the agent proposes it. If it exists, the agent proposes an addition in the shape the predicate expects (for a contact, a new `History` or `Notes` line; for a note, a new note that references the existing one). The scribe and the owner decide whether it becomes a supersession or a new atom.

### 8.4 Provenance

The agent SHOULD set `source_uri` on `ffs_author_atom` to the real origin when there is one (a file, a URL, a message id). When there is none, the MCP server stamps the agent's identity, which is correct: the origin was the agent.

### 8.5 Await acceptance

The call returns a `submission_id`. The agent's work with that content is done. It MUST NOT poll, retry, or re-submit the same content to get it accepted faster.

### 8.6 Revisit

When the agent later learns that a fact changed, it proposes the new fact with the same entity. Supersession preserves the old atom.

---

## 9. Read Before Write

For any substantial task involving the owner's knowledge, an agent SHOULD:

1. Identify the task and the entities it touches.
2. Search for those entities (`ffs_search`).
3. Read what exists (`ffs_render_projection` or `ffs_query`), only for the entities that matter.
4. Perform the task, using what was read.
5. Review what was learned (Section 10).
6. Propose additions or corrections (`ffs_author_atom`).
7. Report which submissions were made and that they await acceptance.

This prevents the substrate from becoming write-only.

---

## 10. Knowledge Review

After every substantial task, the agent SHOULD ask:

1. Did I learn a fact about a person, organization, or thing the owner tracks?
2. Did the owner make or state a decision?
3. Did I discover something non-obvious that a future agent would otherwise rediscover?
4. Did an existing claim in the substrate turn out to be wrong or outdated?
5. Did a relationship between entities change?
6. Did I produce an artifact the owner will want to find later?

If every answer is no, no proposal is needed. The review MUST NOT produce low-value proposals to appear thorough.

---

## 11. Progressive Disclosure

An agent MUST NOT dump the substrate into its context. The preferred discovery pattern is:

```
ffs_search / ffs_list_path      lightweight hits and listings
        ↓
ffs_render_projection           one rendered entity or listing
        ↓
ffs_query / ffs_resolve_url     the atoms behind it, only when needed
```

Rules:

- Use `ffs_search` with a small `limit` (the default is 10; the maximum is 50). Raise it only when the first page did not contain the answer.
- Read a listing (`ffs_list_path`) before rendering the entries in it.
- Render a projection before querying its atoms. Atoms are for provenance, history, and `as_of` questions, not for casual reading.
- Do not enumerate every letter under `contacts/by-name/` to answer a question that `ffs_search` can answer.
- Do not walk the filesystem under `~/.ffs/` as a substitute for the tools. Projection files are a cache, not the substrate.

---

## 12. Provenance and Trust

### 12.1 Generated is not verified

A proposal is generated content. An accepted atom is verified content, signed by the owner. An agent MUST NOT describe its own proposals as verified, accepted, saved, stored, or recorded.

### 12.2 The author key is the actor

Every atom is signed by exactly one author. An agent's identity is its key, or, for MCP proposals, the identity URI the server stamps. An agent MUST NOT attempt to present content as authored by the owner or by another agent.

### 12.3 Engine and model provenance

Proposals produced by the scribe carry `engine` and `model` provenance (ADR-026). An agent MUST NOT strip, forge, or paraphrase away that provenance when it reads or relays a proposal.

### 12.4 Sources

When content came from somewhere, the agent SHOULD say where: `source_uri` on the call, and a citation in the content when the predicate has a place for it (a note's `References` section).

---

## 13. Article Intake Contract

The courier (the deterministic skill bundle in `skills/courier/`, or an agent host running the same loop) turns a newsletter item, a feed record, or a clipped article into one Markdown file under `$FFS_DATA_DIR/ingest/`. The scribe and the resolver read that file; the contract below is what they read. A courier MUST write exactly this shape, and MUST NOT supply entity ids: identity is the resolver's (ADR-030).

### 13.1 One file per item

Filename: `<publication-slug>-<YYYY-MM-DD>-<title-slug>.md`. The slug is a projection basename and a resolver blocking key, never the identity.

Frontmatter keys, in this order:

| Key | Value |
|---|---|
| `predicate` | `source.article` (required) |
| `title` | the headline as printed (required) |
| `url` | normalized (required): lowercase scheme and host; `utm_*`, `fbclid`, `gclid`, `mc_cid`, `mc_eid` stripped; fragment dropped; trailing slash collapsed |
| `publication` | the outlet that sent the item |
| `published_at` | `YYYY-MM-DD` |
| `byline` | omit if unknown |
| `tags` | list; omit if empty |
| `intake` | `pointer` \| `clip` \| `feed` \| `morning_read` |
| `reported_by` | omit unless the item attributes another outlet |
| `content_hash` | omit unless a feed adapter had the record bytes; multibase base58btc of blake2b-256 |
| `fetch` | `off` \| `session` \| `scheduled`, the publisher's policy at filing time |

Body: empty for `pointer`; the blurb or article text for `clip`; the record's own summary for `feed`; the owner's note for `morning_read`. A clipped body lands under the `clip` classification tier and is never federated unless a capability names that tier (ADR-035). The scribe truncates any body over 4,000 characters and attaches a `parse-warning` rationale so the truncation is visible in review.

`## Mentions` bullets: `- <Name> — <one-line role or org context>` (a spaced em dash separates them). Each bullet becomes one `mentions[]` item `{entity, display, context}` with `display` and `context` verbatim and `entity` filled by the resolver. Pointer items have no Mentions section; clip items leave it for the scribe to fill from the body; feed items MAY pre-fill it from structured fields, tagged `courier-structured`, which the scribe treats as a high-confidence hint.

`## Events` bullets (optional): `- <kind>: <description> | <Name> (<role>), <Name> (<role>)`. Kinds: `funding`, `acquisition`, `hire`, `departure`, `expansion`, `opening`, `closing`, `award`, `partnership`, `other`. Roles: `acquirer`, `target`, `investor`, `investee`, `hire`, `employer`, `departing`, `landlord`, `tenant`, `developer`, `winner`, `partner`, `other`. Each named participant becomes a `participants[]` item `{entity, display, role}` resolved the same way.

### 13.2 The digest note

One per publication per day: `<publication-slug>-digest-<YYYY-MM-DD>.md` with frontmatter `predicate: note`, `title: <Publication> digest YYYY-MM-DD`, `tags: [digest, courier]`, the body `Filed by the courier.`, and a `## References` section listing every article filed that day as `- [[<article basename>|<title>]]`. The digest is how "did I read the paper end to end" is answered from the vault.

### 13.3 No full text unless policy says so

What a courier files for a publisher is the owner's policy in `$FFS_DATA_DIR/config/sources.toml` (`intake = pointer | clip`, `fetch = off | session | scheduled`). A courier MUST apply the owner's values as written and MUST NOT carry a domain denylist or allowlist of its own. The software records publishers' terms for the owner's judgment; it does not adjudicate them (ADR-035, amended 2026-09-15). Behavior, not domains, is what a courier refuses: it never crawls a listing, index, search, or section page, never follows a link found inside a fetched page, and never bypasses a login, a paywall, or a bot wall.

### 13.4 `reported_by` and source independence

When a newsletter item attributes another outlet ("(Charlotte Business Journal)", "(WBTV)"), the courier records that outlet as `reported_by`. For ADR-034's quorum the item then counts as a copy of that outlet's report, not as an independent confirmation. Two readers of one report are one source.

### 13.5 Persistence vocabulary

Both couriers use exactly these words, mirroring Section 18: **submitted** (the file was dropped; the watcher will pick it up), **proposed** (in the quarantine), **filed** (an accepted atom, visible in the vault), **auto-filed** (accepted by an ADR-029 policy). A courier says "submitted" or "would submit". It never says "filed".

---

## 14. Inference

An agent may draw conclusions from what it reads. Inferred content MUST be distinguishable from sourced content.

Prefer:

> Based on the 2026-03 meeting note and the org chart, the agent infers that Sara Chen now reports to the platform team. Unconfirmed.

over recording "Sara Chen reports to the platform team" as a bare fact.

When an inference is later confirmed by the owner or by a sourced document, the agent proposes the confirmed fact. The earlier atom, once accepted, remains as history.

---

## 15. Conflicts and Supersession

When new information conflicts with an existing claim, the agent MUST NOT silently overwrite the old meaning, and in FFS it cannot: atoms are immutable and change is supersession.

The agent SHOULD:

1. State the conflict in the proposed content ("earlier note says X; the 2026-06 email says Y").
2. Propose the current state clearly.
3. Leave resolution to the owner. Two accepted leaves for the same entity and predicate surface in the auditor's daily summary, and the owner supersedes explicitly to disambiguate.

An agent MUST NOT propose a supersession purely to make a chain look tidy.

---

## 16. Human Override

The owner's decisions have priority over anything the agent inferred.

- A proposal the owner **rejected** MUST NOT be re-proposed unless the agent has new evidence, and the new proposal MUST say what the new evidence is.
- A fact the owner **corrected** replaces the agent's version. The agent SHOULD adopt the correction in later reasoning without re-litigating it.
- A capability the owner **revoked** is not a bug to work around.

The daily summary is where the owner exercises this override. Agents do not get a vote there.

The same priority applies to publisher policy. What may be fetched or clipped from a publisher, and when, is the owner's configuration in `$FFS_DATA_DIR/config/sources.toml`. The software records the publishers' terms so the owner can decide and applies the owner's setting as written; it does not adjudicate the owner's license or fair-use position (ADR-035, amended 2026-09-15). An agent MUST NOT substitute its own reading of a publisher's terms for the owner's setting.

---

## 17. Capability Boundary

Every call passes the capability evaluator at the daemon. A denial comes back as a tool-level error with `kind: capability_denied` and a reason.

An agent MUST treat a denial as the substrate's answer:

- do not retry the same call;
- do not try to reach the same content through another tool or through the filesystem;
- do not ask the owner to grant a capability unless the task genuinely cannot proceed, and then say exactly which action, predicate, and classification was denied.

Content the agent cannot see is content it does not know. It MUST NOT guess at it.

---

## 18. Failure Handling and Persistence Honesty

`ffs_author_atom` returns a `submission_id`. That id means the content entered the quarantine. It does not mean the content is in the substrate.

An agent MUST report proposals in language that reflects this:

- Correct: "I proposed a contact for Sara Chen (submission `sub-0142`). It will appear in your daily summary for review."
- Incorrect: "I saved Sara Chen to your contacts."

If the call fails (transport error, capability denial, invalid params), the agent MUST report the failure and MUST NOT claim the content was persisted or proposed. Where useful it SHOULD hand the owner the content it intended to submit so a human can complete the write.

An agent MUST NOT batch-submit the same content under several `source_uri` values to improve its odds of acceptance.

---

## 19. Minimal Agent Contract

Any agent using FFS as memory MUST understand this contract:

1. Persistent knowledge belongs in the substrate, proposed through `ffs_author_atom` or `~/.ffs/ingest/`.
2. Search before proposing (`ffs_search`).
3. Read listings and projections before atoms; never dump the substrate.
4. Extend existing entities rather than creating near-duplicates.
5. Do not propose conversation, reasoning, or transient state.
6. Set `source_uri` to the real origin when there is one.
7. Mark inferences as inferences in the content.
8. Never overwrite; supersession preserves history, and the owner resolves conflicts.
9. Never re-propose what the owner rejected without new evidence.
10. A capability denial is the answer; do not route around it.
11. A `submission_id` means proposed, not saved. Never claim persistence that did not happen.

---

## 20. Language

This convention is written in English so it can be used as a language-independent agent instruction. Content in the substrate MAY be in any language the owner uses. An agent MUST preserve the language of existing content when extending it and SHOULD NOT translate existing claims.

---

## 21. Open Questions for v0.2

- **Full-text search over claim bodies.** `ffs_search` matches the canonical name field only. A body-text search (over `notes`, `history`, `body`) is needed for "what did I decide about the Acme deal" questions.
- **MCP-side supersession.** An agent can only propose through the scribe. A direct "supersede atom X with this claim" tool, still quarantined, would let agents propose precise corrections.
- **Agent-authored capability requests.** Today an agent that is denied can only report the denial. A structured request the owner reviews alongside proposals is an open design.
- **Stale detection via the auditor.** The daily summary flags multi-leaf heads and threshold anomalies. It does not yet flag claims whose `valid_to` has passed or whose source has changed.
- **Cross-substrate provenance.** When a federated atom arrives, the receiver records the capability hash that authorized it. How an agent should cite a peer's atom in its own proposals is undefined.

---

## 22. Summary

> **FFS defines how knowledge is represented: signed, classified, bitemporal atoms.**
> **This convention defines when and how an agent proposes to it.**
> **The `ffs-memory` skill teaches the workflow; the courier files by the article intake contract.**
> **The daemon guarantees validation and capability checks.**
> **The owner decides what becomes true.**
