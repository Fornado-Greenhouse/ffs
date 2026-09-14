# Federated signed feeds: how peers share intake without repeating it

**Date:** 2026-09-14
**Question (owner):** "We don't need every agent on earth reading the same news stories every day. What's the best practice for an agentic message queue like this?"
**Track:** A of two. This memo covers federated, signed, content-addressed feed protocols and what they teach about shared intake and dedup across peers. Track B covers queue semantics and multi-agent work allocation.
**Companion:** [2026-09-14-entity-resolution.md](2026-09-14-entity-resolution.md), [2026-09-14-top-level-ontologies.md](2026-09-14-top-level-ontologies.md).

All quotes below come from the primary source linked in the same section. Claims not checked against a primary source are marked *unverified*.

---

## 0. The one-paragraph answer

Every protocol that has solved this at scale does the same three things. Content gets a stable identity that is either a hash of the content (Nostr, IPFS, AT Protocol, Matrix) or a permanent URI the origin controls (ActivityPub, Atom). Re-sharing is a reference to that identity, never a copy (ActivityPub `Announce`, Nostr relays serving the same event, IPFS CIDs). And trust in a peer's content is expressed by signature plus a subscription choice, with any further judgment layered on by third parties (AT Protocol labelers, Nostr relay lists, SSB follow hops). None of them coordinate who reads first. They make reading idempotent and let dedup happen after the fact. FFS already has the signed content-addressed log and the capability-gated pull. What it lacks for shared intake is a per-predicate subscription, an "announce" that references rather than copies, and a cross-peer dedup key for articles, which the normalized URL and content hash already supply.

---

## 1. ActivityPub (W3C Recommendation, 2018)

