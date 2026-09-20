# Ontology alignment of the predicate set

FFS predicate specs may carry an `[ontology]` table (ADR-031). It is
informative only: the loader accepts any string values, nothing in
the daemon branches on it, and no reasoner runs over it. Its job is
design discipline now and vocabulary agreement between substrates
later.

## The `[ontology]` table

```toml
[ontology]
bfo = "role"                 # one of the BFO 2020 values below
cco = "cco:OccupationRole"   # optional Common Core Ontologies class, free identifier
iao = ""                     # optional Information Artifact Ontology class, free identifier
note = "bearer = claim.person, context = claim.organization, temporal region = valid_from..valid_to"
```

`bfo` values in use, from Basic Formal Ontology 2020 (ISO/IEC 21838-2):

| Value | Meaning | When a predicate is this |
|---|---|---|
| `continuant` | persists through time, has no temporal parts | rarely used directly; prefer a subtype |
| `object` | an independent continuant that is a unified whole | a person |
| `object aggregate` | an independent continuant made of objects | an organization |
| `role` | a realizable entity a bearer has because of social circumstance | an affiliation, a title held |
| `quality` | a dependent continuant fully exhibited at any time | a contact method, a location |
| `occurrent` | unfolds in time, has temporal parts | rarely used directly; prefer `process` |
| `process` | an occurrent with participants | a hire, a funding round, an opening |
| `information content entity` | a generically dependent continuant that is about something (IAO) | an article, a note, an assertion, a grant |

`cco` and `iao` are free identifiers; the ones used here are
`cco:Person`, `cco:Organization`, `cco:OccupationRole`, `cco:Act`,
`iao:IAO_0000030` (information content entity), and
`iao:IAO_0000310` (document).

## Starter predicates

| Predicate | `bfo` | `cco` | `iao` | Reading |
|---|---|---|---|---|
| `contact.person` | object | cco:Person | | A person the owner has a relationship with; contact methods are qualities of the person; tier is the owner's classification |
| `person.generic` | object | cco:Person | | A person referenced in narrative content; roles live in `affiliation` atoms |
| `org.company` | object aggregate | cco:Organization | | A group of persons acting as one agent; people attach through affiliation role atoms, not fields |
| `affiliation` | role | cco:OccupationRole | | bearer = `claim.person`, context = `claim.organization`, temporal region = the atom's `valid_from`..`valid_to` |
| `event.business` | process | cco:Act | | An occurrent with participants; `date` is its temporal region; `kind` names the process type; participants carry a role in the event |
| `source.article` | information content entity | | iao:IAO_0000310 | A document about entities; `mentions[]` are is-about links; `url` and `content_hash` identify the bearer; the summary is a derived information content entity |
| `note` | information content entity | | iao:IAO_0000310 | Narrative content about entities; `references[]` are is-about links |
| `entity.same_as` | information content entity | | iao:IAO_0000030 | An owner-signed assertion that two ids designate one particular; the redirect, never a deletion |
| `entity.different_from` | information content entity | | iao:IAO_0000030 | An owner-signed assertion that two ids designate distinct particulars; a hard block for the resolver |
| `capability.grant` | information content entity | | iao:IAO_0000030 | A signed policy statement about what an agent may do; evaluated by the capability evaluator, never rendered as a file (no starter spec; the shape is fixed in ADR-007) |
| `auditor.daily_summary`, `auditor.briefing` | information content entity | | iao:IAO_0000030 | Derived reports about the substrate's own state (no starter spec today; task_41 adds `auditor.briefing`) |

Every starter spec's `[ontology]` table matches its row here. When a
spec changes class, change both.

## What the alignment buys

1. **Design discipline.** Asking whether a predicate is a thing, a
   role, an event, or a document is the question that caught
   "organization as a scalar on the person" (ADR-031). A role is its
   own atom with a bearer, a context, and a temporal region; the
   person's `organization` field is a rendering convenience.
2. **Vocabulary agreement between substrates.** Two peers that both
   say `affiliation` is a BFO role with bearer and context can map
   each other's predicates even when field names differ. Bilateral
   federation (ADR-007) has no vocabulary negotiation today; this
   table is where it will start (ADR-033 reopen question 2).
3. **A cheap path to RDF or OWL export later.** Each atom becomes a
   small graph: `entity rdf:type cco:Person`, a role atom with
   `bearer of` and `has organizational context`, an article with
   `is about`. Nothing in this task builds the export; the
   annotations make it a projection rather than a redesign.

## What is deliberately not adopted

Per ADR-031 and the research memo behind it
(`docs/research/2026-09-14-top-level-ontologies.md`):

- **No OWL reasoning in the substrate.** The runtime's questions are
  lookups, not subsumption.
- **No class hierarchies.** The predicate name is the type, flat.
  Deep artifact and quality trees exist for cross-agency integration
  a personal substrate does not need.
- **No temporalized relations or gain, loss, and stasis process
  atoms.** Those exist because OWL properties are binary and
  atemporal. FFS atoms carry `valid_from` and `valid_to` and a
  supersession chain; the atom is the temporal region.
- **No requirement that a role be realized in a recorded process.**
  Storing the role does not require storing the running of the
  company.
- **No ISO/IEC 21838-1 conformance claim.** Conformance is a relation
  between ontologies with dual OWL and Common Logic axiomatizations
  and consistency proofs. Alignment annotations give the
  interoperability benefit at a fraction of the cost.

## Related

- ADR-028 (registry-declared path families), ADR-030 (entity identity),
  ADR-031 (this alignment), ADR-034 (attestations, which are also
  information content entities about atoms).
- `starter/predicates/README.md` for the per-spec tables.
