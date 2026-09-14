# Keeping a shared fact base accurate: Wikipedia primitives, truth discovery, and k-of-N attestation

Research memo, 2026-09-14. Track C of the federation-resumption research. The owner's framing: shared intake across peers is not mainly about not repeating the reading; it is about "ensuring it's accurate every day, like Wikipedia would: in any collection of N hosts, some percentage of N has to agree it's still accurate, at least one, preferably two or more."

Sources are listed in §7. Claims that could not be checked against a primary source in this session are marked "unverified".

---

## 0. The one-paragraph answer

Wikipedia does not keep facts accurate by agreement among hosts. It keeps them accurate by making every change visible to many watchers (watchlists, recent changes, patrol), by requiring a citation per claim (verifiability, not truth), by dating claims that can go stale ("as of", with update categories), by marking review status without asserting truth (pending changes "accepted" means "checked for obvious problems"), and by ranking competing statements rather than deleting the losers (Wikidata preferred / normal / deprecated, with a reason). The research literature adds two things Wikipedia's process leaves implicit: agreement counts only among independent sources (Dong et al. 2009: copiers must be discounted), and a source's reliability is best estimated from how often its past claims turned out true (TruthFinder, Knowledge-Based Trust). The security world adds the mechanism: k independent signers attesting the same artifact (Certificate Transparency's multiple logs, in-toto's k-of-n rebuilders), with no consensus protocol, because the parties are trusted and the artifact is content-addressed. For FFS, that maps onto one new predicate, `attestation`, a quorum policy expressed as data, and a derived status (current / unconfirmed / stale / disputed) that the projections render and the briefing nags about.

---

## 1. Wikipedia maintenance primitives

### 1.1 Visibility: watchlists, recent changes, patrol

- **Watchlist.** "For every logged-in user, Wikipedia offers to maintain a list of pages you are interested in monitoring changes to, so-called 'watched' pages," and shows "a list of recent changes made to those pages (and their associated talk pages)." The watchlist is the per-person subscription; nobody watches everything, everyone watches something. (Help:Watchlist)
- **Recent changes patrol.** A volunteer process: "Identify 'bad' or 'needy' edits, Remove or improve the edit, Warn the editor, Check the user's other contributions." Its stated purpose: "it is really just a way to try to ensure that every edited article gets checked promptly." The patrol is "entirely voluntary and carries no obligation." (Wikipedia:Recent changes patrol)
- **Autopatrolled.** New pages by editors with this right "are automatically marked as 'reviewed'." The right confers "no additional technical abilities"; it exists to reduce reviewer load for proven contributors (about 25 valid articles created). This is a per-author trust level that changes whether a change needs eyes, not what the change may say. (Wikipedia:Autopatrolled)

### 1.2 Review status without a truth claim: pending changes

On English Wikipedia, pending changes protection means "edits from new or unregistered users are saved, but are marked as pending changes that are awaiting review." Readers see "the most recent accepted revision." Crucially, acceptance is not endorsement: the edit "has been checked for obvious problems." Only level 1 (new and unregistered users) is in use; level 2 was abandoned. The system descends from the flagged revisions trial. (Wikipedia:Pending changes)

The lesson: "reviewed" is a cheap, honest, explicitly weak signal. Wikipedia never lets a review state claim more than it checked.

### 1.3 Verifiability and citations per claim

"The threshold for inclusion is verifiability, not truth." "The burden to demonstrate verifiability lies with the editor who adds or restores material." `{{citation needed}}` flags an unsourced statement and gives time for a source before removal. (Wikipedia:Verifiability)

### 1.4 Dated statements: "as of"

`{{As of}}` marks "potentially dated statements" and adds the article "to the appropriate hidden sub-category of Category:Articles containing potentially dated statements," by month and year, with a visible "[update]" superscript. It must not be used for facts that will not change. `{{Update after}}` handles a known future date; `{{Update inline}}` and `{{When}}` are related. (Template:As of)

This is the closest thing Wikipedia has to `stale_after`: a date on the claim, a category that collects everything past it, and a reader-visible marker.

### 1.5 Conflict damping: edit warring and 3RR

"An editor must not perform more than three reverts on a single page, whether involving the same or different material, within a 24-hour period." A bright line, because edit warring "prevents consensus-building." (Wikipedia:Edit warring)

### 1.6 Bots

