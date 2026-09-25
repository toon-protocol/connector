# A channel is derived from its participants, on both chains, by the same rule

**Status:** Accepted — **built** (#1158). `channelCounter` is gone; `TokenNetwork.channelEpoch` is public and `openChannel` derives `keccak256(p1, p2, epoch)`, with the epoch advancing in `settleChannel`. `ChannelAlreadyExists` is a live refusal. Required by [0058](0058-a-peering-is-established-from-a-url.md) — a peering established from a URL has no channel id to be told, so it must compute one. Disturbs [0024](0024-peer-wire-claims-sign-the-eip-712-balance-proof.md), [0053](0053-a-solana-claim-binds-its-domain-the-way-an-evm-claim-does.md) and [0021](0021-vectors-are-normative-prose-is-not.md). **The redeploy this needs happened on 2026-08-28**: `DeployTestnetCutover.s.sol` broadcast at Base Sepolia block 46055303 — `TokenNetwork` `0xe9E05dfe…` answers `channelEpoch(address,address)` and has no `channelCounter()`, read live after the broadcast. The superseded `0xa79C3b1d…` still carries the counter and still settles the channels opened on it. The record of the broadcast and the repoint is [`docs/evm-deployment.md`](../evm-deployment.md)'s "Second cutover". **Amended by [0074](0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md)** at the client edge only: an x402 batch-settlement channel is named by the voucher and verified on chain rather than derived, and a second live one from the same client is not refused. Every channel a connector opens itself stays derived and unique per pair.

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

**A channel's identifier is computed from its two participants, the token, and a public per-pair
epoch — never from a global counter and never from an index.** Anyone holding the two participant
addresses can compute the identifier and ask the chain whether that channel exists. **At most one
live channel exists per participant pair per token**, on EVM and on Solana alike.

## Why this is required, not merely tidier

[ADR 0058](0058-a-peering-is-established-from-a-url.md) establishes a peering from a URL. A
self-description carries facts about a node; it cannot carry a channel id, because a channel is a
_bilateral_ object that may not exist yet and that neither party can assert alone. So the connector
has to answer one question from public data: **"do I already have a channel with this counterparty?"**

**On Solana that question is already answerable.** The channel account is a PDA
(`packages/solana-program/src/processor.rs:206-212`):

```rust
let channel_seeds: &[&[u8]] = &[
    b"channel", min_participant.as_ref(), max_participant.as_ref(),
    token_mint_info.key.as_ref(), &[channel_bump],
];
```

Sort the pair, add the mint, derive the address, read the account. The PDA also enforces uniqueness
structurally: a second channel for the same pair and mint cannot be created, because it would be the
same address.

**On EVM it is not answerable at all** (`packages/contracts/src/TokenNetwork.sol:225-232`):

```solidity
(address p1, address p2) = sender < participant2 ? (sender, participant2) : (participant2, sender);
bytes32 channelId = keccak256(abi.encodePacked(p1, p2, channelCounter));
channelCounter++;
if (channels[channelId].state != ChannelState.NonExistent) revert ChannelAlreadyExists();
```

`channelCounter` is **global** — it increments whenever anybody opens any channel on this
`TokenNetwork` — so the id cannot be computed from the pair. Storage is `mapping(bytes32 => Channel)`
keyed by id only; there is no participant-to-channel mapping. Several channels between one pair are
legal. And the connector's own index cannot help: its only query is `lookup(channel_id, own_address)`
(`connector-settlement-evm/src/channel_index.rs:432`).

So one chain answers the question by construction and the other cannot answer it at all. That
asymmetry is not a property of EVM; it is a property of this contract.

## The counter earns nothing, and it costs the check below it

`ChannelAlreadyExists` **can never fire.** The counter is monotonic and never reused, so the id is
always fresh and `channels[channelId].state` is always `NonExistent`. It is a guard against a case
the line above it made impossible — dead code sitting where the useful check belongs.

Removing the counter turns that same line into the mechanism this record needs: with the id derived
from the pair, `ChannelAlreadyExists` becomes the chain's own answer to "do I already have one?".

## The decision

**1. The identifier is derived.**

```solidity
mapping(address => mapping(address => uint256)) public channelEpoch;   // public: readable off-chain
bytes32 channelId = keccak256(abi.encodePacked(p1, p2, channelEpoch[p1][p2]));
```

`p1`/`p2` are the participants in sorted order, exactly as today. `channelCounter` is deleted.

**2. At most one live channel per pair, per token.** A `TokenNetwork` is already deployed per token
(`TokenNetworkRegistry`), so the token is implicit in the contract address, and the pair plus the
epoch identify the channel within it. A second `openChannel` for a live pair reverts
`ChannelAlreadyExists` — now a real refusal.

**3. The epoch exists so a pair can start again.** Without it a settled channel would occupy its
pair's only identifier forever and the two parties could never reopen. The epoch increments on
settlement, and because the mapping is `public` its current value is readable by anyone, so the
identifier stays derivable from public data across any number of channel lifetimes.

**4. Both chains answer the same question the same way.** Sort the pair, take the token, take the
current epoch, compute, read. The connector's derive-or-open path is one piece of logic with two
chain bindings, rather than a fast path on Solana and an indexing workaround on EVM.

## Rejected: a reverse mapping on top of the counter

Adding `mapping(address => mapping(address => bytes32[]))` and keeping the counter was considered. It
is a smaller diff and leaves channel-id semantics alone.

It was rejected because it answers the wrong question. "Which of my several channels with this
counterparty should this peering use?" is an ambiguity the caller must then resolve, and there is no
principled answer — the newest is not necessarily the funded one, and the funded one is not
necessarily the one the _other_ side will pick. [ADR 0058](0058-a-peering-is-established-from-a-url.md)'s
symmetry argument is the point: B must be able to add A the same way A added B and land on the same
channel, without either telling the other an id. A list does not give that; a derivation does.

## Rejected: a local participant index in the connector

Storing the participant pair alongside each `ChannelOpened` event in
`connector-settlement-evm`'s channel index would make the lookup local, with no contract change at
all.

It was rejected because the index is only correct after `channel_index_from_block`. A channel opened
before that window is invisible, so the connector concludes "none exists" and opens a duplicate —
spending gas and splitting collateral across two channels, silently, with the wrong answer looking
exactly like the right one. A derived identifier has no window: it is a question about the chain's
current state, not about what this node happened to observe.

## The cost, stated

**This is a redeploy, not a patch.** `TokenNetwork` is `Ownable, Pausable, ReentrancyGuard, EIP712,
ERC2771Context` with no proxy, and `token`, `maxChannelDeposit` and `maxChannelLifetime` are
`immutable`. There is no upgrade path.

- Channels on the current Base Sepolia deployment are **stranded**: a new `TokenNetwork` has none of
  them. The devnet's token is a mock USDC minted on demand, so this is an operational reset, not a
  loss — but it must be sequenced deliberately, and any live channel settled first.
- Devnet configs name `token_network` addresses, so this is a **breaking deploy**: the config lands
  before the tag moves, and the release carries `config-change-required: true`
  ([0055](0055-a-release-is-one-dispatch-and-the-ordering-rides-as-data.md)).
- Nothing here reaches mainnet, because nothing here has a mainnet
  ([0056](0056-production-is-a-named-empty-tier.md)). Doing this before a mainnet deployment exists
  is the cheapest this change will ever be, and after one it is not obviously possible at all.

**A channel id is wire-visible.** A claim signs it
([0024](0024-peer-wire-claims-sign-the-eip-712-balance-proof.md),
[0053](0053-a-solana-claim-binds-its-domain-the-way-an-evm-claim-does.md)), so `vectors/wire-vectors.json`
must be regenerated and this is a **cross-repo change** ([0021](0021-vectors-are-normative-prose-is-not.md)):
`toon-client`, `rig` and `swap` replay those vectors. Regenerate with
`cargo run -p connector-vectors --bin generate-vectors`.

**What does not change:** a channel id stays a 32-byte opaque value to everything above the
settlement backend. Nothing on the packet path, in a claim's validation, or in the client edge learns
that it is now derivable. Code that treats it as opaque stays correct; only code that _constructs_
one is affected.

## The sweep

**Does not survive:**

- **`TokenNetwork.channelCounter`** and the dead `ChannelAlreadyExists` branch beneath it — replaced
  by `channelEpoch` and a live refusal.
- **The premise that a pair may hold several concurrent channels.** Anything relying on it — test
  fixtures that open twice between the same two addresses — must be rewritten. Solana already
  forbade it, so nothing cross-chain depended on the difference.

**Survives unchanged:**

- **[0024](0024-peer-wire-claims-sign-the-eip-712-balance-proof.md)** and
  **[0053](0053-a-solana-claim-binds-its-domain-the-way-an-evm-claim-does.md)** — a claim still signs
  the EIP-712 balance proof, still binds its domain, and still names the channel. The identifier's
  _derivation_ changes; what it is bound into does not.
- **[0005](0005-claims-are-truth-balances-are-a-projection.md)** — nonces, watermarks and the claim
  journal are per channel and are indifferent to how the channel was named.
- **The `Deposit`-credits-by-signer rule.** Funding stays a self-deposit on both chains: deriving a
  channel does not let anyone put collateral behind somebody else's claims.
- **`TokenNetworkRegistry`** — one `TokenNetwork` per token is what makes the token implicit in the
  derivation, and that structure is what this record relies on.

## Consequences

**`POST /peers` becomes structurally idempotent.** Repeating the same request derives the same
identifier, finds the channel it opened last time, and establishes the same peering. Retry safety
stops being a matter of care in the handler.

**Onboarding becomes symmetric.** Two operators who have each other's URL reach the same channel from
opposite directions with no exchange of identifiers. Neither has to go first, and neither has to be
told anything the other's self-description does not already carry.

**The two chains stop needing separate reasoning.** A rule that held on Solana by accident of PDA
design now holds on EVM by decision, and one sentence — _at most one live channel per pair per
token_ — is true of the protocol rather than of one backend.
