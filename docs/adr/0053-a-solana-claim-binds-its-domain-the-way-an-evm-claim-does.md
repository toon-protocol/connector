# A Solana claim binds its domain, the way an EVM claim already does

**Status:** Accepted and **built** (see the Update below; the wire change landed with issue #1082). Extends [0024](0024-peer-wire-claims-sign-the-eip-712-balance-proof.md) to the second settlement chain [0002](0002-drop-mina-from-the-rust-connector.md) kept. Supersedes the reading under which issue #975 is a missing check. **Disturbed by [0059](0059-a-channel-is-derived-from-its-participants.md)** — the channel account is already a participant-derived PDA, so what moves is EVM's side and the vectors, not this record's decision. **Extended by [0074](0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md)**: the client edge gains a second Solana claim scheme, payment-channels' 50-byte Ed25519 voucher, whose domain is bound through the channel PDA and the program id the connector configures.

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

**A Solana claim's signature covers the chain and the program its channel lives on, not only the
channel account.** Until it does, a claim is bound to an account and to nothing else, and a signature
valid on one cluster is valid on every cluster where that account exists.

## The asymmetry

[ADR 0024](0024-peer-wire-claims-sign-the-eip-712-balance-proof.md) chose EIP-712 for EVM claims
specifically so a claim commits to more than its own amounts. Its `BalanceProof` digest covers
`channel_id`, `nonce` and `transferred_amount` — **and `chain_id` and `token_network_address`, through
the domain separator.** Changing any one of them invalidates a prior signature, which is exactly what
the record's committed vector holds open.

The Solana scheme, `connector_signer::claim_signature::solana_balance_proof_message`, is 48 raw bytes:

| bytes  | field                                   |
| ------ | --------------------------------------- |
| 0..32  | `channel_account`                       |
| 32..40 | `nonce`, u64 little-endian              |
| 40..48 | `transferred_amount`, u64 little-endian |

**No cluster. No program id. No token.** A claim binds to an account and to nothing about where that
account lives.

## Why issue #975 is not a missing check

[Issue #975](https://github.com/toon-protocol/connector/issues/975) reads: _"a Solana claim's declared
`cluster` is never checked against the node's chain, so a mainnet payment can record itself as
devnet."_

**The `cluster` is declared in the claim JSON and is not signed over.** Adding a check on it would
catch an honest misconfiguration and nothing else: a forger declares whichever cluster the receiver
expects, and the signature still verifies over the same 48 bytes. A check on an unsigned field
_reads_ like the EVM property and does not provide it, which is worse than a stated gap — it is the
appearance of protection.

Whatever cluster separation exists today is a property of program ids happening to differ between
deployments, not of the signature. That is a deployment accident standing in for a cryptographic
guarantee.

## Decision

**The signed message is extended to bind the domain**: the settlement program's id, and the cluster,
alongside the channel account, nonce and transferred amount. The exact encoding is the implementing
issue's to fix; what this record binds is that **a Solana claim's signature must not verify against a
channel account alone**.

A verifier selects the scheme from the claim's declared `blockchain` field, as today — that field is a
routing hint, never a security boundary, and it stays one. What changes is that the bytes underneath
it commit to the chain the verifier is actually on.

## Consequences

**This is a breaking wire change.** The committed `peer_carriage.claim_solana` vector changes;
`toon-client`, `rig` and every other Solana-paying client must move together.

**It is more affordable now than it will ever be again.** Solana mainnet is not live —
[issue #834](https://github.com/toon-protocol/connector/issues/834) is still scoping the mainnet deploy
path and the program-id promotion — so the cross-cluster case this protects against is precisely the
one that has not shipped. And [issue #1038](https://github.com/toon-protocol/connector/issues/1038)
already exists to migrate live channels onto the locking contract, so there is a migration in flight
to ride rather than one to invent.

**ADR 0024's title and reasoning now cover both chains.** Its record stays EVM-specific, as dated
evidence should; this one is the sibling rather than an amendment, because a second signature scheme
is a new decision and not a clarification of the first.

**`peer-semantics-pre-868.md` §3.5 says there is no Ed25519 claim path** (sweep finding F-34). That
file is frozen history (issue #1065) and is not corrected; the statement was true when written and the
vector set has carried `claim_solana` since.

**Scope note against [0052](0052-permissionless-payment-is-guaranteed-and-a-claim-is-what-authorises.md).**
"Unverifiable is never accepted, by configuration, flag or build profile" concerns whether a claim's
signature can be checked against this connector's own record of the channel. A claim that verifies but
binds too little is a different failure, and this record is what closes it. The two should not be read
as one rule.

## Update (issue #1146): built — and this record's Status line was three issues stale

**This record said "not yet built" for four months after the code landed.** It is built, and was
already built when [issue #1146](https://github.com/toon-protocol/connector/issues/1146) was written
against it — that issue's original "do not build this before ADR 0053" sequencing was derived from
this line and has been retracted in its comments. `docs/adr/README.md`'s row agreed with the stale
line and is corrected alongside it.

What exists, as of issue #1082:

`connector_signer::solana_balance_proof_message` is **96 bytes**, not the 48 described above:

| bytes  | field                                   |
| ------ | --------------------------------------- |
| 0..16  | `TOON-BALPROOF-V2`, the domain tag      |
| 16..48 | `program_id` — the settlement program   |
| 48..80 | `channel_account`                       |
| 80..88 | `nonce`, u64 little-endian              |
| 88..96 | `transferred_amount`, u64 little-endian |

`packages/solana-program/src/processor.rs` and `crates/connector-settlement-solana/src/wire.rs` each
carry the same tag, each commented to stay byte-identical with the others, and
`vectors/wire-vectors.json` carries the changed `peer_carriage.claim_solana` the Consequences section
promised.

**Two details of the Decision were settled differently, and deliberately.** The Decision asks for
"the settlement program's id, **and the cluster**". The cluster is **not** in the message, because a
Solana program knows its own id and nothing about which cluster it runs on — it could not rebuild the
message to compare against, so a cluster in the signed bytes would be unverifiable exactly where it
would need to be verified. A claim's declared `cluster` therefore stays what this record calls it
throughout: a routing hint, never a security boundary, compared off chain against the node's own
`[settlement.solana] rpc_url` (issues #975/#976). And the construction is **domain-separated rather
than appended**: an appended field is silently truncatable by a verifier expecting the old length,
whereas a 48-byte prefix of this message is not a valid message under either scheme.

**What it enables, and why this Update is written now.** ADR 0042's send half could not be built for
Solana while this was thought unbuilt: a covering payer proactively signing on the 48-byte format
would have been minting claims valid on every cluster where the account existed — the deployment
accident this record names. Issue #1146 built that send half on top of these bytes.
