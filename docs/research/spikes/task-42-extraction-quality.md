# Spike 42 findings: extraction quality on local business-press blurbs

Date: 2026-09-14. Task: `.compozy/tasks/ffs-mvp/task_42.md`. Status: run once, scored by hand-labeled gold. No production code touched. No newsletter text appears in this note; raw inputs, model outputs, gold labels, and the throwaway scripts live under `~/.ffs/spikes/cbj/spike42/` and never enter git.

## Setup

- **Corpus.** Five daily CLTtoday newsletters (6AM City, 2026-09-02 to 2026-09-14), condensed to their news items (the events calendar, sponsored items, and shopping sections were dropped). About 1.2 to 2.3 KB of text per file. Not Charlotte Business Journal article bodies: the ACBJ User Agreement prohibits automated access and any use tied to operating a large language model over their content, while 6AM City's license permits personal, noncommercial use of the subscriber's own newsletter (see spike 43). CLTtoday blurbs are shorter and cleaner than a CBJ article, so treat these numbers as an upper bound on blurb-style input, not as article-body numbers.
- **Prompt.** Rendered from a draft `schemas.json` holding the ADR-028 and ADR-031 claim shapes (`person.generic` v2, `org.company`, `affiliation`, `event.business` with `participants[{display, role}]`, `source.article` with `mentions[{display, context}]`), predicate name plus description plus JSON claim_schema each, exactly the task_36 recipe. One submission per newsletter. Prompt size 5.8 to 7.0 KB.
- **Backends.**
  - Local: Ollama `llama3.1:8b`, `/api/chat`, `format: json`, temperature 0, 8k context. Apple Silicon Mac Mini class machine.
  - Claude: Claude Code CLI headless (`claude -p --output-format json --model sonnet --tools ""`), which answers with `claude-sonnet-5`. No API key in the environment; the CLI's session was used. Note: the run logger recorded the first entry of the CLI's `modelUsage` map, which is a small Haiku helper call Claude Code makes on every invocation; the extraction itself was answered by Sonnet 5.
- **Gold.** Hand-labeled from the five files: 8 people, 47 organizations, 7 affiliations, 33 events, plus "optional" entries (venues, sub-brands, minor events) that count if produced but are not required, and an "ignore" list of parenthetical attribution sources that are neither hits nor misses.
- **Scoring.** Case-insensitive normalized name matching (people and orgs by name, affiliations by person plus organization pair, events by kind plus primary participant, with a kind-lenient variant). Hallucinations (names absent from the text) counted separately from wrong-field errors.

## Results

| Predicate | Claude Sonnet 5 P / R | llama3.1:8b P / R |
|---|---|---|
| person.generic | 1.00 / 1.00 (8 of 8, 0 false) | 0.21 / 0.38 (3 of 8, 11 false) |
| org.company | 1.00 / 0.96 (45 of 47) | 1.00 / 0.32 (15 of 47) |
| affiliation | 1.00 / 1.00 (7 of 7) | 0.08 / 0.14 (1 of 7, 11 false) |
| event.business, strict kind | 0.91 / 0.88 (29 of 33) | 0.45 / 0.30 |
| event.business, kind-lenient | 0.97 / 0.94 | 0.95 / 0.55 |
| Hallucinated names | 0 | 0 people or orgs; 10 of the 11 false affiliations had an empty organization |
| Schema-invalid proposals | 0 of 111 | 25 of 72 |
| Latency per newsletter | 112 to 229 s through the Claude Code CLI (mostly CLI overhead: a 19.7k-token system prompt is cached per call; the API itself is a fraction of that) | 35 to 124 s |
| Cost per newsletter | about 0.08 to 0.15 USD at list price through the CLI, dominated by the CLI's own system prompt | 0 |

**Verdict against task_42's criteria.** Claude: PASS. People and orgs are at or above 0.85 precision and 0.75 recall with margin; affiliations are above 0.6. Local llama3.1:8b: FAIL on every predicate except org precision; recorded as required.

Both backends produced the `{display, context}` mention shape and a one-paragraph summary on every file; Claude filled `mentions` with 7 to 16 entries per newsletter, all well-formed. Claude's rationales flagged the two places it categorized rather than copied (an industry label, a construction year used as a date), which is the inference-marking behavior ADR-027 asks for.

## Failure modes, paraphrased

Claude Sonnet 5:
- Two org misses were a research network named in passing and a school district named by its abbreviation; both were captured as event participants rather than as org records.
- Two event-kind disagreements with the gold: a land purchase labeled `other` instead of `acquisition`, and a fundraiser modeled as a `partnership` between the two chefs. The kind-lenient score shows the events themselves were found.
- A restaurant that both closed and changed hands was emitted as one acquisition event rather than a closing plus an acquisition.

llama3.1:8b:
- Files organizations as people: in two of five files it put restaurants, a utility, a townhome project, and a government body under `person.generic`.
- Emits affiliations with no organization (one file produced ten of them), which is why affiliation precision is near zero and why a schema validator with required fields is not optional.
- Invents event kinds outside the enum (demolition, launch, study, revote, performance, anniversary), which the validator rejects; with the kind constraint relaxed its event recall doubles, so the enum is the limiting factor, not entity detection.
- Recall collapses on longer files: the two files with the most items produced the most invalid output, and the richest file produced only six proposals.

## Implications

**For task_36 (the llm engine).** Ship the schema-driven prompt as designed; it worked unchanged on Sonnet 5 with zero invalid proposals. Keep the minimal validator with required-field and enum checks in the critical path: it is the only thing standing between the local model's output and the quarantine. Fallback to the heuristic engine on validation failure is the right default for local models at this size. A local-only install with an 8B model is a "digest only" tier: it can produce a summary and org names with high precision, but it must not be trusted to file people or affiliations without review. A larger local model was not tested and should be before the local path is recommended.

**For the Claude backend specifically.** Do not route production extraction through the Claude Code CLI: two to four minutes per newsletter and a per-call system-prompt cost make it a spike tool only. The Anthropic Messages API with the same prompt is the production shape task_36 already specifies.

**For ADR-030's resolver weights.** On this sample the hints hold. Every person came with a stated organization or role, and organization agreement was the discriminating context in all seven affiliations. No surname collisions occurred in five files, so the "surname alone is weak" weight is untested here; it stays a conservative default. Two orgs appeared under slightly different names across files (a restaurant with and without its cuisine descriptor, a school district by full name and by initials), which is exactly the alias-table case: the resolver's alias tier and the accepted-mention alias growth in ADR-030 would have collapsed both.

**For ADR-029's additive rule.** All 111 Claude proposals were new entities or new events; none would have superseded an existing scalar. On a fresh substrate the auto-file path would have filed everything, which is why the daily cap and the review of new people on first sighting (ADR-030's NIL policy) matter more than the conflict rule in the first weeks.

**For ADR-034's independence rule.** Twelve of the news items carried a parenthetical attribution to another outlet. The prompt's instruction to treat those as sources rather than subjects worked on Sonnet 5 (zero attribution outlets filed as orgs). The courier should carry that attribution into provenance so a CLTtoday item sourced to CBJ counts as CBJ, not as a second source.

## Caveats

- Five newsletters, about 8 KB of text in total, one run per backend at temperature 0. Enough to decide the engine question; not enough to tune thresholds.
- Blurb input, not article bodies. Article bodies have more names per document, quotes, and history; expect lower recall on affiliations and more duplicate mentions per entity.
- The gold set was labeled by the same agent that wrote the prompt, in one pass. A second labeler would move a few event kinds.
- Local model tested: one, at 8B parameters, seven-month-old weights.
