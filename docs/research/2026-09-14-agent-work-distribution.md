# Agent work distribution: queues, logs, leases, and multi-agent coordination

**Date:** 2026-09-14
**Question:** "We don't need every agent on earth reading the same news stories every day. What is the best practice for an agentic message queue like this?"
**Scope:** Track B of the federation research pass. Queue and log semantics, coordination without a broker, classic multi-agent task allocation, current agent protocols, event sourcing and CRDTs. Ends with design implications for FFS. Track A (federated feed protocols) and track C (shared accuracy) are separate memos.

Claims are cited to the source fetched in this session. Anything marked *unverified* could not be confirmed against a primary page (several vendor doc pages returned navigation shells or redirects).

---

## 0. The one-paragraph answer

The mature answer to "who does this unit of work?" is not a broker and not a lock. It is: make the work idempotent under an exact key, deliver at least once, and let duplicates collapse on arrival. Every production queue in this memo (Kafka, SQS, Pub/Sub, Cloud Tasks) delivers at least once and pushes deduplication onto a key the producer supplies. Leases and leader election (SQS visibility timeout, etcd leases, Chubby) exist for the case where the work is expensive or has side effects that cannot be undone. Reading a news article is cheap and its key (the URL) is exact, so FFS should dedup after the fact, not coordinate before. FFS already has the shape that Kleppmann calls "the database inside out": an append-only log of signed immutable facts, pulled by peers at a watermark, with materialized views derived from it. Federation pull with per-peer watermarks *is* the queue. What is missing is only the article key and the rule that a peer's derived atoms arrive as proposals.

---

## 1. Queue and log semantics

### 1.1 Delivery guarantees

Kleppmann's definition, from *Designing Data-Intensive Applications* chapter 11: "at-least-once delivery means that messages may be delivered multiple times, but they are never lost." An idempotent operation "is one that you can perform multiple times, and it has the same effect as if you performed it only once." Relying on idempotence assumes that a restarted task replays the same messages in the same order, that processing is deterministic, and that no other node concurrently updates the same value ([chapter notes](https://technicalsummaries.com/docs/books/designing-data-intensive-applications/ddia_11/); the book itself was not fetched, so the exact wording is *unverified* beyond these secondary notes).

Kafka's own documentation has a "Message Delivery Semantics" section (§4.6) defining at most once, at least once, and exactly once, and documents `enable.idempotence` (producer sequence numbers so retries write each message once) and transactions. The Kafka documentation site returned only a navigation shell on fetch, so the section text is cited by reference and *unverified* here: https://kafka.apache.org/documentation/#semantics. The Kafka intro page did confirm the partition model: "When a new event is published to a topic, it is actually appended to one of the topic's partitions," and "events with the same event key ... are written to the same partition, and Kafka guarantees that any consumer of a given topic-partition will always read that partition's events in exactly the same order as they were written" ([Kafka intro](https://kafka.apache.org/intro)).

