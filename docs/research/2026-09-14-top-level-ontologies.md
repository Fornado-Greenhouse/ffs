# Top-level ontologies and the FFS business graph

Research memo, 2026-09-14. Companion to the entity-resolution memo (track 1) for the question "how does the same person across fifty articles stay one file". This track covers ISO/IEC 21838, Basic Formal Ontology (BFO 2020), TUpper, DOLCE, and the Information Artifact Ontology (IAO), and then maps them onto FFS's predicate design for ADR-028, task_36, task_38, task_39 and task_41.

Verification status is marked per claim. "Verified" means the statement was read in the cited primary source during this research pass. "Unverified" means it comes from a secondary summary (search snippets, a vendor page) and the primary text could not be fetched (the ISO catalogue, SAGE and ACM pages returned 403).

---

## 1. ISO/IEC 21838: the top-level ontology standard

### 1.1 The four parts

| Part | Title | Edition | Status |
|---|---|---|---|
| 21838-1 | Top-level ontologies (TLO), Part 1: Requirements | First edition 2021-08 | Published (verified from the standard's preview PDF) |
| 21838-2 | Part 2: Basic Formal Ontology (BFO) | 2021 on the ISO catalogue and in the CCO paper's reference list; the BFO GitHub README says "ISO/IEC 21838-2:2020" | Published. The year discrepancy is in the sources themselves; treat "2020/2021" as the same first edition |
| 21838-3 | Part 3: Descriptive ontology for linguistic and cognitive engineering (DOLCE) | 2023 | Published (unverified beyond catalogue listings) |
| 21838-4 | Part 4: TUpper | First edition 2023-09 | Published (verified from the standard's preview PDF) |

All four are the work of ISO/IEC JTC 1/SC 32, Data management and interchange (verified, Foreword of Parts 1 and 4).

### 1.2 What Part 1 requires of a top-level ontology

Part 1's scope (verified, §1): it "specifies required characteristics of a domain-neutral top-level ontology (TLO) that can be used in tandem with domain ontologies at lower levels to support data exchange, retrieval, discovery, integration and analysis." Out of scope: the ontology languages themselves, reasoning methods, translators, IRI policy, versioning, and "how ontologies can be used in the tagging or annotation of data."

Key definitions (verified, §3):

- 3.19 category: "general class or type that is shared across many different domains and is represented by a domain-neutral term." Examples given: process, attribute, event, region, information entity.
- 3.20 top-level ontology: "ontology that is created to represent the categories that are shared across a maximally broad range of domains."
- 3.23 ontology conformance: "relation between two ontologies when one consistently extends the other." Note 1 makes this precise: any element in a model of the extending ontology that satisfies the conditions for being an instance of a class in the starting ontology must be an instance of that class in the extending ontology. In other words, a domain ontology conforms to a TLO by subclassing its categories without contradicting them.

The requirements (verified, §4 table of contents and §4.1 text; §4.2 onward from the preview and the standard's own introduction):

1. §4.1 The TLO as a textual artefact: a natural-language document listing domain-neutral terms and relational expressions, identifying which are primitive, and giving definitions for all the rest that are (a) non-circular, (b) a consistent set, and (c) concise. Consistency is to be shown by an axiomatization proven consistent.
2. §4.2 An axiomatization in OWL 2 with direct semantics (or a W3C-designated successor description logic), with the same signature as the textual artefact.
3. §4.3 An axiomatization in a Common Logic conforming language (ISO/IEC 24707). The introduction explains why both: CL has full first-order expressivity, needed for mereology, location and change; OWL 2 is decidable and so usable "by computer systems for purposes of logical reasoning and ontology quality assurance."
4. §4.4 Supplementary documentation: purpose (4.4.2), a demonstration of how a domain ontology conforms to the TLO (4.4.3), consistency of the CL axiomatization (4.4.4), the relation between the OWL and CL axiomatizations (4.4.5), breadth of coverage (4.4.6, with Annex C examples), domain neutrality (4.4.7), and ontology management (4.4.8).
5. §5 Conformity: an ontology claims conformity by supplying the above; Annex D covers conformance of a domain ontology to a TLO.

What this means for a domain project like FFS: conformance is a relation between ontologies, not between data and an ontology. FFS would "conform" only if it published its predicate vocabulary as an ontology that consistently extends a TLO. Nothing in Part 1 requires a data system to do that.

---

## 2. BFO 2020

### 2.1 The categories

BFO is "a small, upper level ontology that is designed for use in supporting information retrieval, analysis and integration in scientific and other domains" (verified, bfo-ontology.github.io). Artifacts: `bfo-core.owl` under `21838-2/owl/`, CLIF axioms organised by sub-theory, Prover9 files, a first-order axiomatization PDF, and SPARQL design checks, all CC BY 4.0 (verified, GitHub README).

Definitions, quoted from `bfo-core.ttl` in the BFO-2020 repository (verified):

- continuant: "an entity that persists, endures, or continues to exist through time while maintaining its identity."
- occurrent: "an entity that unfolds itself in time or it is the start or end of such an entity or it is a temporal or spatiotemporal region."
- independent continuant: a continuant that does not specifically or generically depend on anything else.
- material entity: an independent continuant that always has some portion of matter as part.
- object: "a material entity which manifests causal unity & is of a type instances of which are maximal relative to the sort of causal unity manifested." A person is the canonical object.
- object aggregate: "a material entity consisting exactly of a plurality (>= 1) of objects as member parts which together form a unit."
- specifically dependent continuant: a continuant that specifically depends on some independent continuant that is not a spatial region.
- quality: a specifically dependent continuant that "does not require any further process in order to be realized."
- realizable entity: a specifically dependent continuant "of a type some instances of which are realized in processes of a correlated type."
- role: "a realizable entity such that b exists because there is some single bearer that is in some special physical, social, or institutional set of circumstances in which this bearer does not have to be & b is not such that, if it ceases to exist, then the physical make-up of the bearer is thereby changed."
- disposition: a realizable entity whose loss changes the bearer physically and whose realization occurs "in virtue of the bearer's physical make-up." Function is a disposition that exists through evolution or intentional design.
- generically dependent continuant: "an entity that exists in virtue of the fact that there is at least one of what may be multiple copies which is the content or the pattern that multiple copies would share."
- process: an occurrent with temporal proper parts and, at some time, a material entity as participant.
- process boundary, temporal region, spatial region, site, history: as defined in the file; history is the unique history of a material entity.

Relations (verified, same file): inheres in / bearer of; realizes / has realization; participates in / has participant; concretizes / is concretized by; part of; located in; occupies temporal region; exists at; has history.

The role definition is the load-bearing one for this memo. Three consequences follow directly from it: a role has exactly one bearer; it is "externally grounded", so the bearer can gain or lose it without changing physically (the CCO paper's example is being a student, verified); and it exists only while the circumstances hold, which is why roles have a temporal extent.

### 2.2 Organizations and roles in BFO's mid-level: the Common Core Ontologies

BFO itself has no Person or Organization class. The Common Core Ontologies (CCO) supply them as a mid-level suite of eleven ontologies extending BFO, BSD-3 licensed, endorsed in 2024 as a baseline standard across the US Department of Defense and Intelligence Community, and under IEEE P3195 review as the first standard mid-level ontology (verified, Jensen et al. 2024 and the CUBRC 2019 overview).

What CCO gives us (verified, CUBRC 2019 overview §4.1, §4.2, §4.4, and Jensen et al. 2024):

- Agent Ontology: "The class AGENT comprises both individual agents (PERSON) and coordinated groups of individuals (ORGANIZATION)." Agents bear roles such as CITIZEN ROLE, OCCUPATION ROLE and ALLY ROLE. Figure 5 of the overview shows the exact pattern we need: `person 1 has role doctor role 1`, and `doctor role 1 has organizational context healthcare organization 1`. The role inheres in the person; the organization is the role's context, not its bearer.
- The one-line definition of Organization as "an agent that is a social entity with a collective goal" appears in search summaries but was not read in the OWL file (unverified).
- Event Ontology: processes with participants; `agent in` is a sub-relation of `participates in` for causally relevant agents (verified, Jensen et al. §2.4 footnote and overview §4.4).
- Time handling of roles: because OWL properties are binary and atemporal, CCO represents the gain of a role as a CHANGE process (`GAIN OF ROLE`) occurring on a temporal interval, and the holding of a role as a STASIS process (`STASIS OF ROLE`). The overview's Figures 10 and 11 model Barack Obama gaining the president role on 20 January 2009 and bearing it for 8 years exactly this way (verified). The Industrial Ontologies Foundry adopts the same pattern, introducing Gain of Role and Loss of Role as subclasses of `bfo:process` so that "exact or relative time points" for a role starting or stopping can be expressed, with the bearer clear as a participant in the process (verified, IOF Confluence page).
- Jensen et al. note that CCO's newest release dropped BFO's temporalized object properties (`member part of at all times` and so on) and keeps only the core, untemporalized ones, precisely because temporal qualification was hard to deploy (verified, §2.5).

### 2.3 "Jane Doe is CEO of Acme from 2024", in BFO plus CCO

1. Jane Doe: a `Person`, a BFO `object`.
2. Acme: an `Organization`, a CCO agent (a group of persons with a collective goal), which under BFO is best read as an object aggregate that bears social roles.
3. The CEO position: a `role` (an `Occupation Role` in CCO) that inheres in Jane. It exists because of an institutional circumstance (Acme's appointment) and Jane could lose it without changing physically.
4. Acme is the role's organizational context (CCO `has organizational context`).
5. The role is realized in processes (running the company), which is why a news article reporting an action of Jane's as CEO is evidence for the role.
6. The role exists at a temporal region beginning in 2024. In pure BFO this is `exists at`; in CCO/IOF practice it is a Gain of Role process on 2024 and, later, a Loss of Role process.
7. The Charlotte Business Journal article is an information content entity that is about Jane, about Acme, and about the appointment process; it is concretized by the HTML page you fetched.

---

## 3. TUpper (ISO/IEC 21838-4:2023)

Verified from the standard's own introduction (preview PDF):

> "TUpper is a top-level ontology (TLO) conforming to ISO/IEC 21838-1. It contains definitions of its terms and relational expressions and formal representations in OWL 2 and in Common Logic (CL)."

> "The TUpper ontology follows an alternative approach (referred to as the sideways approach) to the conventional top-level ontology paradigm. Rather than think of a top-level ontology as a monolithic axiomatization centred on a taxonomy, the sideways approach considers a top-level ontology to be a modular ontology composed of ontologies that cover concepts including those related to time, process, and space, from which any underlying taxonomy can be inferred."

Its modules come from existing ISO standards: the Process Specification Language (PSL, ISO 18629), mereotopology and location (ISO 19107 and ISO 19150-1), and units of measure (ISO 80000). It ships as TUpper-terms (natural language), TUpper-OWL and TUpper-CL. §4.4.2 of the standard is titled "Modularity" and §4.9 demonstrates breadth of coverage across space and time, change over time, parts and wholes, processes and events, causality, information and reference, artefacts and socially constructed entities, and fiction (verified, table of contents).

Provenance: Michael Grüninger, Yi Ru and Jona Thai, "TUpper: A top level ontology within standards", Applied Ontology 17(1), 2022, pp. 143-165 (publication details verified via search listings; abstract page returned 403). The Common Logic modules live in COLORE, "a repository of first-order ontologies developed at the University of Toronto by Professor Michael Grüninger and his team", all specified in Common Logic (ISO 24707) (verified, Ontolog Forum COLORE page).

How it differs from BFO: BFO is taxonomy-first (continuant/occurrent at the root, everything is a subclass) and realist. TUpper is process-first and module-first. PSL's core notions are activities, activity occurrences, objects, and fluents that hold at states before and after occurrences; the taxonomy is a consequence of the axioms rather than the starting point. For a system that must reason about plans, precedence and what holds after an occurrence, TUpper is the stronger fit. For a system that must classify things and attach roles and documents to them, BFO with CCO has far more mid-level vocabulary.

---

## 4. DOLCE, briefly (ISO/IEC 21838-3:2023)

Verified from Borgo et al., "DOLCE: A Descriptive Ontology for Linguistic and Cognitive Engineering", Applied Ontology 2022 (arXiv 2308.01597):

- Four basic categories: endurant, perdurant, quality, abstract. "Endurants may acquire and lose properties and parts through time, perdurants are fixed in time."
- Stance: DOLCE "adopts a descriptive (rather than referentialist) metaphysics"; its categories are shaped by natural language, cognition and social practice. BFO's stance is realist (CCO inherits "Realism, Fallibilism, Adequatism", verified in Jensen et al. §2.1).
- Roles: "DOLCE does not formalize functions and roles." Instead, roles are social concepts that classify endurants at a time via the ternary relation CF(x, y, t), "at the time t, x is classified by the concept y". Roles are anti-rigid (an entity can stop being classified) and founded (they depend on an external context). Case 2 of the paper models "Mr. Potter is the teacher of class 2C" exactly this way: the teacher role exists throughout, is played by Potter at t1, by nobody at t2, by Bumblebee at t3.
- Axiomatized in quantified modal logic QS5, with CLIF and OWL approximations; ISO 21838-3 in 2023.

Why this matters for FFS: DOLCE's roles-as-time-indexed-classification and BFO's roles-as-dependent-continuants-with-a-temporal-region are two encodings of the same fact pattern. Both say the role is not a property stored on the person; it is a relationship between a person, a context, and a time. FFS's bitemporal atom can carry either reading. BFO is the better choice for FFS because the mid-level (CCO for agents, events, organizations; IAO for documents) already exists and is standardized, and because BFO's realism matches FFS's "records about the world, signed by whoever claims them" posture. DOLCE would be the choice if the substrate's primary job were modeling how people talk about things rather than what happened.

---

## 5. IAO: articles as information content entities

Verified from Ontobee and the IAO repository:

- information content entity (IAO_0000030): "A generically dependent continuant that is about some thing." Examples: "journal articles, data, graphical layouts, and graphs." Superclass: BFO generically dependent continuant, restricted by `is about some entity`.
- is about (IAO_0000136): "A (currently) primitive relation that relates an information artifact to an entity." Domain: information content entity. The editors deliberately weakened it to a primitive in 2009 and plan sub-properties.
- document (IAO_0000310): "A collection of information content entities intended to be understood together as a whole." Examples: "A journal article, patent application, laboratory notebook, or a book."

CCO's Information Entity Ontology refines the same idea (verified, CUBRC overview §4.1): information content entity `is about` entity, with three sub-relations, `describes` (reports, images; Figure 1 is literally "the content of a newspaper describing a weather event"), `prescribes` (plans) and `designates` (names and identifiers). The content is distinct from the information bearing entity that carries it; "A single information content entity can inhere in multiple information bearers." And the line most relevant to entity resolution: "A person only has one name, but that name (content) can be found in many particular physical tokens." A name is a designative information content entity that designates the person; it is not the person.

So a Charlotte Business Journal article about Jane Doe is: an IAO `document` (a generically dependent continuant), concretized by the HTML page and by your ingest markdown file (two bearers, one content), with a creation date and an author, that `describes` a hiring process and `is about` Jane and Acme. Jane's name in the byline or body `designates` Jane.

---

## 6. Design implications for FFS

The recurring lesson across all three top-level ontologies: a role is not an attribute of a person. It is its own particular with a bearer, a context and a temporal extent. An event is an occurrent with participants who play roles in it. A document is content, distinct from the file that carries it, that is about entities rather than about strings. FFS's atom envelope already provides what CCO and IOF have to bolt on with Gain of Role and Stasis processes: every atom has `valid_from`/`valid_to` (the temporal region) and a supersession chain (the history). We should use that, not re-encode it.

### (a) Affiliation: field on the person, or its own atom?

Recommendation: both, with the atom as the source of truth.

Introduce a predicate `affiliation` whose entity is the affiliation particular itself (the BFO role instance), not the person:

```toml
name = "affiliation"
[claim_schema.properties]
person       = { type = "string" }   # entity id of the bearer
organization = { type = "string" }   # entity id of the organizational context
title        = { type = "string" }   # "CEO", "board member"
kind         = { type = "string", enum = ["employee", "executive", "board", "founder", "investor", "advisor", "member", "other"] }
source       = { type = "string" }   # article url or ffs:// atom
notes        = { type = "array", items = { type = "string" } }
```

The atom's `valid_from` is when the role began, `valid_to` is when it ended (null while current), and a change of title is a supersession. Entity id: `<person-slug>__<org-slug>__<start-year>` so two stints at the same company are two particulars.

Why not only a field on the person:

- Head selection in FFS is unique per `(entity, predicate, as_of)`; multi-leaf states are flagged as conflicts. Two concurrent affiliations under one `person.generic` predicate would either overwrite each other or read as a conflict. Giving each affiliation its own entity makes concurrency the normal case, which is what BFO's "one bearer, many roles" implies.
- "Who was CEO of Acme in 2024" becomes `atom.list predicate=affiliation as_of=2024-06-01` filtered on `organization`; with a field on the person it is a scan over every person's superseded claims.
- ADR-029's additive/conflict rule falls out correctly with no special casing: a new affiliation atom is a new entity, so "Jane joined Acme" is additive and auto-files; "Jane left Acme" supersedes an existing atom to set `valid_to`, so it is conflicting and goes to review. This is exactly the human-gate behavior we want for role changes, and it is the ontology's distinction between gain and loss of role showing up as policy.

Why still keep `organization` on `person.generic` v2: rendering and wikilinks. The person file's frontmatter needs a current primary affiliation for the `[[Org]]` link and for fast-path edits, and the repo already has this split in `contact.person` (`organization` for the current primary, `organizations[]` for the history). Treat the person's `organization` field as a convenience that the scribe writes and the briefing may correct, never as the record of roles over time.

Costs to accept: the projection renderer needs a reverse lookup (affiliation atoms whose `person` or `organization` equals the entity being rendered) to fill an "Affiliations" section in person files and a "People" section in org files. That is a claim-field filter over `list_by_predicate`, cheap at personal scale. Fast-path edits to those sections cannot reverse-map to the affiliation atom through the person file; route them to ingest as corrections, as ADR-014 already does for ambiguous edits. No `affiliations/` folder is needed; the atoms render inside person and org files.

Where it lands: ADR-028 and task_38 (the predicate, the two template sections, the reverse lookup), task_36 (the scribe emits affiliation proposals alongside person and org proposals), task_41 (role changes are detected from affiliation supersession, not from person scalar supersession). Nothing changes in ADR-029.

### (b) event.business as an occurrent with participants

Yes. In BFO an event is a process with participants; CCO further distinguishes `agent in` (causally relevant) from `participates in`. A flat `parties[]` of names loses both the identity of the participants and their role in the event. Replace it with:

```toml
participants = { type = "array", items = { type = "object", properties = {
  entity  = { type = "string" },   # resolved entity id, absent when unresolved
  display = { type = "string" },   # the name as printed, always present
  role    = { type = "string", enum = ["acquirer", "target", "investor", "investee", "hire", "employer", "departing", "landlord", "tenant", "developer", "winner", "partner", "other"] }
}}}
```

The event's `date` is its temporal region; set `valid_from` = `valid_to` = `date` for a point event, and a range for a process with duration (a construction project). The `kind` enum in ADR-028 already names the process type. Change now, in ADR-028 and task_38, because it is the schema and the wikilinks depend on `entity`.

### (c) source.article as an information content entity

Yes, and it clarifies two decisions that are currently loose in ADR-028:

- `mentions` is an `is about` list. Store `{entity, display, context}` items, with `entity` the resolved id and `display` the printed name, the same shape as event participants. A name is a designator, not a referent; storing only names would make "everything about Jane" a string match forever. This is the same finding as track 1 from the other direction.
- The URL and the fetched page are information bearing entities that concretize the content. Record `url` (normalized, the dedup key from task_40), `content_hash` of the fetched page when the courier has it, and `published_at` as the content's creation date. Re-fetching the same URL is a new bearer of the same content and must resolve to the same article entity; that is task_40's dedup rule, now with an ontological reason.

Change now, in ADR-028 and task_40. The `summary` field stays an agent-authored descriptive information content entity about the article, which is why the convention's "mark inferences as inferences" rule applies to it.

### (d) Should predicate specs declare BFO alignment?

Recommendation: yes, as an optional, informative annotation, not a validated one.

```toml
[ontology]
bfo = "role"                       # continuant | occurrent | object | object aggregate | role | quality | information content entity | process
cco = "cco:OccupationRole"         # optional mid-level class
iao = ""                           # optional, for documents
note = "bearer = claim.person, context = claim.organization, temporal region = valid_from..valid_to"
```

What it buys:

1. Design discipline. Forcing each predicate to say whether it is a thing, a role, an event or a document is the single question that would have caught "organization as a scalar on the person" before it was written.
2. Federation vocabulary agreement. Two substrates that both say `affiliation` is a BFO role with bearer and context can map each other's predicates even if field names differ; ADR-007's bilateral federation has no vocabulary negotiation today.
3. A cheap path to RDF/OWL export later: each atom becomes a small graph (`entity rdf:type cco:Person`, role atom `bearer of` and `has organizational context`, article `is about`), which is what any external analytics or a future knowledge-graph tool would want.

What it costs: one optional TOML table and a documentation page. It must not gate validation, and it must not pull a reasoner into the daemon. Put the mapping for all predicates in `docs/ontology-alignment.md`, which is also where the `[ontology]` vocabulary is defined. ADR-028 gets a paragraph; task_38 gets the annotation on the starter specs.

### (e) What not to adopt

- No OWL reasoning in the substrate. ISO 21838-1's own introduction explains OWL 2 is there for decidable ontology QA; FFS's runtime questions are lookups, not subsumption.
- No class hierarchies. `predicate` is the type, flat. CCO's deep artifact and quality trees exist for cross-agency data integration; a personal substrate with four drawers gains nothing from them.
- No temporalized relations or Gain/Loss/Stasis process atoms. Those exist because OWL properties are binary and atemporal. FFS atoms carry `valid_from`/`valid_to` and a supersession chain natively; the atom is the temporal region.
- No requirement that a role be realized in a recorded process. BFO says roles are of types some instances of which are realized; we do not need to store the realization to store the role.
- No attempt at full 21838-1 conformance for FFS's vocabulary. Conformance is a relation between ontologies with dual OWL/CL axiomatizations and proofs of consistency. Alignment annotations give the interoperability benefit at a small fraction of the cost.

### Summary of what changes where

| Change | Where | When |
|---|---|---|
| `affiliation` predicate with person, organization, title, kind, bitemporal window; person and org templates render it via reverse lookup | ADR-028, task_38 | Now |
| `event.business.participants[]` with entity, display, role in event | ADR-028, task_38 | Now |
| `source.article.mentions[]` as `{entity, display, context}`; `content_hash`; url as bearer | ADR-028, task_38, task_40 | Now |
| Scribe emits affiliation proposals; resolution attaches entity ids into mentions and participants | task_36 | Now |
| Briefing detects role changes from affiliation supersession | task_41 | With task_41 |
| Optional `[ontology]` annotation and `docs/ontology-alignment.md` | ADR-028, task_38 | Now, cheap |
| Federation predicate mapping via alignment annotations | new ADR | Later |
| RDF/OWL export | new task | Later |

---

## Sources

Primary, read during this pass:

- ISO/IEC 21838-1:2021 preview PDF (cover, contents, introduction, §1 to §4.2): https://cdn.standards.iteh.ai/samples/71954/7ce56ea22dfd4b63a7424f38e8382988/ISO-IEC-21838-1-2021.pdf
- ISO/IEC 21838-4:2023 preview PDF (cover, contents, introduction): https://cdn.standards.iteh.ai/samples/78928/ce6e82a2cffb4cea89ee6bcfde641fcf/ISO-IEC-21838-4-2023.pdf
- BFO 2020 core OWL in Turtle (definitions quoted in §2.1): https://raw.githubusercontent.com/BFO-ontology/BFO-2020/master/21838-2/owl/bfo-core.ttl
- BFO-2020 repository README: https://github.com/BFO-ontology/BFO-2020
- BFO home page: http://bfo-ontology.github.io/
- Jensen, De Colle, Kindya, More, Cox, Beverley, "The Common Core Ontologies", 2024: https://arxiv.org/pdf/2404.17758
- CUBRC, "An Overview of the Common Core Ontologies", 12 February 2019 (Agent, Event and Information Entity sections, Figures 5, 6, 10, 11): https://www.nist.gov/document/nist-ai-rfi-cubrcinc004pdf
- CommonCoreOntologies repository: https://github.com/CommonCoreOntology/CommonCoreOntologies
- IOF, "Information about Gain of Role and Loss of Role": https://oagi.atlassian.net/wiki/spaces/IOF/pages/4820795401/Information+about+Gain+of+Role+and+Loss+of+Role
- IAO information content entity, is about, document (Ontobee): https://ontobee.org/ontology/IAO?iri=http://purl.obolibrary.org/obo/IAO_0000030 , https://ontobee.org/ontology/IAO?iri=http://purl.obolibrary.org/obo/IAO_0000136 , https://ontobee.org/ontology/IAO?iri=http://purl.obolibrary.org/obo/IAO_0000310
- IAO repository: https://github.com/information-artifact-ontology/IAO
- Borgo, Ferrario, Gangemi, Guarino, Masolo, Porello, Sanfilippo, Vieu, "DOLCE: A Descriptive Ontology for Linguistic and Cognitive Engineering", Applied Ontology 2022: https://arxiv.org/pdf/2308.01597
- COLORE (Ontolog Forum): https://ontologforum.org/index.php/COLORE
- Smith, "What's in an 'is about' link? Chemical diagrams and the Information Artifact Ontology" (abstract only): https://arxiv.org/abs/1204.4805
- Wikipedia, ISO/IEC 21838 (parts table): https://en.wikipedia.org/wiki/ISO/IEC_21838

Secondary or not fetched (claims marked unverified above):

- ISO catalogue pages for 21838-1, -2, -3, -4 (HTTP 403): https://www.iso.org/standard/71954.html , https://www.iso.org/standard/74572.html , https://www.iso.org/standard/78927.html , https://www.iso.org/standard/78928.html
- Grüninger, Ru, Thai, "TUpper: A top level ontology within standards", Applied Ontology 17(1) 2022 (abstract pages 403): https://journals.sagepub.com/doi/abs/10.3233/AO-220263 , https://philpapers.org/rec/GRNTAT-2
- Arp and Smith, "Function, Role, and Disposition in Basic Formal Ontology" (not fetched; the role definition used here is from bfo-core.ttl instead): https://www.nature.com/articles/npre.2008.1941.1.pdf
- CCO Organization one-line definition (search summary only): https://ceur-ws.org/Vol-2969/paper75-SoLEE.pdf
