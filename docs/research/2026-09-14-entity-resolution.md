# How does the same person across fifty articles stay one file?

Research memo, 2026-09-14. Sources are cited inline; anything not confirmed against a fetched source in this session is marked **(unverified)**. Written for tasks 36, 38, 39, and 40 and for a future ADR on entity identity.

---

## 0. The one-paragraph answer

Wikipedia keeps one page per person by separating three things that FFS currently conflates: the **identity** (Wikidata's opaque, permanent QID), the **names** (a label plus any number of aliases, none of which need to be unique), and the **page title** (a human-readable string that can change, be disambiguated with parentheses, and leave a redirect behind). Merges never delete: the losing identity becomes a permanent redirect to the winner, history stays attached, and a wrong merge is undone by restoring both sides. Two explicit human assertions bracket the automation: "said to be the same as" records a suspected duplicate without merging, and "different from" records a confirmed non-duplicate so nobody merges it again. The academic literature on entity linking and record linkage has converged on the same shape: a candidate generator built from an alias table with prior probabilities, a disambiguator that uses context and coherence with the other mentions in the same document, a NIL decision for "this is someone new", and a two-threshold rule that yields three outcomes: link, mint, or ask a human. FFS should adopt exactly that shape, with the atom store as the alias table, the quarantine as the "ask a human" bucket, and signed same-as / different-from atoms as the reversible merge mechanism.

---

## 1. Wikipedia and Wikidata identity mechanics

### 1.1 The title is not the identity

On Wikipedia, the page title is chosen by naming convention and can change. The convention for people is common usage: "The name used most often to refer to a person in reliable sources is generally the one that should be used as the article title" ([Wikipedia:Naming conventions (people)](https://en.wikipedia.org/wiki/Wikipedia:Naming_conventions_(people))). When two people share a name, the title gets a parenthetical: "If there is no usual form of conventional disambiguation, place a disambiguating tag in parentheses after the name", usually an occupation ("(musician)", "(politician)"), and "Disambiguating by vital year may be necessary when there are multiple people with the same name and the same specific disambiguation qualifier" (same page). The disambiguator "is usually a noun indicating what the person is noted for being in their own right".

The general rule for the parenthetical is "the generic class ... that includes the topic, as in Mercury (element), Seal (emblem); or the subject or context to which the topic applies" ([Wikipedia:Disambiguation](https://en.wikipedia.org/wiki/Wikipedia:Disambiguation)). Disambiguation pages exist because "a potential article title is ambiguous, most often because it refers to more than one subject covered by Wikipedia". A **primary topic** takes the bare title only when it is "much more likely than any other single topic, and more likely than all the other topics combined" to be what a reader wants; "first comes to mind" is explicitly rejected as a criterion. Hatnotes at the top of an article point at the alternatives.

The stable identity lives in Wikidata. Each item "has a unique identifier (starting with a Q prefix)" assigned sequentially ([Help:Items](https://www.wikidata.org/wiki/Help:Items)); the glossary defines the QID as "the unique identifier of a data item on Wikidata, comprising the letter 'Q' followed by one or more digits" ([Wikidata:Glossary](https://www.wikidata.org/wiki/Wikidata:Glossary)). Labels are language-specific and "need not be unique"; descriptions are "designed to disambiguate the page in question from other pages with the same or similar labels". The QID carries no meaning and does not change when the label or the Wikipedia title changes. Third-party summaries state that a QID "never changes once assigned" ([PID4NFDI cookbook](https://pid4nfdi-training.readthedocs.io/en/latest/wikidata.html)); whether a *deleted* QID can ever be reassigned was not stated on any page fetched **(unverified)**, but see the merge rule below, which forbids reuse of merged ones.

### 1.2 Aliases are first-class and non-unique

"There can be as many aliases for an item as necessary", and crucially "Multiple items can have the same alias, so long as they have different descriptions" ([Help:Aliases](https://www.wikidata.org/wiki/Help:Aliases)). Search considers "labels and aliases in all languages", is case-insensitive, and the guidance is to include full names for people known by nicknames, alternative spellings and transliterations, and ASCII forms. This is the alias table the entity-linking literature depends on, maintained by humans as data rather than by code.

### 1.3 Merges redirect, never delete; undo restores both sides

On Wikipedia, a merge copies content into the destination and then replaces the source with `#REDIRECT [[DESTINATION ARTICLE]]`; the source's "complete edit history is maintained" and editors must attribute by "including a wikilink to the source article in your edit summaries" ([Wikipedia:Merging](https://en.wikipedia.org/wiki/Wikipedia:Merging)). Redirects are kept "To retain edit history for attributing copied or merged content" and because "Deleting redirects runs the risk of breaking incoming or internal links" ([Wikipedia:Redirect](https://en.wikipedia.org/wiki/Wikipedia:Redirect)). True history merging (interleaving two revision histories) is rare and only possible "if they have not been edited in parallel".

On Wikidata, "A merge ... results in the redirection of the obsolete page to the recipient item" and the policy is blunt: "Merged items should be redirected. Never reuse merged items for other things" ([Help:Merge](https://www.wikidata.org/wiki/Help:Merge)). The redirect is exposed in RDF as `wd:Q18511155 owl:sameAs wd:Q9404406`, and a bot (KrBot) rewrites statements that still point at the old id "within a few days" ([Help:Redirects](https://www.wikidata.org/wiki/Help:Redirects)). Undo is a two-step restore: restore the recipient's pre-merge revision, then the merged item's; "The order is important (especially if the item has sitelinks) because articles can be linked to at most one item" (Help:Merge). If bots have already rewritten references, their operators must be asked to reverse them. Lesson: the cost of a wrong merge grows with time, so the system should make merges cheap to *record* and cheap to *reverse*, and should delay irreversible consequences.

### 1.4 Two assertions bracket the automation

- **P460 "said to be the same as"**: "this item is said to be the same as that item, though this may be uncertain or disputed" ([Property:P460](https://www.wikidata.org/wiki/Property:P460)). It documents a suspected identity *without* merging, symmetric, for cases where "identity claims are uncertain or disputed".
- **P1889 "different from"**: "Item that is different from another item, with which it may be confused" ([Property:P1889](https://www.wikidata.org/wiki/Property:P1889)). Symmetric, with a "criterion used" qualifier (date of birth, profession, country). Its practical job is to stop the next editor or bot from merging two people named Charles Pratt.

Together these give a three-state human vocabulary: same (merge), maybe-same (P460), definitely-not-same (P1889). The last one is the human override the OKF convention calls "do not repeatedly restore what a human rejected".

### 1.5 Notability is a NIL policy

Wikipedia will not create a page for everyone mentioned in the news. People are presumed notable only with "significant coverage in multiple published secondary sources that are reliable, intellectually independent of each other, and independent of the subject", and for people known for one event "The general rule is to cover the event, not the person" ([Wikipedia:Notability (people)](https://en.wikipedia.org/wiki/Wikipedia:Notability_(people))). In entity-linking terms this is a NIL policy with a threshold on evidence: a single mention does not mint an identity page; the event gets the page and the person is a mention inside it. FFS's `person.generic` versus `contact.person` split, and task_41's "mentioned in at least N articles" promotion rule, are the same instinct.

---

## 2. Entity linking: the academic lineage

The task is to map a mention in text ("Chen", "Sara Chen", "the Acme CEO") to an entry in a knowledge base, or to decide there is none (NIL). The Wikipedia-era papers built the standard pipeline; the neural era replaced the components but kept the shape.

| Year | Work | Contribution to the pipeline |
|---|---|---|
| 2006 | Bunescu and Pasca, "Using Encyclopedic Knowledge for Named Entity Disambiguation", EACL, pp. 9-16 ([ACL Anthology](https://aclanthology.org/E06-1002/)) | Dictionary of surface forms built from Wikipedia titles, redirect pages, and disambiguation pages; an SVM kernel over the article text; an explicit "out of Wikipedia" class for mentions with no entry. Reported accuracy figures **(unverified in this session; PDF not text-extractable)**. |
| 2007 | Mihalcea and Csomai, "Wikify!", CIKM, pp. 233-241 ([paper](https://web.eecs.umich.edu/~mihalcea/papers/mihalcea.cikm07.pdf)) | Two steps: detect which phrases deserve a link, then disambiguate using Wikipedia anchor-text statistics; the "most frequent sense" (most common link target for a surface form) is the baseline everything else must beat. |
| 2007 | Cucerzan, "Large-Scale Named Entity Disambiguation Based on Wikipedia Data", EMNLP-CoNLL, pp. 708-716 ([ACL Anthology](https://aclanthology.org/D07-1074/)) | Candidates from titles, redirects, disambiguation pages, and anchor text; disambiguation by maximizing agreement between the document's context and the candidates' categories, for all mentions in a document jointly. Accuracy numbers **(unverified; PDF not extractable)**. |
| 2008 | Milne and Witten, "Learning to link with Wikipedia", CIKM ([ACM](https://dl.acm.org/doi/10.1145/1458082.1458150)) | Two features that became standard names: **commonness** (the prior probability p(entity given surface form) from anchor text) and **relatedness** (link-overlap similarity between candidate and the unambiguous context entities). "recall and precision of almost 75%" on both Wikipedia and real-world documents. |
| 2009 onward | TAC-KBP entity linking, Ji and Grishman et al. ([2010 overview](https://blender.cs.illinois.edu/paper/kbp2010overview.pdf), [2011 overview](https://blender.cs.illinois.edu/paper/kbp2011.pdf), [2014 EDL](https://www.semanticscholar.org/paper/Overview-of-TAC-KBP2014-Entity-Discovery-and-Tasks-Ji-Nothman/9ebb12faa38a9c7e2a43da932abe907a92933000)) | Formalized **NIL**: a mention with no KB entry must be labeled NIL, and from 2011 all NIL mentions must be **clustered** so that two NIL mentions of the same unknown person get the same cluster id. Evaluated with B-cubed and B-cubed+. Best system accuracies for 2011 were not extractable from the PDF in this session **(unverified)**. |
| 2011 | Hoffart et al., AIDA, "Robust Disambiguation of Named Entities in Text", EMNLP, pp. 782-792 ([ACL Anthology](https://aclanthology.org/D11-1072.pdf)) | Combines three signals, "the prior probability of an entity being mentioned, the similarity between the contexts of a mention and a candidate entity, as well as the coherence among candidate entities for all mentions together", solved as a dense-subgraph problem. Introduced the CoNLL-YAGO benchmark; about 82% accuracy on it per the fetched summary, with coherence the signal that mattered most. |
| 2020 | Wu et al., BLINK, EMNLP ([arXiv 1911.03814](https://arxiv.org/abs/1911.03814)) | Two stages: a bi-encoder that "independently embeds the mention context and the entity descriptions" for retrieval, then a cross-encoder that "concatenates the mention and entity text" for reranking. Zero-shot: an entity is "defined only by a short textual description". "6 point absolute gains" on the zero-shot benchmark, state of the art on TACKBP-2010, and "linking with 5.9 million candidates in 2 milliseconds" for the retrieval stage. |
| 2021 | De Cao et al., GENRE, ICLR ([arXiv 2010.00904](https://arxiv.org/abs/2010.00904)) | Generates the entity *name* "token-by-token in an autoregressive fashion" under constrained decoding, so memory scales with vocabulary not entity count, and "new entities can be incorporated simply by specifying their names, without retraining". State of the art or competitive across more than 20 datasets. |

### 2.1 The pipeline every one of them implements

1. **Mention detection**: find the spans that name someone or something.
2. **Candidate generation**: look the surface form up in an alias table (titles, redirects, aliases, anchor text) and rank by prior p(entity given mention). Wikidata's alias list is exactly this table; Milne and Witten's "commonness" is exactly this prior.
3. **Disambiguation**: score candidates by context similarity (does the article talk about the things this candidate is known for?) and by coherence (do the other entities in this article fit together with this candidate?). AIDA showed coherence is the strongest signal; Cucerzan and Bhattacharya and Getoor (below) showed the same from the database side.
4. **NIL decision**: if no candidate clears a threshold, the mention is a new entity. TAC-KBP made this a first-class output rather than a failure.
5. **NIL clustering**: new entities still need identity across documents, so unlinked mentions are clustered with each other.

What changed in the neural era is only the scorer. BLINK replaced hand-built alias tables with dense retrieval over entity descriptions, and GENRE replaced retrieval with generation. Both still produce a ranked candidate list and still need a NIL threshold.

---

## 3. Record linkage and entity resolution: the database lineage

### 3.1 Fellegi and Sunter, 1969

The foundational probabilistic model, *Journal of the American Statistical Association* 64(328), pp. 1183-1210. For each pair of records, agreement on each field is weighed by two probabilities: the **m probability**, "the probability that an identifier in matching pairs will agree (or be sufficiently similar)", and the **u probability**, "the probability that an identifier in two non-matching records will agree purely by chance" ([Record linkage, Wikipedia](https://en.wikipedia.org/wiki/Record_linkage)). Splink explains the same model: weights are `log2(m/u)` per field and "match weights are additive", with a prior λ for the base rate ([Splink, Fellegi-Sunter](https://moj-analytical-services.github.io/splink/topic_guides/theory/fellegi_sunter.html)). The decision rule has **two thresholds and three outcomes**: "Record pairs with probabilities above a certain threshold are considered to be matches, while pairs with probabilities below another threshold are considered to be non-matches; pairs that fall between these two thresholds are considered to be 'possible matches'" and go to clerical review. That middle band is the single most important idea in this memo for FFS: the quarantine already exists, so the review band has somewhere to go.

### 3.2 Christen's five steps

Christen's *Data Matching* (Springer, 2012, [book](https://link.springer.com/book/10.1007/978-3-642-31164-2)) fixes the process vocabulary: "data pre-processing (cleaning and standardisation), indexing, comparisons, record pair classification, and evaluation". **Indexing** is the database word for blocking: "Blocking attempts to restrict comparisons to just those records for which one or more particularly discriminating identifiers agree, which has the effect of increasing the positive predictive value (precision) at the expense of sensitivity (recall)" (Record linkage, Wikipedia). For a personal substrate of tens of thousands of entities, blocking on normalized surname plus family (people vs orgs) is enough.

### 3.3 Collective resolution

Bhattacharya and Getoor, "Collective entity resolution in relational data", ACM TKDD, 2007 ([ACM](https://dl.acm.org/doi/10.1145/1217299.1217304)): "collective entity resolution, in which entities for cooccurring references are determined jointly rather than independently, can improve entity resolution accuracy". Getoor and Machanavajjhala's VLDB 2012 tutorial ([PVLDB 5(12)](http://vldb.org/pvldb/vol5/p2018_lisegetoor_vldb2012.pdf)) frames the field as blocking, matching, and clustering or merging, and lists collective and relational resolution among the open challenges. Applied to a newspaper: "Chen" in an article that also mentions Acme and the Charlotte tech corridor is far more likely the Sara Chen already filed with `organization: Acme` than any other Chen. This is the database-side twin of AIDA's coherence.

### 3.4 Why cluster ids are separate from record ids

Every mature ER system keeps the identity of a *cluster* separate from the identity of each *record* in it, because merges and splits happen. Dedupe's output is a cluster id per record; Splink produces a cluster id per record after connected-components over pairwise links; Wikidata's redirect is a cluster pointer from an old id to a surviving one. If the record id *is* the display name (a slug), then a rename, a merge, or a split forces rewriting every reference. If the id is opaque and the name is data, none of those operations touch references.

### 3.5 Evaluation

Bagga and Baldwin, 1998, "Entity-Based Cross-Document Coreferencing Using the Vector Space Model", COLING-ACL ([Semantic Scholar](https://www.semanticscholar.org/paper/Entity-Based-Cross-Document-Coreferencing-Using-the-Bagga-Baldwin/759f0ec45e3b62254a9a3461240f47f5eaf21f7f)), introduced **B-cubed**: precision and recall computed per mention against its true cluster, then averaged. TAC-KBP adopted B-cubed and B-cubed+ for linking plus NIL clustering. Pairwise precision and recall over "same entity" pairs is the simpler complement. Both are computable from a golden corpus where each expected proposal carries the expected entity id.

---

## 4. Industry identifier practice

- **W3C Reconciliation Service API** (OpenRefine's protocol, [CG Final 0.2](https://www.w3.org/community/reports/reconciliation/CG-FINAL-specs-0.2-20230410/)): a query carries a `query` string, optional `type`, optional `properties` (pid, v) for context, and a `limit`; each candidate returns `id`, `name`, `description`, `type`, `score`, optional `features`, and a boolean `match`: "A boolean matching decision, which indicates whether the service considers this candidate good enough to be chosen as a correct match." That is the three-outcome rule expressed as an API: `match: true` is auto-link, an empty list is NIL, and candidates without `match` are the review band. The `properties` field is how context (organization, location) enters candidate scoring.
- **MusicBrainz MBID**: "When an entity is merged into another, its MBIDs redirect to the other entity" ([MusicBrainz Identifier](https://musicbrainz.org/doc/MusicBrainz_Identifier)). Reuse policy not stated on that page **(unverified)**.
- **ORCID**: duplicates are resolved by marking one record primary and the other deprecated; "obsolete iDs are deprecated rather than completely deleted, with the deprecated iD pointing to the primary record", and API calls to a deprecated iD return a 301 ([ORCID support, via search summary](https://support.orcid.org/hc/en-us/articles/360006896634-I-have-more-than-one-ORCID-iD)). Note the loss: "all information and permissions from the duplicate record are deleted and cannot be transferred", and "Once a duplicate record has been removed, it cannot be reinstated". ORCID is the cautionary case: a redirect without history preservation makes a wrong merge unrecoverable.
- **Freebase MIDs**: when Google closed Freebase (announced December 2014, migration paper 2016, [ACM](https://dl.acm.org/doi/10.1145/2872427.2874809)), the mapping survived only because Wikidata stores the old id as property P646 on each item; the freebase.com URLs themselves stopped redirecting after August 2016 per the Wikidata project page ([WikiProject Freebase](https://www.wikidata.org/wiki/Wikidata:WikiProject_Freebase)). Lesson: an external identifier is only as durable as the record that carries it; store cross-references as data on the surviving entity.
- **Obsidian aliases**: aliases are frontmatter, "always be formatted as a list in YAML" (`aliases: [Doggo, Woofer]`), and when you link through an alias "Obsidian uses the `[[Artificial Intelligence|AI]]` link format to ensure interoperability" rather than `[[AI]]` alone ([Obsidian help: Aliases](https://obsidian.md/help/aliases)). So the wikilink target is the note's file name, the alias is display text. For FFS this means a projection's *file name* is the link identity inside the vault, and aliases in the projection frontmatter make search and link-completion find the file by any name.

---

## 5. Human in the loop

- Wikidata's undo is a two-sided restore in a specific order (section 1.3), and P1889 "different from" is the memory that prevents the same wrong merge twice (section 1.4).
- **Active learning** reduces reviewer effort by asking the human only about the pairs the classifier is least sure of. Sarawagi and Bhamidipaty, "Interactive deduplication using active learning", KDD 2002, pp. 269-278 ([ACM](https://dl.acm.org/doi/10.1145/775047.775087)): the ALIAS system "uses active learning to interactively choose pairs to be labeled and added to the training set", treating deduplication as a two-class classification. Dedupe ([GitHub](https://github.com/dedupeio/dedupe)) productized this: "dedupe takes in human training data and comes up with the best rules for your dataset"; its docs describe labeling uncertain pairs until the classifier stabilizes (docs.dedupe.io returned 403 in this session; the labeling-count claim is **unverified**).
- The practical lesson for a personal system: every review decision the owner makes in the quarantine is a labeled pair. Store it (as a same-as or different-from atom) and the resolver gets better without any model training, because those atoms feed the alias table and the block list directly.

---

## 6. Design implications for FFS

FFS already has most of the pieces: atoms about an entity id, supersession chains that preserve history, bitemporal validity, provenance on every claim, a quarantine that is the human gate, `aliases[]` planned on `person.generic` v2 (ADR-028), daemon-side resolution with `resolution: existing | new` (task_36 as amended), the additive-versus-conflict rule (ADR-029), and wikilinks that resolve by file basename. What follows maps the findings onto those pieces and says which task owns each.

### 6.1 Identity: opaque entity ids, names as data (Phase 1, task_38 + task_36)

Wikidata's lesson is that the identifier must be meaningless and permanent, and the human-readable name must be data that can change. Today an FFS entity id is a slug derived from the display name (task_32), which is Wikipedia's *title*, not Wikidata's *QID*. That works until the first rename, merge, or two people with the same name.

Recommendation:

- Mint entity ids as opaque, stable strings (a short random id or a hash of the first proposal's content plus a nonce; the exact form is a task_38 decision) for every new entity created by the scribe or the MCP path. Never derive them from the name.
- Keep the projection *file name* human-readable: `people/by-name/S/Sara_Chen.md`. The materializer maps entity id to path through `path_for_entity` using the head atom's `display_name`; the mapping is a projection concern, not an identity concern. A rename supersedes `display_name`, the materializer moves the file, and the old basename is left as a one-line redirect note ("Moved to [[Sara_Chen-Acme]]") so existing wikilinks keep resolving, which is exactly Wikipedia's redirect-on-move.
- Collisions get Wikipedia's parenthetical, applied to the *file name only*: `Sara_Chen_(Acme).md` and `Sara_Chen_(City_Council).md`, with the qualifier taken from `organization` first, then `role`, then a year. ADR-028's "family token suffix" rule should be replaced by this, because a qualifier a human recognizes beats a token nobody does.
- Put `aliases` into every projection's frontmatter so Obsidian's link completion and search find the file by any name, and emit links as `[[Sara_Chen_(Acme)|Sara Chen]]` so the display text can be the natural name while the target is the unique basename.

### 6.2 The alias table and priors come from the substrate's own history (Phase 1, task_36 + task_40)

Wikidata maintains aliases by hand; Milne and Witten got priors from anchor text. FFS has both for free:

- **Aliases**: `display_name`, `aliases[]`, and every surface form that has ever been resolved to this entity by an accepted proposal. When the owner accepts a proposal whose mention text was "S. Chen" and whose resolved entity is Sara Chen, append "S. Chen" to her aliases (an additive edit, so ADR-029 can auto-file it).
- **Prior**: for a surface form that maps to several entities, p(entity given form) is the count of accepted resolutions to each, from the atom store. This is "commonness". A new substrate has no counts; fall back to "most recently mentioned" and then to review.
- **`entity.search` v2** (task_40) is the candidate generator. Its `matched_on` field should distinguish `display_name`, `alias`, and `other`, and its ordering should be exact name, then alias, then prior count. This is also the W3C reconciliation shape; consider exposing it as such over MCP later so any reconciliation-aware tool (OpenRefine included) can talk to a substrate.

### 6.3 Disambiguation uses article context and coherence (Phase 1 minimal, later full)

- **Blocking**: candidates come only from the same family (people vs orgs) and from a normalized surname or organization-name key. Nothing else is compared.
- **Context features** (Phase 1, task_36's daemon-side resolver): agreement between the candidate's head atoms and the proposal's fields for `organization`, `role`, `location`. Fellegi-Sunter weights, hand-set to start: organization agreement is strong evidence (high m, low u), a shared surname alone is weak (high u for common names). Store the weights in a TOML next to the predicate specs so they are tunable without code.
- **Coherence** (later): if the same article also proposes an org that the candidate person is already affiliated with, raise that candidate. This is AIDA's coherence and Bhattacharya and Getoor's collective resolution, and it needs the multi-entity proposal envelope from task_36 as amended so the resolver sees all mentions in one submission together. Ship the hook (resolver receives the whole envelope) in Phase 1; ship the scoring later.
- **Dense retrieval** (later, opt-in): when the `llm` engine is enabled (ADR-026), a BLINK-style pass can rerank the top candidates by comparing the article context with each candidate's rendered projection. Keep it behind the same opt-in as the extraction engine; the heuristic resolver must stand alone.

### 6.4 Three outcomes, two thresholds, and how ADR-029 consumes them (Phase 1, task_36 + task_39)

Adopt Fellegi-Sunter's decision rule literally. The resolver emits, per mention, a best candidate with a score and a `resolution` value:

| Score | `resolution` | What happens |
|---|---|---|
| above the upper threshold | `existing` (with entity id) | Proposal targets that entity. ADR-029's additive rule decides whether it auto-files. |
| below the lower threshold, no plausible candidate | `new` | Proposal mints an entity. Auto-file is allowed if the grant covers the predicate (this is the "cover the event, not the person" question; see 6.6). |
| between the thresholds, or two candidates within a margin of each other | `ambiguous` (with the candidate list) | Always routes to review, regardless of any Accept grant. The review card shows the candidates as a reconciliation picker: "Is this Sara Chen (Acme), Sara Chen (City Council), or someone new?" |

task_36's `resolution` field therefore needs the third value, `ambiguous`, plus `candidates: [{entity, score, matched_on}]`, and ADR-029's classifier must treat `ambiguous` as conflicting. Thresholds start conservative (wide review band) and narrow as the golden corpus scores justify; the values live in the same TOML as the weights.

### 6.5 Merge and split as signed, reversible atoms (Phase 1 shape, Phase 2 UI)

Wikidata's redirect and Wikipedia's redirect-on-merge become two predicates:

- **`entity.same_as`**: an atom whose entity is the *losing* id, with claim `{ target: <winning id>, reason, criterion }`, signed by whoever asserted it (the owner from the review UI, or an agent if a future grant allows). Semantics: readers and the materializer resolve the losing id to the winner; the losing entity's atoms stay in place and are shown under the winner's projection (history preserved, exactly Wikidata's "never delete, redirect"). Undo is supersession of the same-as atom with `valid_to: now`, which restores both sides with no data movement, so FFS's undo is simpler than Wikidata's two-step restore.
- **`entity.different_from`**: symmetric, `{ other: <id>, criterion }`, signed by the owner. The resolver consults it as a hard block: no candidate that is `different_from` the proposal's resolved context is ever auto-linked, and the review UI never re-suggests a pair the owner has separated (the human-override rule from the agent memory convention, ADR-027).
- A **split** is a new entity plus a set of supersessions moving specific atoms' `entity` to it, plus a `different_from`. Phase 2.
- "Said to be the same as" (P460) is not needed as a separate predicate: an `ambiguous` proposal sitting in the quarantine *is* the unresolved suspicion.

Both predicates are cheap to add under ADR-028 (they need no path family; they never render as files, only as a "Merged into" line on the losing projection). The resolver must follow `same_as` chains when generating candidates so a mention that matches a losing alias lands on the winner.

### 6.6 NIL policy: when to mint (Phase 1, task_36; refined in task_41)

Wikipedia's notability rule, translated: a person mentioned once, in passing, does not automatically get a file. Recommendation:

- Always create the `source.article` record and the event record; they are append-only and cheap.
- Mint a `person.generic` entity on first mention only when the mention carries a distinguishing attribute (organization or role); a bare name with nothing else becomes a line in the article's `mentions` and a NIL cluster key (normalized name plus article org context) so the second sighting can find the first. This is TAC-KBP's NIL clustering at small scale.
- On the second qualifying sighting, mint and back-fill both mentions. task_41's promotion rule (person.generic to contact.person after N articles) is the next rung of the same ladder.

### 6.7 Evaluation (Phase 1, task_36's corpus)

Every golden-corpus fixture already carries expected proposals. Add an expected entity id (or "new") per proposal and compute pairwise precision and recall on same-entity pairs plus B-cubed over the resolved clusters, for the heuristic resolver in CI and for the LLM-assisted resolver out of band. Include fixtures for: the same person across three articles with a nickname, two different people with the same name at different orgs, a rename (maiden to married name announced in an article), and a wrong-merge undo.

### 6.8 What to record from every review decision (Phase 1, task_39)

Each accept in the quarantine that resolved an `ambiguous` proposal produces two things: the accepted atom(s) and, when the owner picked a candidate over others, an implicit `different_from` between the chosen entity and the rejected candidates for that surface form is *not* safe to infer (they may be the same person seen from a different angle). Record only the positive: append the mention text to the chosen entity's aliases. Record `different_from` only when the owner clicks "these are different people". This keeps the human-override memory honest.

---

## 7. Phase summary

| Item | Owner | Phase |
|---|---|---|
| Opaque entity ids; human file names with parenthetical disambiguation and redirect stubs on rename | task_38 (identity + materializer), task_36 (minting) | 1 |
| `aliases` in projection frontmatter; `[[target\|display]]` link form | task_38 | 1 |
| `entity.search` v2 as candidate generator with `matched_on` and prior ordering; alias growth on accept | task_40, task_39 | 1 |
| Resolver with blocking, hand-set Fellegi-Sunter weights, two thresholds, `resolution: existing / new / ambiguous` with candidates; whole-envelope input | task_36 | 1 |
| `ambiguous` always routes to review; reconciliation picker card | task_39, plugin | 1 |
| `entity.same_as` and `entity.different_from` predicates; resolver honors both; merge from the review UI; undo by supersession | task_38 (specs), task_39 (UI), small ADR | 1 (specs), 2 (UI) |
| NIL policy: mint on distinguishing attribute or second sighting; NIL cluster key | task_36, task_41 | 1 |
| Corpus fixtures with expected entity ids; pairwise P/R and B-cubed scorer | task_36 | 1 |
| Coherence scoring across an article's mentions | task_36 follow-up | 2 |
| LLM or dense reranking of candidates (opt-in) | ADR-026 engine | 2 |
| Split operation and UI | later | 2 |
| Expose `entity.search` as a W3C Reconciliation endpoint over MCP | later | 3 |

---

## 8. Sources fetched or searched in this session

- Wikidata: [Help:Merge](https://www.wikidata.org/wiki/Help:Merge), [Help:Aliases](https://www.wikidata.org/wiki/Help:Aliases), [Help:Items](https://www.wikidata.org/wiki/Help:Items), [Help:Redirects](https://www.wikidata.org/wiki/Help:Redirects), [Wikidata:Glossary](https://www.wikidata.org/wiki/Wikidata:Glossary), [Property:P460](https://www.wikidata.org/wiki/Property:P460), [Property:P1889](https://www.wikidata.org/wiki/Property:P1889), [WikiProject Freebase](https://www.wikidata.org/wiki/Wikidata:WikiProject_Freebase)
- Wikipedia policy: [Redirect](https://en.wikipedia.org/wiki/Wikipedia:Redirect), [Merging](https://en.wikipedia.org/wiki/Wikipedia:Merging), [Disambiguation](https://en.wikipedia.org/wiki/Wikipedia:Disambiguation), [Naming conventions (people)](https://en.wikipedia.org/wiki/Wikipedia:Naming_conventions_(people)), [Notability (people)](https://en.wikipedia.org/wiki/Wikipedia:Notability_(people)), [Record linkage](https://en.wikipedia.org/wiki/Record_linkage)
- Papers: Bunescu and Pasca 2006 ([ACL](https://aclanthology.org/E06-1002/)); Mihalcea and Csomai 2007 ([PDF](https://web.eecs.umich.edu/~mihalcea/papers/mihalcea.cikm07.pdf)); Cucerzan 2007 ([ACL](https://aclanthology.org/D07-1074/)); Milne and Witten 2008 ([ACM](https://dl.acm.org/doi/10.1145/1458082.1458150)); Ji et al., TAC-KBP overviews ([2010](https://blender.cs.illinois.edu/paper/kbp2010overview.pdf), [2011](https://blender.cs.illinois.edu/paper/kbp2011.pdf)); Hoffart et al. 2011 ([ACL](https://aclanthology.org/D11-1072.pdf)); Wu et al. 2020 BLINK ([arXiv](https://arxiv.org/abs/1911.03814)); De Cao et al. 2021 GENRE ([arXiv](https://arxiv.org/abs/2010.00904)); Bhattacharya and Getoor 2007 ([ACM](https://dl.acm.org/doi/10.1145/1217299.1217304)); Getoor and Machanavajjhala 2012 ([PVLDB](http://vldb.org/pvldb/vol5/p2018_lisegetoor_vldb2012.pdf)); Christen 2012 ([Springer](https://link.springer.com/book/10.1007/978-3-642-31164-2)); Bagga and Baldwin 1998 ([Semantic Scholar](https://www.semanticscholar.org/paper/Entity-Based-Cross-Document-Coreferencing-Using-the-Bagga-Baldwin/759f0ec45e3b62254a9a3461240f47f5eaf21f7f)); Sarawagi and Bhamidipaty 2002 ([ACM](https://dl.acm.org/doi/10.1145/775047.775087))
- Tools and identifiers: [W3C Reconciliation Service API 0.2](https://www.w3.org/community/reports/reconciliation/CG-FINAL-specs-0.2-20230410/), [Splink Fellegi-Sunter guide](https://moj-analytical-services.github.io/splink/topic_guides/theory/fellegi_sunter.html), [Dedupe](https://github.com/dedupeio/dedupe), [MusicBrainz Identifier](https://musicbrainz.org/doc/MusicBrainz_Identifier), [ORCID duplicate iDs](https://support.orcid.org/hc/en-us/articles/360006896634-I-have-more-than-one-ORCID-iD), [Freebase to Wikidata migration](https://dl.acm.org/doi/10.1145/2872427.2874809), [Obsidian aliases](https://obsidian.md/help/aliases)
