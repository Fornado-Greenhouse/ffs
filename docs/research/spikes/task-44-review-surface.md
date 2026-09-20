# Spike 44: review load and the review surface (findings, 2026-09-20)

**Verdict: PASS for review as markdown.** The owner decided all 46 proposals in the inbox-file mock in 7 minutes, under the 10-minute criterion, and called the batch "a great extraction overall." The shipped five-item panel was not timed: the owner did not understand what its mock was for, which is itself the finding: a health-flag panel that shows five cards and hides forty is not a surface anyone would work a morning's batch through. The owner's one design request changes ADR-032's layout: group the decisions by source article so that the choices about one article sit together, rather than grouping by predicate as the mock did.

Decision outputs: the daily review surface is the inbox file, ordered by source; ADR-029's `max_per_day` defaults to 50; ADR-032 is accepted with the grouping amendment and the panel keeps only counts and a link.

All content in the mock is synthetic; only counts and patterns are recorded here.

## 1. What was measured

| Pass | Surface | Batch | Minutes | Decisions | Result |
|---|---|---|---|---|---|
| A | `inbox-mock.md` in Obsidian, no plugin | 46 proposals: 3 role changes, 6 ambiguous identities with two candidates each, 8 new people, 4 new organizations, 12 events, 13 articles; 110 checkboxes | 7 | 46 sections decided, 46 ticks, 0 contradictory ticks, 0 sections left pending | PASS (under 10 minutes; the owner would use it) |
| B | Shipped daily-summary panel, via `panel-mock.md` walk-through | Same 46 | not performed | not performed | The owner did not understand the mock; recorded as a finding, not a measurement |

Pass B's mock described what the shipped panel would force: five cards at a time, ten refreshes, 46 accept or reject clicks plus about 18 detours to compare candidates or check what a role change supersedes, no undo. The owner's reaction ("I don't really understand panel-mock.md") is the honest verdict on a surface that requires a walkthrough document to explain how it would be used for a queue. The panel stays what it was designed for: five health flags.

## 2. Patterns in the owner's decisions

- Every one of the 46 sections was decided with exactly one tick; the strict grammar held with no contradictory or malformed blocks.
- All 3 role changes were accepted as supersessions.
- For all 6 ambiguous identities the owner accepted the first-listed, higher-scoring candidate, including one matched via an alias rather than the display name. Nobody was filed as "someone new" and no "these are different people" assertion was made. The mock listed the higher score first, so this is consistent with either the scores being right or an ordering bias; the resolver's candidate order should stay score-descending and the read (ADR-035) should say the second candidate aloud rather than rely on the owner scanning past the first line.
- All 8 new people, 4 organizations, 12 events, and 13 articles were accepted; no rejects anywhere. With an `Accept` grant those 37 additive items would have auto-filed and never reached the file, so the owner's actual daily load at this batch size is 9 decisions, not 46.
- Obsidian wrote the ticks as uppercase `[X]`. A parser that only matched lowercase `[x]` would have counted zero decisions. ADR-032's grammar now requires accepting `[x]`, `[X]`, and any non-space character inside the brackets, since the editor chooses the form.

## 3. The owner's design feedback

Verbatim: "It would be great if we handled them article-by-article so that the choices are more coherently associated with one another."

The mock grouped by kind (role changes, ambiguous, people, organizations, events, articles), so the three role changes, the three "hire" and "departure" events describing the same moves, and the three articles reporting them were in six different places. Grouping by source puts each article's record, its people, organizations, affiliations, events, and any ambiguous resolution or role change together under one heading, and lets the owner affirm one article's whole set at once. This also matches the morning read (ADR-035), which walks the agenda article by article, so the inbox file becomes literally the read's agenda.

## 4. Derived settings and decisions

- **ADR-029 `max_per_day` default: 50.** A batch of 46 took 7 minutes and read as comfortable; 50 is one such morning. The owner may raise it in the grant scope. Because additive items auto-file under the grant, the cap bounds what auto-files, and the file bounds what the owner sees; together they are one morning.
- **ADR-032: accepted with the grouping amendment.** Sections ordered by source; a per-source "accept all under this article" tick is permitted, since it is one source the owner has read, not a bulk action across sources; cross-source items (merge suggestions, past-window attestations) go in a final Housekeeping section; the panel shows counts and a link to the inbox only.
- **task_39:** the review-surface requirement keeps only the accepted branch; `max_per_day` default text set to 50.
- **task_48:** the read's agenda order is the inbox file's source order, and "accept all under this article" is the per-source affirmation the owner may give aloud.

## 5. Fast-path implications (carried from ADR-032)

- A checkbox tick is a classifiable edit; the parser matches on the comment-carried submission and entity ids, not on visible text, and treats any non-space character inside the brackets as ticked.
- An ambiguous pick maps to the resolver's candidate entity id; "someone new" maps to `resolved_entity: "new"`.
- A "these are different people" tick writes `entity.different_from` and only takes effect together with a candidate pick.
- A per-source "accept all" tick expands to the individual accepts for every undecided section under that source, in order, and is refused (parse warning) if any section under the source is ambiguous and has no candidate pick.

## 6. Weakness of this spike

The mock was laid out by predicate, which is exactly what the owner objected to, so 7 minutes is an upper bound for a per-source layout. Pass B was not measured. The owner's timing template was left blank; the numbers above come from the owner's report in the session. The scratch substrate path (`load.sh`) was not used, so nothing was deleted afterwards because nothing was created.

## Appendix: the mock's shape

Frontmatter (`date`, `pending`, `decided`, `auto_filed`), one section per proposal with predicate, resolved entity or candidate list with scores and `matched_on`, source url, engine and model and confidence, submission id, rationale, and a decision block: `accept` / `reject` for ordinary proposals; for role changes an explicit "this supersedes" line above `accept (supersede)` / `reject`; for ambiguous proposals `accept as <candidate>` lines carrying `entity:` ids in HTML comments, `accept as someone new`, `these are different people: A vs B`, and `reject`. Closing sections: Decided (0) and Auto-filed today (0). The full file stays under `$FFS_DATA_DIR/spikes/spike44/`.