Source: [ActivityPub](https://www.w3.org/TR/activitypub/).

- **Identity.** Actors are URIs. "All objects distributed by the ActivityPub protocol MUST have unique global identifiers, unless they are intentionally transient" (§3.1). Identifiers are "publicly dereferencable URIs, such as HTTPS URIs, with their authority belonging to that of their originating server."
- **Who stores what.** Each actor has an inbox and an outbox (§4.1). The origin server is the authority for the object; other servers cache copies but "the HTTP GET method may be dereferenced against an object's id property to retrieve the activity" (§3.2).
- **Push vs pull.** Both. Server-to-server delivery is push: "An HTTP POST request (with authorization of the submitting user) is then made to the inbox, with the Activity as the body of the request" (§7.1). Pull exists too: the outbox is an OrderedCollection a server "can GET from" (§5.1).
- **Dedup key.** The activity `id`. "The server MUST perform de-duplication of activities returned by the inbox. Duplication can occur if an activity is addressed both to an actor's followers, and a specific actor who also follows the recipient actor" (§5.2), done "by comparing the id of the activities and dropping any activities already seen." This is dedup of the *delivery*, not of the *subject matter*: two people posting the same news link produce two objects with two ids. ActivityPub has no notion of "same article".
- **Re-share without copy.** `Announce` (the boost): "Indicates that the actor is calling the target's attention the object" ([Activity Streams Vocabulary](https://www.w3.org/TR/activitystreams-vocabulary/#dfn-announce)). The announced object keeps its origin id; receivers "SHOULD increment the object's count of shares" (§7.11). The object is referenced, not duplicated.
- **Fan-out cost control.** `sharedInbox`: "a server MAY reduce the number of receiving actors delivered to by identifying all followers which share the same sharedInbox" (§7.1.3).
- **Trust.** Weak at the spec level: "Unfortunately at the time of standardization, there are no strongly agreed upon mechanisms for authentication" (§B.1). In practice servers use HTTP Signatures, which the spec references descriptively, not normatively.

Lesson for FFS: an origin-controlled permanent id plus an `Announce` that points at it is the cheapest re-share primitive there is. The gap is that ActivityPub identifies posts, not the things posts are about.

---

## 2. AT Protocol (Bluesky)

Sources: [Data repositories](https://atproto.com/guides/data-repos), [Sync spec](https://atproto.com/specs/sync), [Labels spec](https://atproto.com/specs/label), [feed-generator README](https://github.com/bluesky-social/feed-generator/blob/main/README.md).

- **Identity.** DIDs identify users; each user's repo is "a collection of data published by a single user" that is a "self-authenticating data structure, meaning each update is signed and can be verified by anyone."
- **Signing and content addressing.** "The content of a repository is laid out in a Merkle Search Tree (MST) which reduces the state to a single root hash." "A 'commit' to a data repository is simply a keypair signature over a Root node's CID." "Every node is an IPLD object (dag-cbor) which is referenced by a CID hash." What content addressing buys: any change "chains all the way to the Commit node, which is then signed," so a consumer can verify a whole repo from one signature.
- **Who stores what, push vs pull.** The PDS hosts the repo. Relays "subscribe to multiple upstream firehoses (eg, multiple PDS hosts) and aggregate them in to a single combined event stream," delivered over "WebSockets with CBOR-encoded messages and sequence-based resumption cursors." Consumers "need to track and persist the sequence number of events they have successfully processed, to be used as a cursor value when reconnecting." That is a watermark.
- **Trust.** Repo data "is self-certifying and contains verifiable signatures," but "identity and account information is not self-certifying, and consuming services are responsible for verifying it." Relays are not trusted; consumers re-verify commits.
- **Third-party curation.** Feed generators "are services that provide custom algorithms to users through the AT Protocol." They "subscribe to the firehose," index "as much or as little of the firehose as necessary," and on request return "a list of post URIs with some optional metadata attached." "Those posts are then hydrated into full views by the requesting server." Anyone can run one.
- **Third-party annotation.** "Labels are a form of metadata about any account or content in the atproto ecosystem." "Labeler services each have a service identity, meaning a DID document." "Signatures should be validated when labels are transferred between services." Clients choose whom to listen to via the `atproto-accept-labelers` header.
- **Dedup key.** CIDs for records; the same external link posted by many users is many records with many CIDs. *Unverified:* the AppView caches link-card embeds by URL, but there is no protocol-level canonical entity for "this article."

Lesson for FFS: separate the roles. The origin signs; relays move bytes and are not trusted; curators return references (URIs) that the consumer hydrates; labelers add signed opinions. This is the cleanest published model of "trust is a subscription to a signer" and it maps directly onto capability atoms plus a per-peer subscription.

---

## 3. Nostr

Sources: [NIP-01](https://github.com/nostr-protocol/nips/blob/master/01.md), [NIP-65](https://github.com/nostr-protocol/nips/blob/master/65.md).

- **Identity and signing.** Events carry `id`, `pubkey`, `created_at`, `kind`, `tags`, `content`, `sig`. The `id` is the "32-bytes lowercase hex-encoded sha256 of the serialized event data" over the canonical array `[0, <pubkey>, <created_at>, <kind>, <tags>, <content>]`. Signatures are Schnorr on secp256k1.
- **Who stores what.** Relays are dumb stores. Clients publish with `["EVENT", <event>]` and subscribe with `["REQ", <subscription_id>, <filters>]`; relays answer with matching events and `EOSE` when stored events are exhausted.
- **Dedup key.** The event `id`. Because the id is a hash of a canonical serialization, the same event fetched from five relays is byte-identical and the client drops the extra four. NIP-01 "specifies no explicit cross-relay deduplication mechanism"; it does not need one, the hash is the mechanism.
- **Replaceable and addressable events.** Kinds 10000 to 19999: "only the latest event MUST be stored by relays." Kinds 30000 to 39999: "only the latest event MUST be stored" per `(kind, pubkey, d-tag)`. Tie-break at equal timestamps: "the event with the lowest id (first in lexical order) should be retained." This is a supersession rule expressed as a relay storage policy.
- **Where to read.** NIP-65 kind 10002 lets a user "advertise relays where the user generally writes to and relays where the user generally reads mentions." "When downloading events from a user, clients SHOULD use the write relays of that user." This is the outbox model: you learn where a signer publishes and pull from there.
- **Trust.** Signatures only. Anyone can post anything to any relay; the client's follow list is the filter.

Lesson for FFS: FFS atoms are already Nostr-shaped (canonical JSON, hash id, signature) with a stronger supersession model. Nostr's addressable events are the closest published analogue to "the head of chain for (entity, predicate)". The outbox model is the subscription primitive FFS lacks: "to get Alice's articles, pull predicate `source.article` from Alice."

---

## 4. Secure Scuttlebutt

Source: [Scuttlebutt Protocol Guide](https://ssbc.github.io/scuttlebutt-protocol-guide/).

- **Identity.** An Ed25519 keypair, formatted `@<base64>.ed25519`. "Because identities are long and random, no coordination or permission is required to create a new one."
- **Feeds.** Append-only signed logs. "Each message (except the first one) references the ID of the previous message, allowing a chain to be constructed back to the first message in the feed." Messages carry previous hash, author, sequence, timestamp, hash algorithm, content. "All messages in a feed are signed by that feed's long-term secret key."
- **Why no delete.** "Messages form an append-only log, meaning that once a message is posted it cannot be modified." The chain hash makes any edit visible.
- **Replication.** Gossip. Peers use `createHistoryStream` to "ask each other for a list of messages in a particular feed," from a sequence number onward. The follow graph sets scope: "Following is a way of saying 'I am interested in the messages posted by this feed,'" and clients replicate "messages up to 2 hops out by default."
- **Blobs.** Binary content is "referred to by their blob ID" which is "the base64-encoding of the sha256 hash of the blob," fetched by want/have gossip.
- **Dedup key.** Message id (hash) and, for content, blob id. Two peers linking the same article produce two messages; the article bytes, if attached as a blob, dedup by hash.
- **Trust.** Signature plus follow distance.

Lesson for FFS: SSB is the nearest cousin of FFS federation today: per-identity signed logs, pulled from a sequence watermark, scoped by an explicit social relation (follow there, capability here). SSB's blob layer is the model for "the article body lives once, by hash; the claims about it live per author."

---

## 5. Matrix

Source: [Server-Server API](https://spec.matrix.org/latest/server-server-api/).

- **Rooms as DAGs.** "The prev_events field of a PDU identifies the 'parents' of the event, and thus establishes a partial ordering on events within the room by linking them into a Directed Acyclic Graph (DAG)."
- **Signing.** "PDUs are signed using the originating server's private key so that it is possible to deliver them through third-party servers." Event ids are content hashes.
- **Exchange.** PDUs travel in transactions over `PUT /_matrix/federation/v1/send/{txnId}`, "at most 50 PDUs and 100 EDUs." History is recovered with `/backfill` and `/get_missing_events`.
- **Why it is heavy.** Every server in a room replicates the whole room DAG, and when branches merge "a state resolution algorithm must be used to determine the resultant state," with the algorithm depending on the room version. That machinery exists because room state (membership, power levels) must converge identically on every server.

Lesson for FFS: Matrix solves a harder problem than FFS has (multi-writer shared state with convergent authority). FFS's concurrency rule 1 already picks the simpler answer: supersession chains are trees, ties resolve by tx_time then hash, and the human disambiguates. Do not import state resolution.

---

## 6. WebSub and syndication feeds

Sources: [WebSub](https://www.w3.org/TR/websub/), [RFC 4287 Atom](https://www.rfc-editor.org/rfc/rfc4287), [RSS 2.0](https://www.rssboard.org/rss-specification).

- **Roles.** Publisher, hub, subscriber. The subscriber POSTs `hub.callback`, `hub.mode`, `hub.topic` to the hub; the hub verifies intent with a `hub.challenge`; "Hubs MUST enforce lease expirations, and MUST NOT issue perpetual lease durations." On update the hub POSTs the content to the callback, optionally HMAC-signed (`X-Hub-Signature`).
- **Push after polling.** WebSub turns a polled feed into a push, but the identity model is the feed's.
- **Dedup key in feeds.** Atom: "The atom:id element conveys a permanent, universally unique identifier for an entry or feed," and "when an Atom Document is relocated, migrated, syndicated, republished, exported, or imported, the content of its atom:id element MUST NOT change." Comparison is "on a character-by-character basis." RSS: "guid stands for globally unique identifier. It's a string that uniquely identifies the item. When present, an aggregator may choose to use this string to determine if an item is new." With `isPermaLink` true (the default) the guid is the article URL.
- **Updates.** Atom `updated` marks "the most recent instant in time when an entry or feed was modified in a way the publisher considers significant"; aggregators typically "display only the entry with the latest atom:updated timestamp" per id.

Lesson for FFS: twenty years of feed readers settled on "the publisher's permanent id, which for news is the URL, is the dedup key, and `updated` orders revisions." That is exactly task_40's normalized URL plus `content_hash`. No new invention is needed for articles.

---

## 7. IPFS, IPLD, and gossipsub

Sources: [Content addressing](https://docs.ipfs.tech/concepts/content-addressing/), [gossipsub v1.0](https://github.com/libp2p/specs/blob/master/pubsub/gossipsub/gossipsub-v1.0.md).

- **CIDs.** "A content identifier, or CID, is a label used to point to material in IPFS. It doesn't indicate where the content is stored, but it forms a kind of address based on the content itself." "The same content added to two different IPFS nodes using the same settings will produce the same CID." "Any difference in the content will produce a different CID."
- **Pubsub as a primitive.** gossipsub keeps a `seen` cache, "a timed least-recently-used cache of message IDs that we have observed recently," checked "before forwarding messages to avoid wastefully republishing the same message multiple times." Peers outside the mesh get `IHAVE` lists of message ids and reply `IWANT`. Messages are not persisted: the message cache "shifts the current window, discarding messages older than the history length."

Lesson for FFS: content addressing gives dedup and verification for free, which FFS already has at the atom level (BLAKE3 multihash). gossipsub's `IHAVE`/`IWANT` is the minimal "announce by id, fetch on demand" pattern, and its lack of persistence is why it is a transport, not a memory.

---

## 8. Cross-protocol comparison

| Protocol | Identity | Signed by | Dedup key | Who stores | Learn of new | Trust expressed as |
|---|---|---|---|---|---|---|
| ActivityPub | actor URI, object URI | origin server (HTTP Sig, non-normative) | activity `id` (delivery only) | origin authoritative, others cache | push to inbox; pull outbox | follow + server policy |
| AT Protocol | DID, CID | user key over commit | CID | PDS; relays aggregate | firehose with cursor | verify sigs; choose labelers, feeds |
| Nostr | pubkey, event id | author (Schnorr) | event id (hash) | relays, dumb | REQ filters to write relays (NIP-65) | signature + follow list |
| SSB | pubkey, message id | author (Ed25519) | message id; blob hash | every replicating peer | gossip from sequence | follow hops |
| Matrix | server key, event id | origin server | event id (hash) | every room server | transactions + backfill | room membership + state resolution |
| WebSub / Atom | feed URL, entry id | hub HMAC (optional) | `atom:id` / `guid` | publisher | hub push, else poll | feed choice |
| IPFS / gossipsub | CID | message author (optional) | CID / message id | whoever pins | IHAVE / IWANT | none at protocol level |
| **FFS today** | Ed25519 pubkey, BLAKE3 atom hash, entity id | atom author | atom hash | each substrate; peer mount is attribution only | pull `since` watermark per peer and capability | capability atom, signed by the source |

The FFS row is from `crates/ffs-federation/src/server.rs` (`handle_pull_atoms` walks "every predicate in the capability's scope (or the responder's whole vocab when scope.predicates is None)," lists atoms after `since`, and re-evaluates each against the capability evaluator), `scheduler.rs` (per-peer watermark keyed by capability), `handshake.rs` (vocabulary intersection exchanged at bridge time), and `mount.rs` (pulled atoms "enter the local store with their original signatures preserved"; the mount is "the attribution layer").

---

## 9. Design implications for FFS

### (a) What FFS resembles, and what is missing

FFS federation today is an SSB-shaped signed log pulled with a Nostr-style filter: a peer pulls everything after a watermark that its capability covers, per predicate, and re-verifies signatures locally. That is the right base. Three things are missing for shared intake:

1. **A subscription primitive.** Today "what I pull from Alice" is "everything Alice's capability lets me read," walked predicate by predicate at pull time. Shared intake needs the consumer to say "from Alice I want `source.article` and `event.business`, not her contacts," and needs a per-`(peer, predicate)` watermark rather than one per peer and capability. This is NIP-65's outbox model and AT Protocol's per-collection sync applied to FFS's existing endpoint: add a `predicates` filter to `GET /federation/v1/atoms` and a `federation_subscriptions` table keyed by `(peer, predicate)` with its own watermark. The capability still bounds what the source will serve; the subscription narrows what the consumer asks for. Small change; it lands inside the existing pull scheduler.
2. **An announce that references, not copies.** When Bob's substrate has already filed an article Alice's courier is about to fetch, Bob's `source.article` atom, pulled by Alice, should short-circuit her courier: the atom's `url` and `content_hash` match, so she records provenance `kind: federation, peer: bob, atom: <hash>` on her own article record and does not fetch or re-extract. This is ActivityPub's `Announce` and gossipsub's `IHAVE` in one: the reference is the atom hash, the fetch-on-demand is `GET /federation/v1/atom/<multihash>`, which already exists. No new endpoint; the courier gains one lookup before fetching (task_40's dedup ledger consults pulled `source.article` atoms as well as local ones).
3. **A cross-peer dedup key for articles.** Articles have a natural key that people do not: the normalized URL and the page `content_hash` (task_40). Two substrates that both file the same CBJ story produce two `source.article` atoms with different entity ids but the same key. The `entity.same_as` predicate from ADR-030 resolves that pair mechanically, and it can be auto-asserted for articles because the key is exact, unlike people. This is the Atom `atom:id` rule and the RSS `guid` rule applied across substrates.

None of the seven protocols coordinates "who reads first." They make reading idempotent by key and dedup after the fact. FFS should do the same: no lease, no lock, no leader. Two peers both reading the same email costs one extra fetch and yields one `same_as`. Track B covers the case where reading is expensive enough to warrant a claim.

### (b) A peer's derived atoms: pull as-is, as proposals, or re-derive?

Recommendation: **pull the article as a fact, pull the derived people, orgs, affiliations, and events as proposals into the local quarantine, and never re-derive by default.**

The trust argument, in the protocols' own terms. AT Protocol separates the signed record from the labeler's opinion about it; the consumer verifies the record and chooses whether to honor the label. ADR-027 already makes the same split inside one substrate: a proposal is "generated," an owner-accepted signed atom is "verified." A peer's `person.generic` atom is verified *by that peer's owner*, which for the local owner is exactly one notch above generated: a trusted human looked at it, but not this human, and the local resolver has not matched it against local entities. So:

- `source.article` atoms cross as-is. They are facts about a public document, keyed by URL, signed by the peer, and cheap to verify. Pulling them is the "announce."
- Derived atoms (`person.generic`, `org.company`, `affiliation`, `event.business`) cross as **proposals** carrying provenance `kind: federation` plus the peer's original `engine`/`model` provenance (ADR-026) intact. They enter the local quarantine, run through the local resolver (ADR-030) so Alice's "Sara Chen" is matched against Bob's existing Sara Chen or minted fresh, and are subject to ADR-029's additive rule: a new org auto-files if Bob's grant covers Alice as a grantee, a role change routes to review. The `from/<peer>/` mount continues to show the peer's atoms as the peer's view; the proposal path is how they become the local owner's view.
- Re-derivation stays available as a per-peer setting ("re-extract with my engine") for the case where the owner trusts the peer's reading list but not the peer's model. It is the expensive path and should not be the default; the point of shared intake is not to run the same extraction twice.

This keeps the human gate intact across the federation boundary without making federation useless: the article log is shared, the cabinet stays yours.

### (c) Curators and labelers

Two AT Protocol ideas transfer cleanly:

- **A peer as curator.** "I trust Alice's reading of CBJ" is a subscription to Alice's `source.article` predicate plus an `Accept` capability (ADR-029) naming Alice's substrate key as grantee, scoped to `source.article` and `event.business` only. Alice's articles then auto-file; her people and orgs still go through the local resolver and quarantine. The feed generator analogy holds: Alice returns references (article atoms), the local substrate hydrates (renders projections from its own head atoms).
- **The auditor as labeler.** The auditor already emits signed `auditor.daily_summary` atoms. A label is a signed opinion about a subject; `auditor.briefing` (task_41) flagging "possible duplicate" or "low-confidence extraction" is one. If a peer pulls those atoms under capability, they are labels in the AT Protocol sense: signed by a service identity, about a subject atom, honored or ignored by the consumer. No new machinery; the predicate and the `subject` field are the whole design.

### (d) What not to adopt, for 5 to 20 trusted peers

- **Global relays or a public firehose.** Nostr and AT Protocol need them because the network is open and large. FFS peers are bilateral, capability-gated, and few. The pull scheduler with per-subscription watermarks is the whole transport.
- **Matrix-style replicated state with state resolution.** FFS does not need every peer to converge on one authoritative view; each substrate's owner is the authority for that substrate, and supersession trees plus the human tie-break (concurrency rule 1) are sufficient.
- **Two-hop gossip (SSB).** Friends-of-friends replication would leak who you read. Capability atoms are explicit for a reason; keep replication one hop.
- **Push delivery as the primary path.** ActivityPub's inbox push needs a reachable endpoint and retry queues. FFS chose pull for NAT and simplicity (ADR-020). Keep push only for the existing revocation notice.
- **Ephemeral pubsub without persistence.** gossipsub forgets in seconds; FFS's queue is the atom log itself, which never forgets.

### (e) Summary of changes and where they land

| Change | Where | When |
|---|---|---|
| `predicates` filter on `GET /federation/v1/atoms`; `federation_subscriptions` keyed by `(peer, predicate)` with own watermark | ffs-federation, dispatcher | when ADR-033 lifts |
| Courier consults pulled `source.article` atoms (by normalized url and `content_hash`) before fetching; records `kind: federation` provenance instead | task_40 dedup ledger | task_40 can leave the hook; wiring when ADR-033 lifts |
| Auto-asserted `entity.same_as` for article records with equal keys across peers | ADR-030 resolver, article-only rule | when ADR-033 lifts |
| Peer-derived people/orgs/affiliations/events enter the local quarantine as proposals with federation provenance; re-derive as an opt-in | ADR-029, ADR-033 reopen list | when ADR-033 lifts |
| "Trust Alice's reading": `Accept` grant to a peer key scoped to `source.article`, `event.business` | ADR-029 grantee model | when ADR-033 lifts |
| Auditor atoms as labels: a `subject` field on `auditor.briefing` items | task_41 | task_41 |
| Not adopted: relays, firehose, state resolution, multi-hop gossip, push-first | ADR-033 reopen notes | record now |

---

## 10. Sources fetched in this session

- W3C ActivityPub: https://www.w3.org/TR/activitypub/
- W3C Activity Streams Vocabulary (Announce, Create): https://www.w3.org/TR/activitystreams-vocabulary/#dfn-announce
- AT Protocol data repositories: https://atproto.com/guides/data-repos
- AT Protocol sync (firehose, relays): https://atproto.com/specs/sync
- AT Protocol labels: https://atproto.com/specs/label
- Bluesky feed-generator README: https://github.com/bluesky-social/feed-generator/blob/main/README.md
- Nostr NIP-01: https://github.com/nostr-protocol/nips/blob/master/01.md
- Nostr NIP-65: https://github.com/nostr-protocol/nips/blob/master/65.md
- Scuttlebutt Protocol Guide: https://ssbc.github.io/scuttlebutt-protocol-guide/
- Matrix Server-Server API: https://spec.matrix.org/latest/server-server-api/
- W3C WebSub: https://www.w3.org/TR/websub/
- RFC 4287 Atom: https://www.rfc-editor.org/rfc/rfc4287
- RSS 2.0 specification: https://www.rssboard.org/rss-specification
- IPFS content addressing: https://docs.ipfs.tech/concepts/content-addressing/
- libp2p gossipsub v1.0: https://github.com/libp2p/specs/blob/master/pubsub/gossipsub/gossipsub-v1.0.md
- FFS: `crates/ffs-federation/src/{server,scheduler,handshake,mount}.rs`, ADR-020, task_15

Not fetched or not verified: Bluesky AppView link-embed caching behavior (docs.bsky.app custom-feeds page returned blank); libp2p docs overview page (404, the spec repo was used instead).