Google Cloud Tasks states the trade-off every queue makes when forced to choose: "Cloud Tasks aims for a strict 'execute exactly once' semantic. However, in situations where a design trade-off must be made between guaranteed execution and duplicate execution, the service errs on the side of guaranteed execution," and "You should ensure that duplicate executions are handled gracefully" ([Cloud Tasks common pitfalls](https://docs.cloud.google.com/tasks/docs/common-pitfalls)).

### 1.2 Idempotency keys and dedup windows

Amazon SQS FIFO queues take a producer-supplied `MessageDeduplicationId`: "within a 5-minute deduplication window, only one instance of a message with the same deduplication ID is processed and delivered. If Amazon SQS has already accepted a message with a specific deduplication ID, any subsequent messages with the same ID will be acknowledged but not delivered to consumers," and "Amazon SQS continues tracking the deduplication ID even after the message has been received and deleted" ([SQS deduplication ID](https://docs.aws.amazon.com/AWSSimpleQueueService/latest/SQSDeveloperGuide/using-messagededuplicationid-property.html)). Two lessons: the key comes from the producer, and the dedup memory outlives the message.

Google Pub/Sub uses ordering keys rather than dedup keys: "messages published with the same ordering key are expected to be received in order," delivery is at least once, and "redeliveries of a message trigger redelivery of all subsequent messages for that key, even acknowledged ones." Affinity of a key to one subscriber "is not guaranteed," so applications must "handle messages in any client for a given ordering key" ([Pub/Sub ordering](https://docs.cloud.google.com/pubsub/docs/ordering)). Even the ordered-delivery product tells consumers to be idempotent.

### 1.3 Partition ownership as "one consumer per key"

In Kafka the mechanism for "only one consumer handles this key" is consumer groups: partitions are assigned to exactly one consumer per group, and the key determines the partition. The Kafka intro confirms key-to-partition mapping but the consumer-group assignment rule is documented in §4.5 of the full docs, *unverified* by fetch in this session. The point for FFS: ownership is derived from the key, not negotiated per message.

### 1.4 Visibility timeouts and leases

SQS: "When you receive a message ... it remains in the queue but becomes temporarily invisible to other consumers." The default is 30 seconds, the maximum 12 hours from first receipt, and "if you don't delete it before the timeout expires, the message becomes visible again in the queue and can be retrieved by another consumer." Even so, "because of the at-least-once delivery model, Amazon SQS doesn't guarantee that a message won't be delivered more than once within the visibility timeout period" ([SQS visibility timeout](https://docs.aws.amazon.com/AWSSimpleQueueService/latest/SQSDeveloperGuide/sqs-visibility-timeout.html)). A lease is a hint that reduces duplicate work; it is never a guarantee against it.

### 1.5 Claim check and outbox

Claim Check (Hohpe and Woolf): "Store message data in a persistent store and pass a Claim Check to subsequent components. These components can use the Claim Check to retrieve the stored information" ([EIP Claim Check](https://www.enterpriseintegrationpatterns.com/patterns/messaging/StoreInLibrary.html)). FFS's content addresses are claim checks by construction: a peer receives an atom hash and fetches content only if a capability permits.

Transactional Outbox (Richardson): messages are written to an outbox table "in the same transaction that updates business entities"; a relay publishes them; "the message relay may publish duplicates," so "a message consumer must be idempotent, perhaps by tracking the IDs of the messages that it has already processed" ([microservices.io outbox](https://microservices.io/patterns/data/transactional-outbox.html)). FFS's single-writer atom store plus pull-on-watermark is an outbox with no separate relay: the store is the outbox and the peer's pull is the relay.

### 1.6 The log as the source of truth

Kleppmann, "Turning the database inside out": "make writes an append-only stream of immutable facts," derive materialized views from the log, and "the log defines the order in which writes are applied, so all the views that are based on the same log apply the changes in the same order, so they end up being consistent" ([Kleppmann 2015](https://martin.kleppmann.com/2015/03/04/turning-the-database-inside-out.html)). Log-based brokers assign "monotonically increasing sequence number, or offset, to every message," so "the broker does not need to track acknowledgments for every single message, it only needs to periodically record the consumer offsets" (DDIA chapter 11 via notes, *unverified* wording). FFS's `tx_time` watermark per peer is exactly this offset.

---

## 2. Coordination without a broker

### 2.1 Idempotency over coordination

When work is cheap and the key is exact, dedup after the fact dominates coordination before the fact: no lock service, no lease renewal, no failure mode where a lease holder dies mid-work and everyone waits for expiry. The cost is wasted duplicate work bounded by the number of peers who acted in the same window. For reading a news article that cost is one HTTP fetch and one extraction, and the duplicate collapses on the article key.

### 2.2 When leases earn their keep

Chubby's abstract: it exists "to provide coarse-grained locking as well as reliable (though low-volume) storage for a loosely-coupled distributed system," designed for "availability and reliability, as opposed to high performance" ([Chubby](https://research.google/pubs/the-chubby-lock-service-for-loosely-coupled-distributed-systems/)). etcd leases: "The cluster grants leases with a time-to-live. A lease expires if the etcd cluster does not receive a keepAlive within a given TTL period," and "when a lease expires or is revoked, all keys attached to that lease will be deleted" ([etcd API](https://etcd.io/docs/v3.5/learning/api/)). These are for work that is expensive, rate-limited, or side-effecting: a paywalled account with a daily article cap, an LLM extraction that costs money, a fetch that could trip a bot wall for everyone sharing the source. The lease pattern is coarse-grained by design; nobody leases per message.

### 2.3 The default

At-least-once delivery plus an idempotent consumer keyed on a producer-supplied id, with a dedup memory that outlives the message. Add a lease only for the expensive step, and make it advisory, as SQS does.

---

## 3. Classic multi-agent task allocation

### 3.1 Contract Net

Smith, 1980, IEEE Transactions on Computers C-29(12): "The contract net protocol has been developed to specify problem-solving communication and control for nodes in a distributed problem solver. Task distribution is affected by a negotiation process, a discussion carried on between nodes with tasks to be executed and nodes that may be able to execute those tasks" ([DeepDyve abstract](https://www.deepdyve.com/lp/crossref/the-contract-net-protocol-high-level-communication-and-control-in-a-LKgNX9ZI6w), [ACM](https://dl.acm.org/doi/10.1109/TC.1980.1675516)). The steps: task announcement (a manager broadcasts a call), bidding (contractors propose or decline), awarding ("the manager chooses among the proposals the one that suits it best and sends to the corresponding contractor an accept"), and fulfillment (inform or cancel). FIPA standardized it as the Contract Net Interaction Protocol in its ACL ([Wikipedia summary](https://en.wikipedia.org/wiki/Contract_Net_Protocol); the fipa.org specification page now serves unrelated content, so the FIPA text is *unverified*).

What Contract Net says that matters here: announcing work is the cheap half; bidding and awarding is the expensive half, worth it only when contractors differ in capability or cost. Peers reading the same free newspaper do not differ. The "announce" step alone (I have read this; here are the keys) gives most of the benefit.

### 3.2 Blackboard systems

Nii, 1986, AI Magazine: the blackboard model has a shared data structure where "problem-solving information is recorded and accessed by all participants," independent knowledge sources that contribute to it, and a control mechanism that decides which source acts next; Hearsay-II (1971 to 1976) was the first ([Nii 1986](https://ojs.aaai.org/aimagazine/index.php/aimagazine/article/view/537)). A federated FFS is a blackboard with signatures: the shared structure is the union of pulled atoms, knowledge sources are peers' scribes, and control is each owner's quarantine rather than a central scheduler.

### 3.3 Market-based allocation

Auction and market mechanisms generalize Contract Net's bidding with prices. Not fetched in this session; noted only because they answer a question FFS does not have (heterogeneous cost). *Unverified.*

---

## 4. Current agent protocols

### 4.1 A2A

The A2A specification: an Agent Card describes "identity, capabilities, skills, service endpoint, and authentication requirements"; a Task is "the fundamental unit of work managed by A2A, identified by a unique ID," with states SUBMITTED, WORKING, INPUT_REQUIRED, AUTH_REQUIRED, COMPLETED, FAILED, CANCELED, REJECTED; artifacts are outputs "composed of Parts"; push notifications are HTTP POSTs to a client-registered webhook. Task ids are server-generated: "Client-provided taskId values for creating new tasks is NOT supported" ([A2A spec](https://a2a-protocol.org/latest/specification/)). So A2A has no client-side idempotency key for task creation; dedup is the caller's problem, which is one more reason to key on content, not on task ids.

### 4.2 MCP

MCP resources support optional subscriptions: a server declares `resources: { subscribe: true, listChanged: true }`, a client sends `resources/subscribe` with a URI, and the server emits `notifications/resources/updated` when it changes, plus `notifications/resources/list_changed` for the catalog ([MCP resources, 2025-06-18](https://modelcontextprotocol.io/specification/2025-06-18/server/resources)). This is a push primitive at the agent-to-tool boundary, not between substrates. If an agent wants "tell me when a new article lands," `ffs-mcp` could expose the article listing as a subscribable resource without touching federation.

### 4.3 ANP and ACP

The Agent Network Protocol describes itself as three layers (communication, syntactic, semantic) for agents to "discover, identify, authenticate, and communicate securely across open networks" ([ANP white paper](https://arxiv.org/abs/2508.00007), [repo](https://github.com/agent-network-protocol/AgentNetworkProtocol)). AGNTCY's Agent Connect Protocol defined "a standard interface to invoke and configure remote agents over an API"; its spec and SDK repositories were archived on 2026-04-11 with the OASF schema recommending A2A instead ([acp-spec](https://github.com/agntcy/acp-spec); archival date from search summary, *unverified* against the repo page). Neither adds a work-distribution primitive beyond A2A's task model.

### 4.4 Framework-level shared memory

LangChain's handoff guidance: agents pass control through state updates ("tools update the state variable to move between states"), and "you must explicitly decide what messages pass between agents"; the recommendation is to pass only the handoff pair, because "by passing only the handoff pair, you keep the parent graph's context focused on high-level coordination" ([LangChain handoffs](https://docs.langchain.com/oss/python/langchain/multi-agent/handoffs)). Shared state is a typed object with explicit ownership per field. This is single-process orchestration, not federation, but the discipline (explicit contract, explicit ownership, minimal handoff payload) transfers directly to the article key.

---

## 5. Event sourcing, CRDTs, and watermarks

A CRDT is a data structure where independent modifications on separate replicas "can always be merged into a consistent state. This merge is performed automatically by the CRDT, without requiring any special conflict resolution code or user intervention" ([crdt.tech](https://crdt.tech/)). Automerge "allows concurrent changes on different devices to be merged automatically" and "keeps track of the changes you make to the state, so that you can view old versions, compare versions, create branches" ([Automerge](https://automerge.org/docs/hello/)).

An append-only set of signed, content-addressed atoms is a grow-only set, the simplest CRDT: union is the merge, order does not matter, and no consensus is needed. FFS already is one. Supersession chains are a DAG on top of that set, and ARCHITECTURE.md concurrency rule 1 (multi-leaf heads resolve by latest `tx_time`, then hash) is a deterministic merge function. What FFS deliberately lacks (rule 7, no vector clocks) is fine for a grow-only set; it would matter only for concurrent edits to the same claim, which the human gate handles instead.

Watermarks: Flink defines "a Watermark(t) declares that event time has reached time t in that stream, meaning that there should be no more elements from the stream with a timestamp t' <= t" ([Flink time](https://nightlies.apache.org/flink/flink-docs-stable/docs/concepts/time/)). FFS's per-peer watermark is a processing-time offset, not an event-time watermark; a peer can still receive an atom whose `valid_from` is in the past. That is expected and harmless because atoms are facts about time, not events ordered by it.

---

## 6. Design implications for FFS

What FFS has today, verified in the code: signed, content-addressed, immutable atoms with supersession; one logical writer per substrate; federation pull over mTLS with `GET /federation/v1/atoms?since=<tx_time>&capability=<hash>` returning atoms with `tx_time > since` ([server.rs](../../crates/ffs-federation/src/server.rs)); per-peer, per-capability watermarks persisted and advanced only after verification ([federation_peers.rs](../../crates/ffs-core/src/federation_peers.rs), task_15); a scheduler with `DEFAULT_HEARTBEAT` of 60 seconds and exponential backoff capped at 60 seconds ([scheduler.rs](../../crates/ffs-federation/src/scheduler.rs)); capability atoms as the ACL; the quarantine as the human gate; and, planned, `source.article` keyed by normalized URL and `content_hash` (task_40) filed by a scheduled courier.

### (a) Who reads today's CBJ: dedup, not leases

Recommendation: no claim, no lease, no election. Every peer's courier reads its own email on its own schedule and files what it finds. Duplicates across peers collapse on the article key when pulled. The wasted work is one fetch and one extraction per peer per article, which is the cost of one morning's reading and is the same cost the peer would pay if it were alone.

A claim atom becomes worth adding only when one of three conditions holds: full-page fetch is enabled and the source rate-limits or bot-walls the shared account (spike task_43 measures this); extraction runs on a paid LLM backend and peers want to split the bill; or a source has a per-subscriber daily cap. Then the shape is a lease expressed in atoms, not a lock: a `courier.claim` atom whose `valid_to` is the TTL, keyed on the article or the digest issue, honored advisorily the way SQS honors visibility timeouts. If the claimant fails, `valid_to` expires and anyone may proceed. No renewal protocol; a short TTL and idempotent dedup cover the failure case.

### (b) The dedup contract

- **Key.** `source.article` is identified by the normalized URL (scheme and host lowercased, tracking parameters stripped, trailing slash removed) and, when present, `content_hash` of the fetched bytes. The URL is the primary key across peers because it is what every peer can compute without fetching; `content_hash` catches the same content behind two URLs.
- **Collision across peers.** Two peers file the same article with different summaries. Both `source.article` atoms exist, each signed by its author, each with its own provenance and engine/model. Because the key is exact, the resolver links them with an automatic `entity.same_as` on arrival (ADR-030 reserves `same_as` for the owner's click on people; for articles the exactness of the key makes the machine link safe, and it stays undoable by supersession). The local reader sees one file, with the local summary as the head and the peer's summary as a second provenance entry and a collapsed "also filed by" section.
- **Trust per peer.** ADR-027's generated-versus-verified distinction applies per author key. A peer's article is "generated by peer P, verified by P's owner" from the local point of view; it never becomes "verified by me" until the local owner accepts it. Derived atoms (people, orgs, affiliations) from a peer are proposals, see (d).
- **Dedup memory outlives the message.** As in SQS, the key index must persist: a peer that re-files an article a month later must still collapse. The `entity.search` v2 index over `url` and `content_hash` (task_40) is that memory.

### (c) Federation pull is the queue

| Queue concept | FFS equivalent |
|---|---|
| Topic | Predicate (`source.article`, `affiliation`, ...) |
| Partition key | Entity id; for articles, the normalized URL |
| Offset | Per-peer, per-capability `tx_time` watermark |
| ACL on the topic | The capability atom named in the pull request |
| Subscription | "Pull predicates P from peer X every N seconds," which is the mount plus the heartbeat |
| Consumer commit | Watermark advance after signature verification (task_15 rule) |
| Message id for dedup | Atom content hash; article key for cross-author dedup |
| Dead-letter queue | Quarantine reject with reason |

Push is not needed for 5 to 20 peers on a daily cadence. The 60-second heartbeat already bounds latency far below the courier's once-a-day rhythm; polling with watermarks is simpler, has no webhook surface to secure, and matches ADR-020's pull-only decision. The one place push could earn its place is agent-facing, not peer-facing: an MCP resource subscription on the article listing so a local agent learns of new arrivals without polling `ffs_list_path`.

### (d) The trust boundary

A peer's derived atoms (people, orgs, affiliations, events) arrive through federation pull as signed atoms authored by the peer. They must not be inserted as head atoms about the local owner's entities. They land in the local quarantine as proposals with provenance `kind: federation` and the peer's key, subject to ADR-029's additive rule and to an Accept grant whose grantee is the peer identity. Additive proposals (a new article, a new person the local substrate has never seen, an appended mention) can auto-file if the owner has granted that peer `Accept` for those predicates; anything that would supersede a local scalar routes to review. This makes "trust peer X's reading of the news" a capability grant, revocable by supersession, and keeps the local owner's signature the only mark of "verified here."

Article atoms themselves can bypass the quarantine because they are append-only records keyed exactly and carry no claim about a local entity; ADR-029 already classifies append-only predicates as additive.

### (e) A minimal work announcement, if wanted later

Contract Net's announce step without bidding: a `courier.run` atom per run, claim `{ source: "cbj", issue: "2026-09-14", article_keys: [...], count, engine, model }`, signed by the peer, valid for the day. Peers that pull it before their own run may skip fetching keys already listed and instead pull the articles. It is optional, advisory, and costs one atom per run. It is also the audit record the auditor's briefing needs ("courier ran, 23 articles, 2 peers also covered this issue").

### (f) What not to build

- **A broker.** No Kafka, no NATS, no shared queue process. The substrate is the log; pull is the relay.
- **Exactly-once machinery.** No transactions across peers, no two-phase anything. At least once plus an exact key is the industry default and is enough.
- **Consensus.** No leader, no Raft, no vector clocks. A grow-only set of signed atoms merges by union; conflicts on a claim go to a human.
- **Per-message leases.** A claim atom, if ever, is per issue or per expensive step, advisory, with a short `valid_to`.
- **Push between peers.** Polling with watermarks at 60 seconds is already two orders of magnitude faster than the daily cadence.

### Phase summary

| Item | Where | When |
|---|---|---|
| Article key (normalized URL, `content_hash`) and persisted key index | task_40 | With task_40 |
| Automatic `same_as` for articles on exact key across authors; "also filed by" rendering | task_45 (resolver), task_38 (template) | With task_45 |
| Peer-derived atoms enter the quarantine as proposals with `kind: federation`; Accept grant per peer | ADR-029 amendment, task_39 | When federation resumes (ADR-033) |
| Article atoms bypass quarantine as append-only records | ADR-029 rule as written | With task_39 |
| `courier.run` announcement atom | task_40 follow-up | When federation resumes |
| `courier.claim` lease atom | new task | Only if spike task_43 finds rate limits or paid extraction makes duplicate work costly |
| MCP resource subscription on the article listing | ffs-mcp follow-up | Later |

---

## 7. Sources fetched or searched in this session

- SQS deduplication ID: https://docs.aws.amazon.com/AWSSimpleQueueService/latest/SQSDeveloperGuide/using-messagededuplicationid-property.html
- SQS visibility timeout: https://docs.aws.amazon.com/AWSSimpleQueueService/latest/SQSDeveloperGuide/sqs-visibility-timeout.html
- Google Pub/Sub ordering: https://docs.cloud.google.com/pubsub/docs/ordering
- Google Cloud Tasks common pitfalls: https://docs.cloud.google.com/tasks/docs/common-pitfalls
- Kafka intro (fetched): https://kafka.apache.org/intro ; Kafka documentation §4.6 semantics and `enable.idempotence` (navigation shell only, *unverified*): https://kafka.apache.org/documentation/
- Enterprise Integration Patterns, Claim Check: https://www.enterpriseintegrationpatterns.com/patterns/messaging/StoreInLibrary.html
- Transactional Outbox: https://microservices.io/patterns/data/transactional-outbox.html
- Kleppmann, Turning the database inside out (2015): https://martin.kleppmann.com/2015/03/04/turning-the-database-inside-out.html
- Kleppmann, DDIA chapter 11 (secondary notes only): https://technicalsummaries.com/docs/books/designing-data-intensive-applications/ddia_11/
- Chubby: https://research.google/pubs/the-chubby-lock-service-for-loosely-coupled-distributed-systems/
- etcd leases: https://etcd.io/docs/v3.5/learning/api/
- Smith 1980, Contract Net: https://dl.acm.org/doi/10.1109/TC.1980.1675516 ; abstract via https://www.deepdyve.com/lp/crossref/the-contract-net-protocol-high-level-communication-and-control-in-a-LKgNX9ZI6w ; author PDF https://www.reidgsmith.com/The_Contract_Net_Protocol_Dec-1980.pdf (not fetched)
- Contract Net summary and FIPA note: https://en.wikipedia.org/wiki/Contract_Net_Protocol ; FIPA SC00029H (site serves unrelated content, *unverified*)
- Nii 1986, Blackboard systems: https://ojs.aaai.org/aimagazine/index.php/aimagazine/article/view/537
- A2A specification: https://a2a-protocol.org/latest/specification/
- MCP resources (2025-06-18): https://modelcontextprotocol.io/specification/2025-06-18/server/resources
- ANP white paper: https://arxiv.org/abs/2508.00007 ; repo https://github.com/agent-network-protocol/AgentNetworkProtocol
- AGNTCY ACP spec (archived): https://github.com/agntcy/acp-spec
- LangChain handoffs: https://docs.langchain.com/oss/python/langchain/multi-agent/handoffs
- CRDT definition: https://crdt.tech/ ; Automerge: https://automerge.org/docs/hello/
- Flink watermarks: https://nightlies.apache.org/flink/flink-docs-stable/docs/concepts/time/
- FFS code read: `crates/ffs-federation/src/{server.rs,scheduler.rs}`, `crates/ffs-core/src/federation_peers.rs`, `crates/ffs-daemon/src/dispatch.rs` (`federation_pull`), `.compozy/tasks/ffs-mvp/task_15.md`
