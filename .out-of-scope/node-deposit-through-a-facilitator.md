# A node depositing through a facilitator

A node's own outbound deposit on EVM is not relayed by an x402 facilitator, even when the
counterparty names one. A node operator holds the chain's native gas token. The connector sends
`deposit` from its settlement key and pays its own gas.

## Why this is out of scope

The decision was made on 2026-10-01 (#1443): **an operator is assumed to hold gas.**

The request was for a node that holds a network's token and no gas to open its first peering by
posting its deposit to the `facilitator` its counterparty publishes, the way a client does (ADR
0076). On the devnet that would let `toon init && toon wallet fund && toon join devnet` finish
without the operator finding Base Sepolia ETH, since the devnet faucet sends none.

It is refused for three reasons.

- **A gasless deposit does not make a gasless node.** A peering is two channels (ADR 0075
  decision 4). The node pays over one and is paid over the other, and it collects what it earns
  with a `claim` and a `settle` it sends itself. Withdrawing from its own channel is a transaction
  too. A node with no gas could open a peering and then never collect, so the operator needs gas
  the first time it earns anyway. Relaying the deposit moves that step. It does not remove it.
- **The facilitator is the seller's cost of being paid by clients.** ADR 0076 decision 2 has the
  operator fund a facilitator because a client is a customer with a wallet and nothing else. A
  node operator runs infrastructure, and bringing gas is part of running it, as bringing the token
  is.
- **The connector calls no facilitator.** ADR 0076 decision 1 and its falsifier keep
  `/settle`, `/verify` and `/supported` out of `crates/`. Relaying a node's deposit would make the
  connector an HTTP client of a URL its counterparty chose, with a fallback rule, a mode switch and
  the `socks_proxy` question that come with it, all to defer one top-up.

A first run that stops for gas is `toon`'s to explain, not the connector's to route around:
toon-protocol/toon_cli#68 checks for gas before a deposit and says where it comes from.

The other answer considered, the devnet faucet sending ETH again, is not this repository's to
give. The drip was turned off on purpose (toon-protocol/infra, `docs/devnet.md`), and it would
help only the devnet.

A client is unaffected. It still deposits through the facilitator the operator names, with no gas
of its own.

## Prior requests

- #1443: "A node with no native gas cannot make its own deposit, even when the counterparty names
  a facilitator"