- **ClueBot NG** reverts vandalism using a Bayesian classifier plus a neural network trained on labeled edits; at its operating point it holds a 0.1 percent false-positive rate and catches about 40 percent of vandalism (0.25 percent and about 55 percent at a looser threshold). Post-filters include a whitelist and a one-revert-per-day cap per user and page. Humans can emergency-stop it. (User:ClueBot NG)
- **InternetArchiveBot** finds dead links and adds archive URLs, tagged or untagged, funded by the Internet Archive. Link rot is the reference-layer form of staleness. (User:InternetArchiveBot)

Both bots are precision-first: a low false-positive rate is chosen over recall, and every automated action is reversible by a human.

### 1.7 Quality assessment

WikiProjects grade articles Stub, Start, C, B, GA, A, FA, and importance Top, High, Mid, Low, recorded on the talk page banner. Assessment is coarse, per article, and separate from any per-fact status. (Wikipedia:Content assessment)

### 1.8 The "many eyes" evidence

- Giles, Nature, 2005: 42 science articles; reviewers found 162 factual errors in Wikipedia and 123 in Britannica, four serious errors in each; Wikipedia "comes close to Britannica." Britannica called the study "fatally flawed" (excerpts, youth-edition content, reviewer assertions unchecked) and noted Wikipedia had a third more inaccuracies. (Reliability of Wikipedia)
- Viégas, Wattenberg, Dave 2003 (history flow): vandalism "is usually repaired extremely quickly, so quickly that most users will never see its effects"; median survival minutes, mean survival days because of a long tail. A 2007 study: "42 percent of damage is repaired almost immediately," yet "hundreds of millions of damaged views" occur. Later work: median correction time around four minutes, while "some subtle forms of vandalism still persist for months and even years." (Reliability of Wikipedia; Counter-Vandalism Unit studies)

The reading for FFS: many eyes fix obvious damage fast and subtle staleness slowly. Subtle staleness is exactly what a movers-and-shakers cabinet accumulates (a person who quietly left), so the process must date claims and schedule re-checks rather than rely on someone noticing.

---

## 2. Wikidata primitives for accuracy over time

### 2.1 Ranks

Three ranks. **Preferred**: "the most current statement or statements that best represent consensus." **Normal**: the default, "provides no judgement or evaluation of a value's accuracy and currency." **Deprecated**: statements containing "errors (i.e. data produced by flawed measurement processes, inaccurate statements) or that represent outdated knowledge" in the sense of information "that was never correct, but was at some point thought to be." Historical facts with proper time qualifiers keep normal rank; a former mayor's term is not deprecated, it is dated. **P2241, reason for deprecated rank**, records why. (Help:Ranking)

This is the distinction FFS must keep: *superseded because the world changed* (bitemporal, history preserved) versus *deprecated because it was never true* (a correction). Both exist; they are not the same operation.

### 2.2 Temporal qualifiers

Qualifiers "allow statements to be expanded on, annotated, or contextualized." The temporal ones: **point in time (P585)**, **start time (P580)**, **end time (P582)**; "Louis XIV: King of France, from 14 May 1643 to 1 September 1715." (Help:Qualifiers). FFS's `valid_from` / `valid_to` are these two qualifiers promoted into the envelope.

### 2.3 References per statement and "retrieved"

"References are used to point to specific sources that back up the data provided in a statement." **Retrieved (P813)** records "the date when the data was taken from the web page." "The majority of statements on Wikidata should be verifiable." Community-edited sites should be replaced by original sources. (Help:Sources). The retrieved date is the attestation timestamp in all but name.

### 2.4 Disputes and constraints

- **P1310, statement disputed by**: a qualifier naming the "entity that disputes a given statement." Disputes are recorded on the statement, not resolved by deletion. (Property:P1310)
- **Property constraints** are "rules on properties that specify how properties should be used," reported on item pages, on Special:ConstraintReport, and in bot-maintained database reports. Common types: single-value, format, item-requires-statement, value-type, distinct-values. Constraints flag; they do not block. (Help:Property constraints portal)

### 2.5 Bots

"Bots can add interwiki links, labels, descriptions, statements, sources, and can even create items," and "have the ability to make edits very quickly and can disrupt Wikidata if they are incorrectly designed or operated," hence an approval process and a bot flag. (Wikidata:Bots). The parallel to ADR-029's Accept grant is exact: a named automated actor, scoped, approved, revocable.

---

## 3. Research on truth and freshness in knowledge bases

### 3.1 Truth discovery

