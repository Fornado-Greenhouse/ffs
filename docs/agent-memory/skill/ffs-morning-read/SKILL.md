---
name: ffs-morning-read
description: Conduct the owner-present morning read of the day's FFS agenda (ADR-035). Use when the owner says "morning read", "read the paper with me", "what's in today's digest", or asks to walk through the inbox together. Never runs unattended, on a schedule, or in response to a request to open every link; it is a session with the owner present, one article at a time, on their cue.
---

# FFS Morning Read

The courier (`skills/courier/`) files pointers and permitted feeds on a schedule. The morning read is the other half of the newspaper goal: an interactive session in which the owner is present and works through the day's agenda with you at human pace. You open one article at a time when the owner says so, summarize it in the session, propose what is worth remembering, and file only what the owner chooses. This skill states the rules first because they are the point.

## 1. The rules, as refusals

These five sentences are the behavioral line ADR-035 draws. A docs test asserts they are present verbatim.

1. One article at a time, on the owner's cue; never before it.
2. No bulk opening: a request to open all of today's links is refused as bulk behavior, whatever the publisher.
3. No prefetching, no background opening, no scheduled opening in the read; only articles named in the owner's own digest; never crawling.
4. No article body is stored except the one the owner said to clip, and that one lands under the clip tier.
5. Summaries are spoken; only chosen notes and clips are filed; inferences are marked; persistence claims name only what the tools returned.

Publisher policy is the owner's `$FFS_DATA_DIR/config/sources.toml`, never a rule about a domain. You honor `fetch = "session"` by opening that publisher's articles only inside the read and only on the owner's cue, and `fetch = "off"` by never opening that publisher yourself: the owner opens the article and pastes what matters. A refusal is always phrased as a refusal of the behavior ("that is a bulk open"), not of the publisher.

## 2. What the host must provide

- An MCP client with the `ffs-mcp` tools (`ffs_list_path`, `ffs_query`, `ffs_search`, `ffs_render_projection`, `ffs_author_atom`, `ffs_accept_proposal`, `ffs_inspect_predicate`, `ffs_audit_query`, `ffs_resolve_url`).
- A capability to open one URL in the owner's own browser session and read its page text. Name it by what it does, not by a vendor. Without it, the read degrades gracefully: the owner opens the article and pastes what matters, and you proceed from the pasted text.

## 3. The session, step by step

1. **Open the agenda.** Read today's `inbox/<YYYY-MM-DD>.md` with `ffs_render_projection` when it exists (ADR-032: the quarantine projected as a file, sections ordered by source). When it does not, list pending submissions through the daemon's pending list and group them by source yourself. Walk the agenda in the file's source order, so one article's pointer, its extractions, its ambiguous resolutions, and its role changes are handled together. For every ambiguous item, speak every candidate aloud with its score and what matched; do not lead with the first one (spike 44 showed the owner picks the first listed).
2. **Wait for the cue, then open one.** For a pointer, say the headline and the publisher and wait. When the owner says "open it", open exactly that URL in the owner's session, read the page text, and summarize aloud in a few sentences. Do not open the next one until asked.
3. **Propose.** Name the people, organizations, affiliations, and events worth remembering in the ADR-031 shapes: a person or org as `display` plus one line of `context`; an affiliation as person, organization, title, kind; an event as kind, participants with their role in the event. Say which are stated in the article and which you infer, and mark inferences as inferences in what you file.
4. **File the owner's chosen notes.** Call `ffs_author_atom` with `source_uri` set to the article url and content in the article intake contract (CONVENTION.md section 13) carrying the read's frontmatter:

   ```markdown
   ---
   predicate: person.generic
   display_name: Pat Example
   role: chief executive
   organization: Example Maker Co
   intake: morning_read
   owner_present: true
   session: 2026-09-21-0814
   actor: mcp:agent/claude-code
   ---
   Stated in the article: named chief executive of Example Maker Co on 2026-09-20.
   ```

   The scribe turns `intake`, `owner_present`, `session`, and `actor` into provenance, never into claim data (section 4). The result is a `submission_id` for a proposal; say "proposed".
5. **Accept on "file it".** When the owner says to file it, call `ffs_accept_proposal { submission_id, owner_present: true }` (with `choices` or `resolved_entity` when the set has an ambiguous proposal and the owner picked). The daemon signs the atoms with the owner key and emits the ADR-034 attestation with basis `owner_knowledge`, because the owner read it live. Only the hashes the tool returns are "filed".
6. **Identity decisions.** "These are different people" and "same person as" are the owner's calls. Tick the corresponding line in the inbox file (the fast path turns it into the daemon's `entity.assert_different` or `entity.merge` call), or ask the owner to tick it; there is no MCP tool for merges by design, so you never merge on your own.
7. **"Clip this."** When the owner says to clip the open article, file it as a `source.article` with its body: the contract frontmatter (`title`, `url`, `publication`, `published_at`), `intake: morning_read`, `owner_present: true`, `session`, `actor`, `content_hash` of the page text, and the article text as the body; then accept it with `ffs_accept_proposal`. The signer stores it under the `clip` classification tier, which no "any classification" capability covers, so a federation pull never serves it unless a capability names `clip`. This is the owner's library copy. No other article text is ever filed.
8. **Refuse bulk.** "Open all of today's links", "read everything and file it", "keep reading while I'm away": refuse with the reason (bulk behavior, unattended behavior), record the refusal in the session log, and continue with the next item on cue.
9. **Close.** Write the day's digest note if the courier did not (a `note` whose `## References` lists what was read and filed as `[[basename|title]]`), file the session log (section 5), and give a two-line recap that names only what the tools confirmed: what was filed (hashes returned), what is proposed (submission ids), what was attested. Nothing is described as saved that the accept path did not return.

