# Spike 43: intake reality (findings, 2026-09-14)

**Verdict: FAIL on the email-only criterion, and full fetch is blocked by the publisher's User Agreement, not by engineering.** The Charlotte Business Journal digest carries headlines and links only, so zero percent of items yield people or orgs from the email. Automated fetching of article bodies and any LLM operation over bizjournals.com content are prohibited by ACBJ's terms. The recommended courier shape is therefore: CBJ as a pointer source (title, url, date, publication from the email, no content), with entity extraction run on sources whose license permits personal use, starting with the CLTtoday newsletter. **Owner decision (2026-09-14): ADR-035.** No unattended fetch of terms-restricted publishers; the assistant conducts an owner-present morning read instead, and the courier files pointers and permitted feeds only.

Real email and article content stays under `$FFS_DATA_DIR/spikes/cbj/` and never enters this repository.

## 1. Where the digest lives

- The CBJ digest is delivered to the Fornado Greenhouse Google Workspace account, not the personal Gmail account. Sender `reply@news.bizjournals.com`. Cadence: a morning edition around 8:10 AM, an afternoon edition around 5:10 PM, weekend editions, and occasional specials. More than fifty threads were visible.
- The personal Gmail account holds only the subscription's PayPal receipts and a second local source, the CLTtoday newsletter from 6AM City Inc (`hello@clttoday.com`, daily around 6:00 AM).
- Implication for task_40: the courier's mailbox is a Workspace account, so IMAP needs an app password or OAuth. The claude.ai Gmail connector was not required for this spike; the user's signed-in Chrome session sufficed.

## 2. What one digest contains

Afternoon edition of 2026-09-14, inspected in full:

| Measure | Value |
|---|---|
| HTML size | about 112 KB |
| Visible text | about 2.3 KB |
| Anchors | 50 |
| Unique bizjournals.com/charlotte links after decoding | 23 (about 14 news articles, 4 sponsored listings, digital edition and account links) |
| Blurb text per item | none: headline plus "Read full story" |
| People or orgs extractable from the email alone | 0 of 14 |

Link shape: `https://link.bizjournals.com/click/<campaign>.<id>/<base64url of the real URL>/<hash>`. The canonical article URL decodes from the path with no click, so a courier can recover URLs deterministically and never touch the tracker. This is the one useful engineering finding on the CBJ side.

Result against the PASS criterion (at least 70 percent of items yielding people and orgs from the blurb alone): 0 percent. FAIL by construction.

## 3. Fetch tests

| Variant | Result |
|---|---|
| Plain fetch, default user agent, homepage | 403 |
| Plain fetch, `/charlotte/feed/news` | 403 |
| Plain fetch, `feeds.bizjournals.com/bizj_charlotte` | 403 (XML content type, body blocked) |
| Plain fetch, browser user agent, homepage | 403 |
| Chrome profile before sign-in, article URL | Paywall preview only: "Preview this article", one sentence. |
| Chrome profile after the owner signed in to bizjournals.com, same URL | Full article rendered: no preview marker, 10 paragraphs, about 2,900 characters. Measured only; no content stored. Authenticated browser fetch works technically; the User Agreement in section 4 is the remaining blocker. |
| Exported cookie jar plus stdlib urllib | Not run; moot given section 4 unless the owner overrides |

A bot wall sits in front of the site and the RSS host. There is no unauthenticated escape hatch.

Transfer mechanics learned along the way: Gmail's page CSP blocks page scripts from posting to localhost, and the browser JavaScript tool truncates results near 1,000 characters, while the page-text tool allows 50,000. A browser-hosted courier moves text through page-text extraction, not scripts.

## 4. Terms of use

ACBJ User Agreement, last revised 2024-08-13, `https://www.bizjournals.com/useragreement`, Prohibitions on Use of the Service, quoted verbatim:

> "use any bots, cheats, macros, scripts, or run Maillist, Listserv or any form of autoresponder, or use any other automated process, or engage in meta-searching or periodic caching of information, to access, visit and/or use the Service"

> "copy, harvest, crawl, index, scrape, spider, mine, gather, extract, compile, obtain, aggregate, capture, access, store, or republish any Content ... for any and all purposes other than indexing Content for inclusion in a Search Engine, including but not limited to any purpose related to data mining and/or the training or operation of any software or service to the extent that it incorporates a large language model"

Stated remedies: termination or suspension of the account with or without notice; binding individual arbitration.

Reading: an automated courier that fetches full bizjournals.com articles, and any LLM extraction over stored bizjournals.com content, is what these clauses name, regardless of whether the bot wall can be passed. Parsing the subscriber's own email for headlines and URLs is the subscriber's own mail and is not covered. Filing title, url, date, and publication as a `source.article` record with no content is the defensible floor.

6AM City Legal (`https://6amcity.com/legal`, last revised 2025-03-01): a "non-transferable, non-sublicensable, non-exclusive, revocable, limited license to use and access the Site solely for your own personal, noncommercial use"; no copying, reproducing, or republishing of the Site or Services; no unreasonable load. No clause names large language models. Reading one's own subscribed newsletter and keeping a private, personal summary of the entities it names sits inside that license; republishing the blurbs would not.

## 5. CLTtoday as an extraction source

Five CLTtoday emails (2026-09-02 through 2026-09-14) were inspected. Each carries about 7 KB of text: full blurbs naming people, businesses, addresses, dates, and amounts across openings, closings, real estate, civic votes, hires, and awards. Email-only intake yields people, orgs, affiliations, and events here; spike task_42 measured extraction quality on exactly this material.