- **TruthFinder** (Yin, Han, Yu; KDD 2007, TKDE 2008): the "veracity" problem, finding true facts among conflicting claims from many websites. The iterative principle: "a website is trustworthy if it provides many pieces of true information, and information is likely true if provided by many trustworthy websites." Outperformed search-engine ranking at identifying trustworthy sites on the test data. (Semantic Scholar; Illinois Experts)
- **Source dependence** (Dong, Berti-Equille, Srivastava; PVLDB 2009): agreement among sources is only evidence of truth when the sources are independent. Copiers must be detected and discounted, otherwise one wrong source copied widely wins the vote. Exact abstract wording unverified in this session (the PDF and the ACM page did not render), but the paper's thesis is well established and cited by the later fusion literature. (PVLDB vol 2)
- **Knowledge-Based Trust** (Dong et al., Google; PVLDB 2015): estimate a source's trustworthiness from "the correctness of factual information provided by the source" rather than link structure, using a multi-layer probabilistic model that separates extraction errors from source errors; applied to 2.8 billion facts and 119 million pages. (arXiv 1502.03519; PVLDB vol 8)

Together: count independent confirmations, weight each by the confirmer's track record, and separate "the extractor misread it" from "the source was wrong."

### 3.2 Temporal validity

- **TISCO** (Rula et al., Journal of Web Semantics 2019): temporal scoping of facts, deciding "the time intervals in which the fact is valid," because most knowledge bases "do not provide temporal information explicitly."
- **When Facts Expire** (CIKM 2025): the first benchmark for temporal fact validation, built from the May 2023 Wikidata dump by corrupting the temporal context of true facts; the motivating case is employment, "a person's employment at a company," which is precisely the affiliation atom. Venue and year per ACM DL; the paper body was not fetched (403). (ACM DL; Zenodo)
- A 2026 preprint on stale-fact errors in agent retrieval memory exists (arXiv 2606.26511); not read, listed for follow-up. Unverified.

The field's consensus is that facts carry validity intervals and that validating existing temporal facts is underexplored relative to predicting new ones. FFS's bitemporal envelope puts it ahead of most knowledge bases on representation; what it lacks is the validation loop.

### 3.3 Reversibility over prevention

ClueBot NG's operating point (0.1 percent false positives, about 40 percent recall) and the one-revert-per-day cap are the empirical answer to "how aggressive may an automated corrector be": very conservative, always reversible, and rate-limited per target. The same posture is right for any FFS automation that would supersede a fact.

---

## 4. Distributed and security primitives for "k of N agree"

- **Quorums (Dynamo, SOSP 2007).** N replicas, R readers, W writers, with R + W > N guaranteeing overlap; "sloppy quorum" and hinted handoff trade strict overlap for availability. The paper's exact text was not parsed in this session; the description here is from the standard account. Unverified quotes. The transferable idea: the policy is three integers, chosen per deployment, not a protocol.
- **OpenPGP certification levels (RFC 4880 §5.2.1).** Four signature types on a user id: generic (0x10, no assertion of how well checked), persona (0x11, "has not done any verification"), casual (0x12, "some casual verification"), positive (0x13, "substantial verification"). Multiple signers certify the same binding; each states how hard it looked. Most implementations use only 0x10. This is the model for an attestation that says *what basis* it rests on.
- **Certificate Transparency (RFC 6962, RFC 9162).** Certificates are logged in multiple independent append-only logs; clients that require SCTs from multiple logs make "collusion attacks between CAs and logs" harder; "all clients should gossip with each other, exchanging STHs." Chrome's policy defines log-operator uniqueness (separate operators, not separate logs from one operator). Independence of the attesters is a policy requirement, not an assumption. (RFC 9162; Chrome CT policy)
- **Sigstore Rekor.** "An immutable, tamper-resistant ledger of metadata"; verifiers "can also monitor the log for their identities"; witnesses (Omniwitness) and checkpoint monitoring audit that the log stays append-only. (Sigstore docs)
- **Reproducible builds and in-toto.** Federated rebuilders independently rebuild packages and "produce attestations using in-toto link metadata, making it possible to cryptographically assert that a package has been reproducibly built by a set of k out of n rebuilders"; in-toto's step thresholding is the mechanism. The Arch Linux verifier found "an unnoticed and security-relevant packaging issue affecting 16 packages." (in-toto at Reproducible Builds Summit; USENIX Security 2019; arXiv 2505.21642)
- **Why not Byzantine agreement.** Every system above assumes attesters are known, authenticated, and non-malicious in the aggregate; the guarantee is detection and accountability (signatures over a content-addressed artifact), not agreement under adversarial minorities. Among 5 to 20 mutually trusted peers with mTLS identities and capability atoms, the same holds. Consensus protocols solve a problem FFS does not have.

