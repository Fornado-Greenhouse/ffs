---
status: completed
title: "Spike: intake reality, does the courier need a browser or an agent (gates task_40)"
type: docs
complexity: low
dependencies:
  - task_26
---

# Task 43: Spike: intake reality, does the courier need a browser or an agent (gates task_40)

## Overview
The pipeline as first imagined starts with a Hermes agent that opens the Charlotte Business Journal email, clicks every link with the owner's logged-in session, and downloads each article. That is the first link in a daily chain, and it is the one with the most unknowns: what the digest email actually contains, whether bizjournals.com sits behind a bot wall, what the terms of use say about automated access, and whether an agent with a browser is needed at all when a deterministic script might do. If the blurbs already carry the who and the what, "click every link" is an exception, not the rule. This spike looks before task_40 builds. The intelligence belongs in the scribe, which FFS owns; the courier should be the dumbest thing that works.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST inspect five actual CBJ digest emails (kept out of git under `$FFS_DATA_DIR/spikes/task_43/` or another gitignored directory) and record, per item: headline, blurb length, byline present, URL shape, and whether the blurb alone names the people and organizations involved. Tabulate the count of items per email.
- MUST classify every item as blurb-only sufficient (people and orgs extractable from headline plus blurb) or full-fetch required, and compute the blurb-only percentage across all five emails.
- MUST test one full fetch of a bizjournals.com article URL three ways using only the Python standard library: (a) no session, (b) with a cookie jar exported from the owner's logged-in browser, (c) with a realistic User-Agent header added to (b). Record for each: HTTP status, whether the response is the article, a paywall interstitial, or a bot-wall challenge (Cloudflare or similar), and the response size. Do not retry aggressively; one attempt per variant.
- MUST read the site's terms of use and quote only the sentence or sentences that address automated access, scraping, or account sharing, with the URL and the date read. State the practical risk (account termination, not litigation, for personal use) without offering legal advice.
- MUST estimate reviewer-relevant coverage from email-only intake: of the items an owner would plausibly want filed (people and orgs of local business relevance), what fraction is captured from the blurb alone.
- MUST compare two courier shapes in writing, with a recommendation: (1) a deterministic stdlib Python skill bundle under `skills/courier/` run on a daemon schedule (`imaplib` for the mailbox, `email` for parsing, `urllib` with a cookie jar for optional full fetch, credentials from the OS keychain per task_27; zero pip deps per ADR-009; testable with fixture emails); (2) a Hermes or OpenClaw agent driving a browser. Compare on determinism, testability, failure modes, credential handling, and maintenance.
- MUST state the decision output for task_40: which shape is built first, and whether full fetch is a per-article opt-in (for example, only for items whose blurb lacks a person or org) or dropped from Phase 1.
- MUST write the findings to `docs/research/spikes/task-43-intake-reality.md` with the PASS/FAIL verdict stated in the first paragraph.
- MUST NOT modify any file under `crates/`, `skills/`, `starter/`, or `obsidian-plugin/`; MUST NOT commit email content, cookies, or article text.

PASS criterion for "email-only is enough": at least 70 percent of digest items yield people and organizations from the headline plus blurb alone.

What changes on FAIL: if fewer than 70 percent are blurb-sufficient and full fetch works, task_40 builds full fetch as the default for the courier. If a bot wall or paywall blocks fetch in all three variants, full fetch is dropped from Phase 1 and the courier files blurbs only, with the article URL as the provenance link for the owner to open by hand.
</requirements>

## Result (2026-09-14)

**Verdict: FAIL on the email-only criterion; full fetch blocked by terms, not engineering; owner decision recorded as ADR-035.** Findings: `docs/research/spikes/task-43-intake-reality.md`.

