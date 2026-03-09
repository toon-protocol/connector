# Deep Research Prompt: Signal-Grade NIP Use Cases for the Agent-Runtime + Agent-Society Stack

## Research Objective

Identify Nostr NIPs that, when combined with the existing ILP micropayment infrastructure and TOON-encoded event exchange, produce **implementable, high-value agent network use cases** — not speculative features that require years of new engineering. Every recommendation must map to primitives that already exist or are 1-2 epics away.

## Background Context

### What's Built (agent-runtime)

- ILP connector with RFC-0027 packet routing (longest-prefix match)
- BTP WebSocket peering between connectors
- Tri-chain settlement (EVM/Base L2, XRP, Aptos) with payment channels
- TigerBeetle double-entry accounting
- Admin API for peer/route/channel management (Epics 20-21)
- Agent-runtime middleware: bidirectional ILP forwarder with SHA256(data) fulfillment
- STREAM flow control for micropayment streaming

### What's Built (agent-society)

- NIP-01 compliant relay (read/write, filter matching, event verification)
- NIP-02 follow list (kind:3) → SocialPeerDiscovery → connector peering
- NIP-44 encrypted SPSP negotiation (kind:23194 request / kind:23195 response)
- kind:10032 ILP Peer Info (replaceable event: ILP address, BTP endpoint, settlement chains)
- TOON encoding/decoding for any Nostr event inside ILP packets
- BLS with per-kind pricing (kind:0 free, kind:1 cheap, kind:30023 expensive, kind:23194 configurable)
- SocialTrustManager (trust scores via BFS on follow graph — implemented but unwired)
- Bootstrap service: 3-phase discovery → handshake → announce flow
- Settlement negotiation: chain intersection + preference ordering

### What's NOT Built

- No social routing (routing is standard longest-prefix, not trust-weighted)
- No NIP-57 zap integration (referenced as TODO in SocialTrustManager)
- No NIP-05 DNS identity
- No NIP-47 Nostr Wallet Connect (only used as design reference)
- No skill registry (referenced in docs, no implementation)
- No secret sharing, no agent insurance, no Nostr-native smart contracts

### Key Architectural Insights

**"BLS Negotiates, Connector Executes":**

- The BLS handles settlement negotiation policy (which chain, which token, accept/reject)
- The connector owns all payment channel infrastructure
- BLS drives channel operations through connector Admin API
- No blockchain SDK imports in BLS code

**Social Peering vs Social Routing (current gap):**

- Kind:3 follow lists determine **topology** (who to peer with), not **routing** (which path to take)
- SocialPeerDiscovery subscribes to own kind:3 → queries kind:10032 → SPSP handshake → registers peer with connector
- Once registered, connector uses standard RFC-0027 longest-prefix matching — no trust weighting
- SocialTrustManager computes trust scores (social distance via BFS, mutual followers) but is **not wired into routing decisions**
- To achieve true social routing, trust scores need to feed into route priority during peer registration

### Currently Implemented NIPs and Event Kinds

| NIP    | Status                | Used For                                            |
| ------ | --------------------- | --------------------------------------------------- |
| NIP-01 | Fully Implemented     | Relay protocol, filter matching, event verification |
| NIP-02 | Fully Implemented     | Peer discovery via follow lists (kind:3)            |
| NIP-05 | Not Implemented       | DNS identity (future)                               |
| NIP-44 | Fully Implemented     | SPSP request/response encryption                    |
| NIP-47 | Design Reference Only | Inspiration for encrypted request/response pattern  |
| NIP-57 | Planned (TODO)        | Zap-based reputation scoring                        |

