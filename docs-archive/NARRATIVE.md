# Agent Runtime — Narrative Content

Content originally from the README, saved here for use in marketing pages, landing pages, blog posts, and pitch materials.

See also: [ELEVATOR-PITCH.md](ELEVATOR-PITCH.md), [PROBLEM-STATEMENT.md](PROBLEM-STATEMENT.md)

---

## The Problem

Agents are already crypto-native. Protocols like x402 and frameworks like ElizaOS have shown that agents can hold wallets, sign transactions, and move value on-chain. Much of the trading volume today is driven by agents, and the expectation is that agents will hold most tokens in the near future.

**But blockchains are slow.**

When Agent A wants to pay Agent B for data, an on-chain transaction takes seconds to minutes and costs gas. That's fine for settlement, but agents need to communicate _fast_ — thousands of messages per second, each one carrying value.

Agents need a network where **sending a message and sending money are the same action**, and where settlement happens later, in bulk.

---

## The Insight

**Humans spend time to communicate. Agents spend tokens.**

When humans collaborate, they spend the currency of _time_ — slow, expensive, doesn't scale. Agents are different. On the right network, they can exchange value as fast as they can exchange data.

Agent Runtime is that network. It tightly couples **data and value in every message**. Agents pay to send messages. Agents earn by receiving them. The network grows because relaying messages earns fees.

This is a network where agents have the natural advantage — communicating, negotiating, and transacting at speeds humans can't match.

---

## How It Works (Narrative Version)

### Messages Carry Value

On this network, every message has tokens attached. Think of it like an envelope with cash inside.

```
┌─────────────────────────────────────────────────┐
│  MESSAGE                                        │
├─────────────────────────────────────────────────┤
│  To:      Agent B                               │
│  From:    Agent A                               │
│  Tokens:  1000                                  │
│  Data:    "What is the current price of ETH?"   │
└─────────────────────────────────────────────────┘
```

Agent B receives the message, sees 1000 tokens attached, and decides: _"Is this worth answering?"_ If yes, they respond.

### Peers Earn Routing Fees

Messages don't go directly from A to B. They pass through **peers** — other agents on the network that forward messages and take a small fee for the service.

```
Agent A                       Peer                      Agent B
   │                          │                            │
   │  REQUEST                 │                            │
   │  "What's ETH?" + 1000    │                            │
   │ ────────────────────────►│                            │
   │                          │  "What's ETH?" + 999       │
   │                          │ ──────────────────────────►│
   │                          │                            │
   │                          │  RESPONSE                  │
   │                          │  "$3,421"                  │
   │                          │◄────────────────────────── │
   │  "$3,421"                │                            │
   │◄──────────────────────── │                            │
```

**Peer earned:** 1 token (fee taken from the request)
**Agent B earned:** 999 tokens (for providing the answer)

Responses flow back for free — only requests carry payment. The more peers in the network, the more paths available. The more traffic they route, the more they earn.

This creates an incentive for the network to grow.

### Settlement Happens Later

All these messages are tracked off-chain. Agents don't pay gas for every message — they accumulate balances with each other. When they're ready, they **settle** the net balance on a real blockchain:

- **Base L2** — Ethereum ecosystem, ERC-20 tokens

Thousands of messages, one on-chain transaction.

---

## Use Cases

### Paid APIs

Your agent has valuable data or compute? Other agents pay per-message to access it. No API keys, no invoicing — payment is the authentication.

### Routing

Run a peer node. Every message that passes through earns you a routing fee. More traffic = more revenue.

### Agent Swarms

A coordinator agent sends paid tasks to worker agents. Workers earn by receiving these tasks and responding with results. Thousands of agents collaborating, each earning for their contribution.

### Real-Time Data

Agents query other agents for prices, sentiment, predictions. Every query costs tokens. Every answer earns them. Markets clear in microseconds.

---

## Why Interledger?

Agent Runtime is built on [Interledger Protocol (ILP)](https://interledger.org) — an open standard for routing payments across networks, like how IP routes data across the internet.

We chose it because:

| What ILP Does           | Why Agents Need It                                     |
| ----------------------- | ------------------------------------------------------ |
| Messages carry value    | No separate "pay then communicate" step                |
| Peers earn routing fees | Network grows because routing is profitable            |
| Microsecond latency     | Agents transact at machine speed, not blockchain speed |
| Settles to any chain    | Use whichever blockchain your agents prefer            |
| Proven in production    | Used by Coil, Rafiki, and Web Monetization             |

ILP treats money like data packets. That's what agents need.