The common shape: an artifact with a content address; k independent, authenticated signers each publish a small signed statement about it with a timestamp and a basis; a verifier counts distinct signers against a policy threshold; nothing is deleted when signers disagree.

---

## 5. Fact-checking vocabulary

**ClaimReview** (schema.org): required `claimReviewed` (under 75 characters), `reviewRating` (numeric and textual), `url`; recommended `author` ("the publisher of the fact check article, not the publisher of the claim"), `itemReviewed` as a `Claim` with `datePublished` and `appearance`. Google is phasing it out of Search rich results but keeps it in the Fact Check Explorer. (schema.org; Google Search Central)

"Verified as of" operationally means: a named reviewer, a date, a rating, and a pointer to where the claim appeared. That is four fields, and all four should be on an FFS attestation.

---

## 6. Design implications for FFS

FFS already has: signed, content-addressed atoms with `valid_from` / `valid_to` and supersession; provenance per atom; generated-vs-verified semantics (ADR-027: a quarantined proposal is generated, an owner-accepted atom is verified by that owner); OKF's `verified: [{by, at}]` and `stale_after` as the convention FFS adapted; capability atoms as policy; pull-based, capability-filtered federation with watermarks among trusted peers; an auditor with a daily summary and a planned briefing (task_41). What follows adds the validation loop.

### 6.1 The `attestation` predicate (ADR-034, now)

An attestation is an atom about a fact. Shape:

```toml
name = "attestation"
[claim_schema.properties]
subject      = { type = "string" }   # the attested atom's content hash (multihash),
                                     # or "<entity>/<predicate>" to attest the current head
as_of        = { type = "string" }   # ISO 8601 date the attester believes the fact held
basis        = { type = "string", enum = ["re_read_source", "independent_source", "primary_source", "owner_knowledge"] }
source       = { type = "string" }   # url, ffs:// atom, or "person:<name>" for owner knowledge
note         = { type = "string" }
```

- `subject` by hash attests one immutable claim; `subject` by head attests "whatever is current for this entity and predicate at `as_of`". Prefer the hash: it is exact, and a superseded atom's attestations then belong to the historical claim, which is what Wikidata's normal-rank-with-dates behavior wants.
- Signed by the attesting peer's key like any atom. Federates like any atom, gated by capability. No path family; attestations render inside the attested projection as a "Confirmed" line.
- The owner's acceptance of a proposal is itself an attestation with `basis = re_read_source` (they read the proposal against its provenance) or `owner_knowledge` (they knew it). Emit it automatically on accept so k = 1 is met by the owner alone with no extra click.
- Relation to OKF `verified: [{by, at}]`: an attestation is one entry of that list, with two fields OKF lacks (`basis`, `source`) and a signature. Export to OKF is a projection of attestations onto `verified[]`.
- `basis` is the PGP certification level, made explicit. Two attestations with `basis = re_read_source` and the same `source` are one source, not two (§6.5).

### 6.2 Quorum policy as data (ADR-034, now)

Where the threshold lives: **in the predicate spec**, not in capabilities and not in a separate policy atom.

```toml
[attestation]
k = 1                 # distinct independent attesters needed for "current"
window_days = 90      # attestation age after which the fact becomes "unconfirmed"
independent = true    # count distinct sources, not distinct signers (see 6.5)
```

Reasons: the freshness horizon is a property of the kind of fact (an affiliation goes stale in months, a birth date never), so it belongs next to the claim schema; specs are git-versioned and hot-reloaded; capabilities answer "may this peer do X" and should stay orthogonal to "how much confirmation does this fact need." A per-substrate override in `config/attestation.toml` can raise k for a paranoid owner. Defaults: k = 1, window 90 days for `affiliation` and `person.generic`; k = 1, window 365 for `org.company`; no window for `source.article` and `event.business` (they record that something was published or happened, which does not go stale). A peer's attestation counts only if that peer is trusted for attestations: a capability grant with a new action `Attest` scoped to predicates, mirroring ADR-029's `Accept`.