| Event Kind | Type               | Used For                                                     |
| ---------- | ------------------ | ------------------------------------------------------------ |
| kind:0     | Standard           | Profile metadata (TOON encoded)                              |
| kind:1     | Standard           | Text notes (priced at 5/byte)                                |
| kind:3     | Standard           | Follow lists → peer discovery                                |
| kind:7     | Standard           | Reactions (pricing overrides)                                |
| kind:10032 | Custom Replaceable | ILP Peer Info (ILP address, BTP endpoint, settlement chains) |
| kind:10047 | Custom Replaceable | SPSP Info (destination account, shared secret)               |
| kind:23194 | Custom Ephemeral   | SPSP Request (NIP-44 encrypted, settlement negotiation)      |
| kind:23195 | Custom Ephemeral   | SPSP Response (NIP-44 encrypted, settlement result)          |
| kind:30023 | Standard           | Long-form content articles (priced at 100/byte)              |

## Research Questions

### Primary Questions (Must Answer)

#### 1. NIP-90 (Data Vending Machines) — The Missing Skills Registry?

DVM defines a request/response pattern for computational jobs (kind:5000-5999 requests, kind:6000-6999 results, kind:7000 feedback). How does this map to the existing BLS payment flow where ILP packets carry TOON-encoded events and the BLS prices per-kind? Could DVM job requests be TOON-encoded, paid via ILP micropayment, and fulfilled by agent peers — creating a paid computation marketplace without new infrastructure? What's the gap between DVM's expected relay-based flow and our ILP-based payment flow? Is this the missing "skills registry" primitive?

**Specific sub-questions:**

- Can kind:5000-5999 job requests be TOON-encoded and sent as ILP PREPARE packets?
- Does the BLS per-kind pricing model naturally extend to DVM job pricing?
- How does DVM's "bid" mechanism map to ILP payment negotiation?
- Can kind:7000 feedback events serve as the reputation signal SocialTrustManager lacks?

#### 2. NIP-57 (Lightning Zaps) → ILP Zaps for Reputation

Zaps provide public, verifiable proof-of-payment tied to specific events. The SocialTrustManager has a TODO for zap-based reputation. Can we define an "ILP Zap" equivalent using existing kind:9734/9735 structures but backed by ILP micropayments instead of Lightning? What would the trust score formula look like (zap volume + social distance + settlement history)? How does this feed into route priority for social routing?

**Specific sub-questions:**

- Can ILP payment channel claims substitute for Lightning invoices in the zap receipt structure?
- What fields in kind:9735 (zap receipt) need modification for ILP backing?
- How does zap history feed into SocialTrustManager.computeTrustScore()?
- Can zap volume on an agent's kind:0 profile serve as a credible reputation signal?

#### 3. NIP-15 (Nostr Marketplace) + NIP-99 (Classified Listings) — Skill Marketplace

These define structured listings with prices. Can agent skills be listed as NIP-15 stall products or NIP-99 classifieds, with ILP payment pointers embedded? Does this create a discoverable, priced skill marketplace using existing Nostr infrastructure without building a custom registry?

**Specific sub-questions:**

- Can kind:30017 (stall) and kind:30018 (product) model agent capabilities?
- How does NIP-15's payment flow (order → invoice → payment) map to ILP PREPARE/FULFILL?
- Can NIP-99 kind:30402 (classified listing) represent service offerings with embedded kind:10032 ILP Peer Info?
- Does this make agent capabilities discoverable via standard Nostr clients (Primal, Damus)?

#### 4. NIP-29 (Relay-based Groups) — Agent Swarms as Groups

Groups provide scoped communication channels. Can agent swarms (collaborative research, coordinated analysis) be modeled as NIP-29 groups where membership requires an open payment channel? How does group membership map to ILP routing prefixes?

**Specific sub-questions:**

- Can NIP-29 group membership be gated by payment channel deposit?
- Does a group's relay naturally map to a shared ILP address prefix (e.g., `g.swarm-abc.member-1`)?
- Can group events (kind:9, kind:11, kind:12) be TOON-encoded for intra-swarm ILP communication?
- What's the lifecycle: form group → open channels → execute task → settle → dissolve?

#### 5. NIP-51 (Lists) — Explicit Routing Preferences