- The CBJ digest (delivered to the Fornado Greenhouse Workspace account, not the personal Gmail) is headlines and links only: 0 of 14 items yield a person or an organization from the email. The real article URL decodes from the base64 path of each tracking link, so a courier recovers canonical URLs without a click.
- Plain fetch is 403 on the site, the news feed, and the RSS host, with or without a browser user agent. The owner's signed-in Chrome session renders full articles (ten paragraphs, about 2,900 characters); measured once, nothing stored.
- ACBJ's User Agreement (2024-08-13) prohibits automated access and any LLM operation over its content; the Observer and Axios carry equivalent clauses. CLTtoday (6AM City) permits personal use and carries full blurbs; spike task_42 passed on it.
- The primary-source probe ranked Mecklenburg County's permit FeatureServer and Charlotte's Legistar Web API as the first two feeds to add (public record, JSON, no key), with SEC EDGAR third and Duke Energy and Truist RSS as a feed list. Both public feeds corroborated CBJ headlines from the same week.
- Owner's decision (quoted in ADR-035): "I don't want this to act as a bot performing a scrape. I want it to be my personal assistant, conducting our morning read." The courier files pointers and permitted feeds only (task_40 reshaped); the read is an owner-present session (task_48). Cookie-jar fetch (43.3) is not applicable.

## Subtasks
- [x] 43.1 Collect five CBJ digest emails into a gitignored spike directory; tabulate items with headline, blurb length, byline, URL shape, and who/what presence.
- [x] 43.2 Classify each item blurb-only vs full-fetch-required; compute the percentage.
- [x] 43.3 Fetch one article URL three ways with stdlib `urllib`; record status, response class, and size for each. *(Closed as not applicable per ADR-035: no-session fetch returned 403 across the site and RSS host; the cookie-jar variants were not run because automated fetch of bizjournals.com is out under the User Agreement and the owner's decision. The owner's signed-in browser was measured once: full article rendered, nothing stored.)*
- [x] 43.4 Read the terms of use; quote the automated-access clauses with URL and date; state the practical risk.
- [x] 43.5 Write the courier-shape comparison (deterministic skill bundle vs browser agent) with a recommendation and the full-fetch policy.
- [x] 43.6 Write `docs/research/spikes/task-43-intake-reality.md` with the verdict first and the decision output for task_40.

## Implementation Details
No production code. The fetch test is a ten-line script. The email inspection is reading. The courier comparison should reference the existing skill bundle shape (`skills/scribe/SKILL.md`, `skills/_lib/ffs_skill.py`) so the recommended shape is concrete for task_40.

### Relevant Files
- `skills/scribe/SKILL.md`, `skills/_lib/ffs_skill.py` — the bundle shape a `skills/courier/` would follow (read-only).
- `crates/ffs-core/src/store/keyring.rs` — the keychain helpers a courier would use for mailbox credentials (read-only).
- `docs/research/spikes/task-43-intake-reality.md` — the deliverable.

### Dependent Files
- `.compozy/tasks/ffs-mvp/task_40.md` — courier shape and full-fetch policy set by the verdict.
- `docs/agent-memory/skill/ffs-courier/SKILL.md` (task_40) — rewritten around the chosen shape.

### Related ADRs
- [ADR-009](adrs/adr-009.md) — stdlib-only skill bundles; the deterministic courier fits it.
- [ADR-026](adrs/adr-026.md) — ambient vs invoked; a scheduled courier is ambient and must be an explicit owner setting.
- [ADR-031](adrs/adr-031.md) — the URL as the article's bearer key; fetch policy affects `content_hash` availability.

## Deliverables
- Item table across five emails with the blurb-only percentage **(REQUIRED)**.
- Fetch test results for the three variants **(REQUIRED)**.
- Terms-of-use quotation with URL and date **(REQUIRED)**.
- Courier-shape comparison and recommendation; full-fetch policy for task_40 **(REQUIRED)**.

## Tests
- Verification is observational; no automated tests are added by this spike.
- [x] Every email item is classified.
- [x] All three fetch variants have a recorded result.
- [x] The verdict paragraph cites the percentage that decides it.

## Success Criteria
- The findings note exists, states PASS or FAIL in its first paragraph, and task_40's requirements are amended with the chosen courier shape and fetch policy, referencing the note.
- No email content, cookie, or article text is in the repository.