### 6.3 Derived status: current / unconfirmed / stale / disputed (ADR-034, now)

Computed, never stored on the fact:

| Status | Rule |
|---|---|
| **current** | at least k independent attestations within `window_days`, and `valid_to` is null or future, and no open dispute |
| **unconfirmed** | fewer than k attestations ever, or the newest is older than the window; the fact is still shown, with a marker |
| **stale** | `valid_to` has passed, or a superseding atom exists; history, not error (Wikidata normal rank with an end time) |
| **disputed** | an open `dispute` exists (§6.4) |
| **deprecated** | superseded with provenance `kind: correction` and a reason; "was never true" (Wikidata deprecated rank plus P2241) |

Rendering: projections print "as of 2026-06-21, confirmed by you (re-read) and Alice (independent)" on current facts and "[unconfirmed since 2026-03-01]" on the rest, the way `{{As of}}` prints "[update]". The auditor counts each status per predicate; the briefing (task_41) gets a section "Past their window: 14 affiliations last confirmed more than 90 days ago, 11 by you alone" with a one-click "re-read source" that opens the article and records a fresh attestation, and a "these are unchanged" bulk action that records `owner_knowledge` attestations, which is the cheapest honest re-confirmation and must never be automatic.

### 6.4 Disputes (ADR-034 shape now, UI with task_39)

When a pulled peer atom contradicts a local head (a different current affiliation for an entity the two substrates have linked with `entity.same_as`), do not resolve. Reuse ADR-029's conflict route: the peer's atom arrives as a proposal, the classifier marks it Conflicting, it lands in review. Add a `dispute` marker as an attestation variant: `basis = "contradicted_by"` with `source` = the contradicting atom's hash, so the status derivation can show "disputed" until the owner accepts one side. This is P1310 as an atom. Three-revert-style damping: a peer whose atoms the owner rejects three times on one entity stops auto-proposing for that entity until the owner re-enables; the auditor reports it.

Later: truth-discovery weighting. Once attestations and accept/reject history exist, a peer's reliability per predicate is the fraction of their proposals the owner accepted, and the resolver (task_45) can weight candidates from reliable peers higher, TruthFinder-style. Not now; the data does not exist yet.

### 6.5 Source dependence: two readers of one article are one source (ADR-034, now)

Dong et al.'s point is the one most likely to be missed. If Alice and Bob both file the same CBJ article, their two attestations share a `source` url and a `content_hash` (task_40). With `independent = true`, the quorum counts distinct sources, so the fact has one confirmation, not two. A second publication, a primary source (the company's own announcement, a filing), or owner knowledge counts as a second. Certificate Transparency's operator-uniqueness rule is the same idea. The attestation's `basis` and `source` fields exist for this reason; without them, k of N degrades into "N copies of one reporter's sentence."

### 6.6 What not to build

- Byzantine consensus or any agreement protocol: peers are authenticated and trusted; the guarantee wanted is accountability and freshness, which signatures plus counting provide.
- Global reputation scores: the group is 5 to 20 people; per-owner, per-predicate acceptance rates are enough and stay local.
- Automatic reverts or automatic supersession from peer disagreement: ClueBot NG's posture is the ceiling, precision-first, reversible, rate-limited; FFS goes further and does nothing automatic to an existing fact.
- Deleting losing statements: Wikidata deprecates with a reason; FFS supersedes with provenance. Nothing is erased.
- A separate "review status" field on atoms: status is derived from attestations and time, so it cannot drift from the evidence.

### 6.7 What belongs in ADR-034 now versus later

| Item | When |
|---|---|
| `attestation` predicate; owner accept emits an attestation automatically | ADR-034 now; implement with task_39 (it touches the accept path) |
| `[attestation]` table in predicate specs (k, window_days, independent) and starter defaults | ADR-034 now; task_38 ships the table in the loader and specs |
| Derived status and its rendering in projections; auditor counts | ADR-034 now; task_38 (render) and task_41 (counts, "past their window" section) |
| `Attest` capability action for peers | ADR-034 now; implement when federation resumes (ADR-033) |
| Dispute marker; peer contradiction routes to review via ADR-029 | ADR-034 shape now; UI with task_39 |
| Independent-source counting by `source` and `content_hash` | ADR-034 now; data arrives with task_40 |
| Three-strikes damping per peer per entity | later, with federation |
| Reliability weighting of peers and sources in the resolver | later, after accept/reject history exists |
| OKF `verified[]` export of attestations | later, with any OKF export |