Lists enable curated collections (kind:10000 mute, kind:10001 pin, kind:30000 categorized). Can agents maintain public "trusted peers" lists (beyond kind:3 follow) that explicitly advertise routing preferences? Could a "preferred routes" list feed SocialTrustManager scores?

**Specific sub-questions:**

- Can a custom list kind (e.g., kind:30000 with `d` tag "trusted-routes") publish routing preferences?
- How does a peer's list of trusted routes feed into route priority on the connector?
- Can mute lists (kind:10000) serve as explicit distrust signals?
- Does this bridge the gap between social peering (kind:3) and social routing (trust-weighted path selection)?

#### 6. NIP-32 (Labeling) — Agent Capability Tags and Quality Ratings

Labeling attaches metadata ("L" and "l" tags) to events. Can labels be used for: (a) tagging agent capabilities/skills on kind:0 profiles, (b) rating quality of agent responses post-payment, (c) flagging unreliable agents?

**Specific sub-questions:**

- Can "L" namespace tags define agent skill taxonomies (e.g., `["L", "agent-skill"]`, `["l", "translation", "agent-skill"]`)?
- Can post-payment quality labels on kind:6000-6999 DVM results create a feedback loop?
- How do labels compose with SocialTrustManager trust scores?
- Can third-party labeling serve as the "social credit scoring" from the use-cases doc?

#### 7. NIP-58 (Badges) — Verifiable Agent Credentials

Badges are verifiable credentials issued by authorities. Can connector operators issue badges for: settlement reliability, uptime, throughput benchmarks? Can badges influence trust scores and route priority?

**Specific sub-questions:**

- Can kind:30009 (badge definition) model "99.9% settlement reliability" or "1M packets routed"?
- Can kind:8 (badge award) be issued automatically based on on-chain settlement history?
- Does badge verification via Nostr event signatures provide the "credit scoring without central authority" from the use-cases doc?
- How do badges compose with NIP-32 labels and NIP-57 zaps for a multi-signal trust model?

### Secondary Questions (Nice to Have)

#### 8. NIP-47 (Nostr Wallet Connect) — Standard BLS→Connector Interface?

NWC defines a standard for remote wallet operations. Since the connector already has payment channel management via Admin API, could NWC be adapted as the standard interface for BLS→connector channel operations, replacing the custom Admin API? Or is NWC too Lightning-specific?

#### 9. NIP-53 (Live Activities) — Pay-per-Second Data Streams

Live activities model real-time streaming events. Could agent data streams (real-time market feeds, sensor data) use NIP-53 structure with STREAM micropayment gating — pay-per-second access to live agent broadcasts?

#### 10. NIP-46 (Nostr Remote Signing) — Key Management for Multi-Agent Deployments

Remote signing allows delegated key management. For multi-agent deployments, could a "key agent" hold the master Nostr key and sign on behalf of sub-agents, avoiding key duplication across containers?

#### 11. NIP-78 (Application-specific Data) — Agent State on Relays

kind:30078 stores arbitrary app data. Could this store agent runtime state (last known balances, channel states, routing preferences) on relays for crash recovery — using Nostr as a distributed config store?

#### 12. NIP-85 (Trusted Assertions) — Arbiter Agents

Trusted assertions allow third-party attestations. Could this formalize the "arbiter agent" pattern — where trusted agents attest to the completion of tasks or the accuracy of data, enabling conditional ILP payments?

#### 13. NIP-69 (Peer-to-peer Order Events) — Agent Service Agreements

P2P order events define offer/take flows. Could this model agent-to-agent service agreements (not just trades) — "I offer 1000 translations at X price, you take 50" — with ILP payment channels as the settlement layer?

## Research Methodology

### Information Sources

1. **Primary:** Full NIP specification text from https://github.com/nostr-protocol/nips (each NIP's .md file)
2. **Secondary:** agent-society codebase for implementation feasibility
3. **Secondary:** agent-runtime codebase for connector/Admin API capabilities
4. **Tertiary:** Existing Nostr client implementations that use these NIPs (what patterns already work in the wild?)

