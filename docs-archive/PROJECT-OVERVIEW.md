# M2M Project Overview

## Elevator Pitch

**M2M (Machine-to-Machine)** is an educational protocol stack that demonstrates how autonomous AI agents can coordinate economically without centralized infrastructure. By fusing the Interledger Protocol (ILP) with Nostr's decentralized communication and EVM payment channels (Base L2), M2M creates agents that are simultaneously payment routers, event relays, and settlement executors. Every agent interaction carries native micropayments, routed through social graph topology with cryptographic escrow guarantees. The result: a working prototype of an agent society where machines compensate each other for services—queries, storage, compute, streaming—without trusted intermediaries, using proven standards instead of inventing new consensus mechanisms or token economics.

---

## Problem Statement

The explosion of AI agents and autonomous economic actors has created an urgent need for a **protocol-level infrastructure** that enables machine-to-machine (M2M) coordination, value transfer, and service exchange. Current solutions face three critical gaps:

### 1. Payment Infrastructure Gap

No native protocol exists for micropayment routing between autonomous agents. Existing payment rails (credit cards, crypto exchanges) are designed for humans, with high fees, slow settlement, and no support for sub-cent transactions required for AI-to-AI services.

### 2. Communication-Payment Decoupling

Agent communication protocols (HTTP APIs, message queues, Nostr relays) are fundamentally separate from payment systems, creating friction, trust dependencies, and delayed settlement that prevent truly autonomous operation.

### 3. Centralization Dependencies

Current agent architectures rely on centralized infrastructure (payment processors, relay servers, coordination services), introducing single points of failure, censorship risks, and trusted intermediaries incompatible with autonomous agent societies.

**Without a unified protocol that combines decentralized routing, native micropayments, and trustless settlement across multiple blockchains, the emerging agent economy cannot scale beyond walled gardens and centralized platforms.**

---

## Project Description

**M2M (Machine-to-Machine)** is an educational implementation of a next-generation protocol stack for autonomous agent communication and commerce, combining three proven standards into a unified infrastructure:

### Core Innovation: Unified Connector-Relay Architecture

M2M extends the **Interledger Protocol (ILP)** to create agents that are simultaneously:

- **ILP Connectors** — Route payment packets through multi-hop networks
- **Nostr Relays** — Store and query events locally using libSQL databases
- **Settlement Executors** — Settle balances on EVM (Base L2)

This architectural fusion eliminates the separation between communication and payment, enabling **every agent interaction to carry native value transfer**.

### Key Capabilities

#### 1. Multi-Hop Payment Routing (ILPv4 + BTP)

- RFC-compliant packet routing with cryptographic escrow (hashed timelock agreements)
- Bilateral Transfer Protocol (BTP) for connector-to-connector WebSocket communication
- Social graph-based routing using Nostr follow relationships (Kind 3)
- Automated test packet sender for network validation

#### 2. EVM Settlement Infrastructure

- **EVM Payment Channels** (Base L2) — Instant settlement with sub-cent fees

#### 3. Agent Society Protocol (ILP + Nostr)

- TOON-serialized Nostr events inside ILP packets (40% smaller than JSON)
- Per-agent local event storage (no external relay dependencies)
- Micropayment-enabled services: queries, storage, work execution, streaming payments
- NIP-90 DVM (Data Vending Machine) compatibility for ecosystem interoperability

#### 4. Production-Grade Developer Experience

- Docker Compose topologies (linear chain, full mesh, hub-spoke, 8-node complex)
- Local blockchain infrastructure (Anvil for EVM)
- Real-time telemetry and structured JSON logging
- Agent Explorer UI for packet inspection, event browsing, settlement monitoring
- Vercel AI SDK integration for AI-native agents with modular skills

#### 5. Advanced Agent Capabilities (In Progress)

- **Private Messaging (Epic 32)** — NIP-59 giftwrap with 3-layer encryption routed through ILP
- **Streaming Payments (Epic 23)** — NIP-56XX replacing ILP STREAM for continuous micropayments
- **zkVM Compute Verification (Epic 25)** — Trustless execution proofs via RISC Zero/SP1
- **Service Markets (Epic 26)** — Multi-party staking markets for agent reliability
- **Workflow Composition (Epic 31)** — Multi-hop image processing with cross-chain settlement

### Why M2M Matters

- **Educational Foundation**: Learn Interledger protocol mechanics through hands-on experimentation with real packet routing, settlement, and multi-agent coordination
- **Production Patterns**: Demonstrates architectural patterns for building autonomous agent economies with cryptographic guarantees
- **Ecosystem Bridge**: Combines ILP (proven payment routing), Nostr (decentralized communication), and EVM settlement (Base L2)
- **Market Validation**: Targets $76-113B live streaming market (Epic 24) and broader AI agent service economy

### Current Status

- ✅ **Core ILP Implementation** (Epics 1-2, 4-10)
- ✅ **EVM Settlement** (Epics 8, 9, 27-30)
- ✅ **Agent Society Protocol** (Epics 13-16)
- 🚧 **Private Messaging & Workflows** (Epics 31-32, in progress)
- 📋 **Advanced Features** (Epics 17-26, roadmap)

---

## Target Users

1. **Blockchain Developers** — Learn ILP routing, payment channels, and EVM settlement
2. **AI Agent Builders** — Prototype autonomous agents with native payment capabilities
3. **Protocol Researchers** — Experiment with decentralized coordination and economic primitives
4. **Students & Educators** — Understand distributed systems, cryptographic escrow, and micropayment networks

---

## Strategic Positioning

M2M is **not a production payment network** (see Rafiki for production ILP). Instead, it's an **educational protocol laboratory** that demonstrates how autonomous agents can coordinate economically using proven standards (ILP, Nostr, payment channels) without inventing new consensus mechanisms or token economics.

The project validates the **technical feasibility** of agent societies with:

- Cryptographic payment guarantees (HTLCs)
- Decentralized routing (social graph topology)
- Trustless settlement (EVM payment channels)
- Privacy-preserving communication (NIP-59 giftwrap)

---

## References

- [Epic List](prd/epic-list.md) — Complete roadmap of all epics
- [Epic 13: Agent Society Protocol](prd/epic-13-agent-society-protocol.md) — Core protocol specification
- [README](../README.md) — Technical documentation and setup instructions
- [Interledger RFCs](https://interledger.org/rfcs/) — Protocol specifications
