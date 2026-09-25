# Peer-wire claims sign the EIP-712 balance-proof digest, not a connector-internal SHA-256 tuple

**Status:** Accepted. Its own inline note already records that `SettlementChannel.sol` was deleted (#578, #589) and `connector-settlement-evm` retargeted at the deployed `TokenNetwork`. `connector_domain::claim_digest` is gone from `crates/`, as this record required. **Disturbed by [0059](0059-a-channel-is-derived-from-its-participants.md)** — the channel identifier a claim binds becomes derivable from the participants; what a claim signs is unchanged, but the vectors move. **Extended by [0074](0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md)**: the client edge gains a second EVM claim scheme, x402's EIP-712 `Voucher(channelId, maxClaimableAmount)`. The peer wire is untouched.

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

`ClaimBook` (`crates/connector-runtime/src/claim.rs`) now signs and verifies every EVM peer-wire
claim over `connector_signer::evm_balance_proof_digest` -- the same EIP-712 `BalanceProof` digest
`packages/contracts/src/TokenNetwork.sol` recovers on redemption -- instead of
`connector_domain::claim_digest`, a SHA-256 hash of `len(channel_id) ‖ channel_id ‖ nonce ‖
cumulative_amount` that no chain has ever verified. `claim_digest` is deleted, not kept as a
second answer to the same question.

> **Since this was written.** `SettlementChannel.sol`, referred to below in the present tense, has
> been deleted (#578, #589). `connector-settlement-evm` was rewritten against the deployed
> `TokenNetwork`, reached through a `TokenNetworkRegistry` via `getTokenNetwork(token)` — the same
> contract whose typehash this ADR's digest is computed for. The gap it describes ("nothing had
> ever redeemed a peer-wire claim on chain", against a contract "that does not check a claim's
> signature at all") is therefore closed: the redemption target now verifies the signature this
> ADR made the wire produce. The decision itself is unchanged.

## The disagreement this records

`docs/protocol/peer-semantics-pre-868.md` §3.5 already said, before this issue, that an `evm` claim's
signature is _"ECDSA over the EIP-712 balance-proof digest"_. The implementation disagreed with
its own specification: `ClaimBook::record_fulfillment`/`accept_inbound` signed and verified a
SHA-256 tuple instead. This was invisible because nothing had ever redeemed a peer-wire claim on
chain -- `connector-settlement-evm` targets a different, unrelated contract
(`SettlementChannel.sol`, tracked separately, issue #566) that does not check a claim's signature
at all. Recorded here rather than silently corrected: the spec was right and the code was wrong,
not the other way around, and the two are not in tension going forward -- §3.5's own wording is
unchanged by this issue.

## The decision this records

- **Reuse, don't reimplement.** `connector_signer::evm_balance_proof_digest`
  (`crates/connector-signer/src/claim_signature.rs`) already computes exactly this digest, written
  for the client edge's own claim verification (issue #506/#510) and already matching the domain
  read from the live `TokenNetwork` on Base Sepolia (issue #566). The peer wire is pointed at that
  one implementation rather than growing a second. `connector-domain` keeps its ADR 0001 "no
  dependencies" shape -- the digest lives in `connector-signer`, which both the client edge and now
  the peer wire depend on, not in the dependency-free domain crate.
- **The EIP-712 domain (`chainId`, `verifyingContract`) is a configured input, per channel.** It is
  per-token, not per-chain -- each token gets its own `TokenNetwork` and therefore its own
  `verifyingContract` (issue #566) -- and it is deliberately **not** read from a settlement backend.
  `ClaimBook::set_channel_domain` is the one place a channel's domain enters the system; a channel
  with none configured produces or accepts no claim at all, exactly like a node with no signer
  configured never emits one. This is what lets this change land, and be reviewed, independently of
  #576's settlement backend retarget.

  > **Amended by #1136, decision unchanged.** The domain is still a configured input, per channel,
  > and is still not _read_ from a settlement backend. It is now **corroborated** against one:
  > `connector_cli::runtime`'s `check_evm_channel_domains` holds every declared
  > `[[peer_channels]]`, `[[pay_channels]]` and `[[client_channels]]` domain against the
  > `TokenNetwork` `EvmSettlementBackend::connect` resolved, and a disagreement refuses the boot.
  > Until then nothing compared the two at all, so a row left stale after a redeploy produced a
  > node that accepted claims under one `TokenNetwork` while redeeming through another — silent,
  > and in the paying direction. Deriving the value instead was rejected: it would contradict this
  > clause, and it would leave one source with no cross-check, so a mistyped `[settlement.evm]
token_address` would silently re-domain every channel rather than being caught. The model is
  > `[settlement.evm] decimals` (#564), which is likewise declared in config and refused at connect
  > when the chain disagrees — not the Solana `program_id` removals (#981/#1082/#1128), which
  > deleted a copy of a value the same file already stated.

- **The channel id a claim signs over must already be the on-chain `bytes32`.**
  `ClaimBook::set_channel_domain` parses the configured channel id as either `0x`-prefixed (or
  bare) 64-character hex -- `TokenNetwork.sol`'s own `channelId` shape -- or a plain decimal
  numeral, embedded as the big-endian bytes of that integer -- the shape a `uint256` channel
  counter takes on `SettlementChannel.sol` and this workspace's own `InMemorySettlementBackend`
  fake. Both are exact, lossless encodings of the value the string already names; anything else is
  refused at configuration time rather than hashed or truncated into a `bytes32` that names nothing
  real on any chain.
- **`lockedAmount`/`locksRoot` are still hashed as zeros.** They are dead per ADR 0004, but they
  remain in the deployed `TokenNetwork` typehash, and omitting them would compute a digest the
  signer's wallet never actually signed -- the same reasoning `claim_signature.rs`'s own doc
  comment already gives for the client edge's identical claim shape.
- **The wire encoding of `WireClaim` is unchanged.** `channel_id` stays the length-prefixed
  `String` it always was; only what it is checked to _mean_ (and what gets fed into the digest,
  never the raw UTF-8 bytes) changed. A claim's signature scheme changing is still a peer-wire
  change in substance -- a payload signed under the old scheme no longer verifies -- and is called
  out to the cross-repository clients tracked by issue #534, even though no field width moved.

## Consequences

A claim this connector signs now recovers, through `connector_signer::verify_evm_balance_proof`,
to this connector's own address -- and does not recover under a different `chainId` or
`verifyingContract`. Redeeming a peer-wire claim against a real `TokenNetwork` is not this
decision's job: that is issue #577, which waits on both this and the settlement backend retarget
(#576) before a Rust-signed claim can be proven to redeem on the deployed contract.