Attribution pattern that matters for ADR-034: many CLTtoday items end with a parenthetical outlet, most often the Charlotte Business Journal, the Charlotte Observer, WBTV, or Axios Charlotte. A CLTtoday item sourced to CBJ is a copy of a CBJ report, not an independent confirmation. The courier must carry that attribution into provenance so quorum counting can discount it.

## 6. Decision output for task_40

Recommended and adopted by ADR-035:

1. **Courier shape: deterministic skill bundle, email-only intake.** Not a browser agent. Read the Workspace mailbox, decode CBJ links, file `source.article` pointers (title, url, date, publication, no content), and file CLTtoday items as full submissions with blurb text and attribution.
2. **CBJ is a pointer source.** No fetch, no article text, no LLM over CBJ content. The briefing links the owner to articles; what the owner reads and writes down enters as the owner's own note.
3. **Full fetch is dropped from Phase 1**, on terms grounds. It is not an opt-in flag; it is out unless the owner decides to accept the account risk explicitly.
4. **Primary sources are the next intake sources**, and they double as ADR-034's independent confirmations: company newsrooms and press releases, SEC EDGAR, the NC Secretary of State business registry, Mecklenburg County permits and deeds, city and town council agendas. CBJ headlines become the trigger that tells the courier which primary sources to read.
5. **Spike 42 runs on newsletter blurbs and primary-source text**, not on CBJ bodies (done; see `task-42-extraction-quality.md`).

Scope changes: task_40's `fetch.mode = "cookies"` option and the `ffs-courier` agent-hosted alternative both lose their CBJ use case; keep the mechanism only if a permitted source needs it. task_43 subtask 43.3 (cookie-jar fetch) is closed as not applicable.

## 7. Open items

- Owner decision on section 6 item 3: made, ADR-035.
- IMAP or OAuth choice for the Workspace mailbox (task_40).
- Primary-source probe: done, see section 8.

## 8. Primary-source probe (added 2026-09-14)

Probe run against live endpoints; full notes with sample queries stay under `$FFS_DATA_DIR/spikes/primary-sources-probe.md`. Two public-record feeds already corroborate this week's CBJ headlines: the county permit feed returned a $41.1M data-center upfit, a $16.9M library, and an $11.6M Target alteration issued since 2026-09-01, two of which match CBJ stories from the same week, and the Council API returned the 2026-09-14 agenda with rezoning petitioners named. These are the independent confirmations ADR-034 counts.

## Ranking (value for the movers-and-shakers cabinet x ease and permissibility of automated intake)

| Rank | Source | Machine-readable | Terms for automation | Value | Verdict |
|---|---|---|---|---|---|
| 1 | Mecklenburg County permits, ArcGIS FeatureServer `AccelaAllPermits` | Yes, REST JSON, no key | Public record, county GIS; no automation clause found | High: who is building what, for how much, owner LLC names | Add first |
| 2 | Charlotte City Council, Legistar Web API (`webapi.legistar.com/v1/charlottenc`) | Yes, JSON, no token needed for Charlotte | Granicus API is public; 1,000-row cap per query | High: rezonings, contracts, incentive votes, with petitioner names | Add second |
| 3 | SEC EDGAR full-text search and `data.sec.gov` submissions | Yes, JSON, 10 req/s with a declared User-Agent | Fair-access policy permits declared automated clients | Medium-high for public companies (8-K officer changes, 13G, Form 4); thin for private Charlotte firms | Add third |
| 4 | Company newsroom RSS: Duke Energy, Truist | Yes, RSS XML | Press releases are meant to be redistributed | Medium: executive hires and moves straight from the source | Add as a feed list, cheap |
| 5 | NC Secretary of State business registry | No public API; weekly CSV via paid data subscription | Web search: "Automated or scripted searches ... are not permitted" | High for entity resolution (officers, addresses, aliases) | Later, via subscription |
| 6 | Mecklenburg Register of Deeds (meckrod.manatron.com) | Web portal only, login for full access | Disclaimer only; no automation clause found | Medium: property sales and grantor/grantee | Later, manual or licensed |
| 7 | NC courts (eCourts Portal, RPA program) | RPA online $495 setup; extracts need a $5,000 bond | Licensed access only | Low-medium for this cabinet | Not now |
| 8 | Charlotte Observer (McClatchy), Axios Charlotte | No | Both prohibit scraping and use for AI/ML per their terms (see below) | High editorially | Pointer sources only, like CBJ |
| 9 | Bank of America and Honeywell newsrooms, Charlotte Regional Business Alliance | No RSS found | n/a | Medium | Skip or manual |

### Recommendation
Add, after CLTtoday: (1) Mecklenburg County permits via the ArcGIS FeatureServer, (2) Charlotte City Council via the Legistar Web API. Both are public records with JSON endpoints, no keys, no automation prohibition, and both name the entities the cabinet is about (owner LLCs, petitioners, contractors) on the actual decision dates. Then EDGAR (declared User-Agent, 10 req/s) for the public companies, and the Duke and Truist RSS feeds as a cheap fourth. NC SoS data subscription is the resolver's alias table and should be priced when task_45 starts.

Out of scope, noticed: owner names on permits are frequently LLC vehicles; the resolver will need an org-to-org "vehicle of" relationship or an alias rule to connect Digital Moores Chapel LLC to Digital Realty, which the plan's known-gaps list already flags as unmodeled.