### Analysis Framework: The Signal Test

For each NIP evaluated, apply this scoring rubric:

| Criterion                   | Question                                                                                     | Scoring                              |
| --------------------------- | -------------------------------------------------------------------------------------------- | ------------------------------------ |
| **Primitive Fit**           | Does this NIP's event structure map to an existing agent-runtime or agent-society primitive? | Must reuse >=1 existing component    |
| **Implementation Distance** | How many epics/stories to implement?                                                         | <=2 epics = signal, >4 = noise       |
| **Economic Model**          | Does the use case have a clear ILP payment flow (who pays whom, for what)?                   | Must have identifiable payer + payee |
| **Differentiation**         | Does this create something that doesn't exist in vanilla Nostr or vanilla ILP alone?         | Must require BOTH Nostr + ILP        |
| **Composability**           | Does this enable other use cases or is it a dead end?                                        | Must unlock >=1 additional scenario  |

A NIP passes the Signal Test only if it meets ALL 5 criteria.

### Data Requirements

- Full text of each NIP specification (not summaries)
- Event kind numbers, tag structures, and expected flows from each NIP
- Cross-reference with agent-society event kind registry to identify overlaps
- Cross-reference with agent-runtime Admin API to identify integration points

## Expected Deliverables

### Executive Summary

- Top 5 NIP-based use cases ranked by signal score (primitive fit x implementation distance x economic value)
- For each: one-paragraph description, which NIPs involved, which existing components reused, estimated epic count

### Detailed Analysis Per Recommended NIP

For each NIP that passes the Signal Test:

1. **NIP Specification Summary** — What the NIP defines (event kinds, tags, flow)
2. **Mapping to Stack** — Which agent-runtime/agent-society components it touches
3. **Payment Flow** — Who pays whom, how much, via what mechanism (ILP prepare/fulfill, STREAM, channel deposit)
4. **Implementation Sketch** — Which files change, what new event kinds needed, what TOON encoding additions
5. **Composability Map** — What other use cases this enables
6. **Risk/Gap Analysis** — What's missing, what could go wrong, what assumptions are made

### Anti-Patterns Section

- NIPs that look appealing but fail the Signal Test (with explanation of why)
- Common traps: NIPs that require infrastructure we don't have, NIPs that duplicate existing functionality, NIPs whose economic model doesn't survive contact with ILP micropayments

### Architecture Recommendation

- Suggested epic ordering for NIP adoption
- Dependencies between NIP implementations
- Which NIPs should be proposed as formal NIPs (extending kind:10032, kind:23194/23195 into a proper Interledger-on-Nostr NIP family)

### Mapping to Original Use Cases Document

For each use case in `docs/research/agent-network-use-cases.md`, identify:

- Which standard NIPs (if any) can deliver it
- Whether it's achievable with NIP adoption alone or requires custom protocol work
- Revised implementation distance given NIP leverage

## Success Criteria

1. Every recommended use case passes ALL 5 Signal Test criteria
2. No recommendation requires building a primitive that doesn't exist in the current codebase
3. At least 3 recommendations are implementable within 1 epic each
4. The research identifies which use cases from the original `agent-network-use-cases.md` doc can be achieved using standard NIPs (vs. requiring custom invention)
5. Clear epic-level implementation roadmap for top 3 recommendations

## Constraints

- **Do NOT recommend NIPs that require Lightning Network** — we use ILP, not LN. If a NIP is LN-specific, evaluate whether the pattern can be adapted to ILP or skip it.
- **Do NOT recommend building custom consensus mechanisms** — if a use case requires consensus, use existing Nostr relay semantics or point to a specific NIP that handles it.
- **Do NOT recommend NIPs in "Unrecommended" status** (NIP-04, NIP-08, NIP-26, NIP-96) — use their replacements instead.
- **Prioritize NIPs with "Final" status** — these have stable specifications and existing client support.