## 4. Provenance of a read-filed atom

The atom envelope is frozen: a provenance entry is `{kind, uri, hash}` and nothing else (ADR-017). The read is therefore encoded as two entries on every atom filed from it:

| kind | uri | hash |
|---|---|---|
| `morning_read` | the article url the owner had open | the submitted content's hash |
| `session` | `ffs-session://<actor>/<session id>?owner_present=true` | the session id's digest |

The scribe writes both from the four frontmatter keys above and drops the keys from the claim. `ffs_query` shows them on the atom. A clipped article additionally carries the `clip` classification.

## 5. The session log

At the end of the read, file one `note` titled `Morning read <YYYY-MM-DD>` with tags `[morning-read, session-log]` and these sections, each present only when it has items: `## Opened` (`- <time> <url>`), `## Proposals` (`- <predicate>: <display> (filed | proposed | skipped)`), `## Refusals` (`- <time> <request>: refused as bulk behavior`), `## Clips` (`- [[basename|title]]`). The scribe bundle exposes `render_session_log(...)` in `skills/scribe/contract.py` for hosts that want the exact shape. The log is the audit trail ADR-035 relies on: every open and every refusal is in it, and it is filed through the same accept path as everything else.

## 6. The accept path, decided

`ffs_accept_proposal` exists because an agent host may have no shell for the `ffs` CLI and the read is conducted through MCP. It is a thin pass-through to the daemon's `ingest.accept`, refuses anything but a literal `owner_present: true` before touching the daemon, and is documented as valid only in an owner-present session. It adds the ninth tool to the catalog (ADR-027 named eight); no other tool was added, and merges deliberately have none.

## 7. Worked example

Agenda for 2026-09-21 (synthetic; placeholder publisher and urls):

1. Pointer: "Widget maker breaks ground on second plant" (Example Ledger, `https://news.example-ledger.test/2026/09/21/widget-plant`).
2. Newsletter extraction: "Harbor & Pine Coffee opens third cafe" with two mentions, Casey Rivera (owner) and Harbor & Pine Coffee (org).
3. Ambiguous: "Sam Ortiz" resolved to two candidates, Sam Ortiz (Example Maker Co, 8.1, organization) and Samuel Ortiz (Ortiz Legal, 6.4, alias).

The session:

- You: `ffs_render_projection { path: "inbox/2026-09-21.md" }`. "Three items today. First, a pointer from Example Ledger: Widget maker breaks ground on second plant. Open it?"
- Owner: "Open it."
- You: open that one URL in the owner's session, read the page text. "Example Maker Co is building a second plant in Concord; the chief executive, Pat Example, is quoted. Stated: the plant and the role. I infer nothing beyond that. File Pat Example as a person with an affiliation to Example Maker Co?"
- Owner: "File it."
- You: `ffs_author_atom { source_uri: "https://news.example-ledger.test/2026/09/21/widget-plant", content: <the frontmatter block from section 3 plus one affiliation bullet> }` returns `submission_id: sub-0042`. "Proposed as sub-0042." Then `ffs_accept_proposal { submission_id: "sub-0042", owner_present: true }` returns two hashes. "Filed: the person and the affiliation, two atoms."
- You: "Second, from the newsletter: Harbor & Pine Coffee opens a third cafe; two mentions, Casey Rivera as owner and the cafe as an organization. Both are stated. Accept both?"
- Owner: "Accept all under that one."
- You: `ffs_accept_proposal { submission_id: "sub-0043", owner_present: true }` returns three hashes. "Filed: the article, the person, the organization."
- You: "Third is ambiguous. The article mentions Sam Ortiz. Two candidates: Sam Ortiz at Example Maker Co, score 8.1, matched on organization; and Samuel Ortiz at Ortiz Legal, score 6.4, matched on an alias. Which one, or someone new?"
- Owner: "The Example Maker one. And open all of tomorrow's links now so we're ahead."
- You: `ffs_accept_proposal { submission_id: "sub-0044", owner_present: true, resolved_entity: "zEx4mpl3M4k3rS4m0rt1z" }` returns one hash. "Filed against Sam Ortiz at Example Maker Co. I won't open tomorrow's links: that is a bulk open, and the read opens one article at a time on your cue. Logged the refusal."
- You: file the session log and the digest, then the recap: "Filed six atoms across three items and their attestations; nothing is pending. One request refused as bulk behavior."

## 8. Checklist before you say "done"

- Every open in the log was on the owner's cue, and there is one per article.
- Every "filed" in the recap has a hash the accept tool returned; everything else is "proposed" or "skipped".
- The only stored article body is the one the owner clipped, and it is under the `clip` tier.
- Inferences are marked in what was filed.
- The session log is filed.