---

## 7. Sources fetched or searched in this session

Wikipedia and Wikidata (fetched):

- https://en.wikipedia.org/wiki/Wikipedia:Pending_changes
- https://en.wikipedia.org/wiki/Wikipedia:Recent_changes_patrol
- https://en.wikipedia.org/wiki/Wikipedia:Autopatrolled
- https://en.wikipedia.org/wiki/Help:Watchlist
- https://en.wikipedia.org/wiki/Wikipedia:Verifiability
- https://en.wikipedia.org/wiki/Template:As_of
- https://en.wikipedia.org/wiki/Wikipedia:Edit_warring
- https://en.wikipedia.org/wiki/User:ClueBot_NG
- https://en.wikipedia.org/wiki/User:InternetArchiveBot
- https://en.wikipedia.org/wiki/Wikipedia:Content_assessment
- https://en.wikipedia.org/wiki/Reliability_of_Wikipedia
- https://en.wikipedia.org/wiki/Wikipedia:Counter-Vandalism_Unit/Vandalism_studies/Study1 (search result)
- https://www.wikidata.org/wiki/Help:Ranking
- https://www.wikidata.org/wiki/Help:Qualifiers
- https://www.wikidata.org/wiki/Help:Sources
- https://www.wikidata.org/wiki/Property:P1310
- https://www.wikidata.org/wiki/Help:Property_constraints_portal
- https://www.wikidata.org/wiki/Wikidata:Bots

Research papers:

- Yin, Han, Yu, "Truth Discovery with Multiple Conflicting Information Providers on the Web," KDD 2007 / TKDE 2008: https://dl.acm.org/doi/10.1145/1281192.1281309 , http://hanj.cs.illinois.edu/pdf/kdd07_xyin.pdf
- Dong, Berti-Equille, Srivastava, "Integrating Conflicting Data: The Role of Source Dependence," PVLDB 2(1) 2009: https://dl.acm.org/doi/10.14778/1687627.1687690 , http://www.vldb.org/pvldb/vol2/vldb09-pvldb47.pdf (PDF and ACM page did not render; thesis stated from the literature, quotes unverified)
- Dong et al., "Knowledge-Based Trust: Estimating the Trustworthiness of Web Sources," PVLDB 8(9) 2015: https://arxiv.org/abs/1502.03519 , https://www.vldb.org/pvldb/vol8/p938-dong.pdf
- Rula et al., "TISCO: Temporal scoping of facts," Journal of Web Semantics 2019: https://www.sciencedirect.com/science/article/abs/pii/S1570826818300453
- "When Facts Expire: Benchmarking Temporal Validity in Knowledge Graphs," CIKM 2025: https://dl.acm.org/doi/10.1145/3746252.3761648 (403; abstract via https://zenodo.org/records/15680977 )
- "Temporal Validity in Retrieval Memory," arXiv 2606.26511 (not read; unverified)
- Viégas, Wattenberg, Dave, "Studying Cooperation and Conflict between Authors with history flow Visualizations," CHI 2004 (numbers via Reliability of Wikipedia and Wikipedia vandalism studies pages)

Distributed systems and security:

- DeCandia et al., "Dynamo: Amazon's Highly Available Key-value Store," SOSP 2007: https://www.allthingsdistributed.com/files/amazon-dynamo-sosp2007.pdf (PDF did not render; N/R/W description unverified against the text)
- RFC 4880 OpenPGP §5.2.1 certification types: https://www.rfc-editor.org/rfc/rfc4880
- RFC 9162 Certificate Transparency 2.0: https://www.rfc-editor.org/rfc/rfc9162.html ; Chrome CT policy: https://googlechrome.github.io/CertificateTransparency/ct_policy.html
- Sigstore Rekor overview: https://docs.sigstore.dev/logging/overview/
- in-toto at the Reproducible Builds Summit 2018: https://ssl.engineering.nyu.edu/blog/2019-01-18-in-toto-paris ; Torres-Arias et al., USENIX Security 2019: https://www.usenix.org/system/files/sec19-torres-arias.pdf ; rebuilderd: https://github.com/kpcyrd/rebuilderd ; Arch Linux independent verifier: https://arxiv.org/abs/2505.21642

Fact-checking:

- schema.org ClaimReview: https://schema.org/ClaimReview ; Google Search Central fact check markup: https://developers.google.com/search/docs/appearance/structured-data/factcheck
