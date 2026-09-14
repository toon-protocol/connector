# Wire vectors: the invariants behind them

**Status:** **Live — the vector companion, role unchanged** (wayfinder map #1049, issue #1065).
Its Scope section is stale on three counts and is corrected as part of the vector-coverage work
(issue #1073): the committed set now also carries a `peer_carriage` section (20 dual-encoded entries,
several of them behavioural) and a `channel_control_declaration` section that nothing describes, and it
carries **no client-edge carriage section at all** despite this document's Scope claiming the client edge
as its subject. _Originally:_ Non-normative. Per [ADR 0021](../adr/0021-vectors-are-normative-prose-is-not.md), the
committed vector set (`vectors/wire-vectors.json`) is the cross-repo contract; this document only
names the invariants it is evidence of, written down before any vector was generated, per its own
acceptance criterion. A disagreement between this text and the vectors is a bug in this text.
**Consumers:** anyone regenerating or extending the vector set (`crates/connector-vectors`);
`toon-client`, `rig` and `swap`, as background for what the bytes they replay are supposed to mean.

## Scope

This covers the **client edge** termination wire (issue #498): the structured envelope
(`connector_domain::envelope`), the gift wrap sealing it (`connector_signer::giftwrap`), the
fulfilment a terminating connector derives from it (ADR 0019), and the EIP-712 `BalanceProof`
claim-signing scheme (`connector_signer::claim_signature`, ADR 0024). The claim scheme is included
even though it is also what the **peer semantics**'s claim exchange uses (`docs/protocol/
peer-semantics-pre-868.md` §3.5) — `connector_signer::claim_signature` is one implementation shared by
both wires, not two, and a client-edge claim (`client-edge-spec.md` §1.3 step 4) is checked against
exactly the same digest. Nothing else about the peer semantics is in scope here: it is
operator-to-operator on both ends (ADR 0003), already normative prose for a different reason, and
the rest of it is out of this issue's scope.

**The ILP packet's own encoding is in scope of the committed set, and was not in scope of this
document.** That is worth saying because it has been misread three times: the `peer_carriage`
fixtures added later carry complete OER `PREPARE`, `FULFILL` and `REJECT` packets
(`prepare.http_body_hex`, `fulfill_ack_accepted.packet_hex`, `reject_with_cost.packet_hex`), so
[ADR 0021](../adr/0021-vectors-are-normative-prose-is-not.md) has bound the packet bytes since
those landed, even though no section is named for them and no invariant below is written about
them. The encoding is **not** RFC 0027's — RFC 0027's semantics in TOON's own encoding
([ADR 0063](../adr/0063-the-ilp-packet-is-toons-dialect-not-rfc-0027s.md)) — and
[`vectors/README.md`](../../vectors/README.md#the-ilp-packet-encoding) is where a replaying SDK
finds the three divergences, the grammar and a byte-by-byte walk of the pinned PREPARE.

## Invariants

### 1. An envelope round-trips

Encoding an [`EnvelopeRequest`] or [`EnvelopeResponse`] and decoding the result returns exactly
the value that was encoded — for every value the type can hold, not only worked examples.

Held open by `connector-domain`'s `envelope::tests::any_request_round_trips` and
`any_response_round_trips` (arbitrary method/target/headers/body, arbitrary status), plus
`header_list_round_trips_exactly` (order and duplicate names both survive, since a header list is
encoded as a sequence, never a map).

### 2. Decode refuses malformed input, distinguishably and without panicking

`EnvelopeRequest::decode`/`EnvelopeResponse::decode` never panic on arbitrary bytes
(`decode_never_panics_on_arbitrary_bytes`), and every byte sequence either one accepts re-encodes
to exactly itself — no two distinct byte sequences decode to the same envelope
(`request_decode_accepts_only_bytes_that_reencode_to_themselves`,
`response_decode_accepts_only_bytes_that_reencode_to_themselves`). This is what makes the OER
length determinants canonical (ADR 0023): a non-minimal encoding, a zero-length long-form alias,
and an over-wide determinant are each refused with a distinct, named `EnvelopeError` variant
rather than accepted as a synonym for some other encoding of the same value or truncated into
one. A wrong type byte and trailing bytes after an otherwise-complete envelope are refused the
same way.

### 3. A sealed payload opens only under the intended key, in both directions

A request sealed with [`seal_request`] to a receiver's public key opens with [`open_request`]
under that receiver's own `Signer` and recovers exactly the plaintext and shared secret that were
sealed — and fails to open under any other identity's `Signer`, including one holding a
structurally valid but different key pair. A response sealed with [`seal_response`] under a
shared secret opens with [`open_response`] under that same secret and recovers exactly the
plaintext — and fails to open under any other 32 bytes. Neither direction ever needs a second key
exchange for the response: the secret the request carried is what seals the answer.

Held open by `connector-signer`'s `giftwrap::tests` module: the existing worked examples
(`a_sealed_request_opens_with_the_receivers_signer`,
`a_sealed_request_does_not_open_under_a_different_identity`,
`a_response_opens_with_the_shared_secret_from_its_request`,
`a_response_does_not_open_under_the_wrong_shared_secret`) plus new proptest properties over
arbitrary plaintext (`any_plaintext_round_trips_through_seal_and_open_request`,
`any_plaintext_round_trips_through_seal_and_open_response`) that generalize them past fixed
byte strings.

### 4. A derived fulfilment is deterministic and secret-specific

For any shared secret, `derive_fulfillment(secret)` — the HKDF output
`connector_signer::giftwrap::derive_fulfillment` produces — is the same value every time it is
computed from that secret, and a different secret produces a different fulfilment. A terminating
connector derives its answer this way (ADR 0019); the sender's own end-to-end check
(`connector send`) compares a returned fulfilment against this same derivation over its own
sealed secret, which is what catches a forged delivery now that the packet carries no execution
condition to check it against instead (issue #1269 / ADR 0069).

Held by `giftwrap::tests::derive_fulfillment_is_deterministic_for_the_same_secret` (any secret) and
`giftwrap::tests::a_terminating_connector_derives_the_same_fulfillment_the_sender_would`, which ties
determinism to a genuine `seal_request`/`open_request` pair rather than to a secret handed to both
sides out of band.

### 5. A claim's EIP-712 `BalanceProof` digest recovers to its signer, under its own domain

`connector_signer::claim_signature::evm_balance_proof_digest` is deterministic for the same
fields, and `verify_evm_balance_proof` accepts a signature over that digest only from the address
that actually produced it — changing any one field (`channel_id`, `nonce`, `transferred_amount`,
`chain_id`, or `token_network_address`) invalidates a prior signature rather than being silently
tolerated. This is the scheme both the peer semantics (`ClaimBook::accept_inbound`) and the client edge
(`client-edge-spec.md` §1.3 step 4) check a claim's signature against, replacing a SHA-256 tuple
(`connector_domain::claim_digest`, removed by #575/#583) that no chain ever verified.

Held open by `connector-signer`'s `claim_signature::tests` module:
`a_genuine_evm_signature_verifies_against_its_signers_address`,
`an_evm_signature_does_not_verify_against_a_different_partys_address`,
`changing_any_evm_proof_field_invalidates_a_prior_signature` (covers every field, including the
domain's `chain_id`/`token_network_address`), and `the_evm_digest_is_deterministic`.

## Generation

`crates/connector-vectors` builds the committed set from **fixed literal fixtures** — hardcoded
keys, secrets, nonces and payloads, not values sampled anew each run — and self-verifies each
entry against the same functions these invariants name (an envelope vector is decoded back and
compared before being serialized; a giftwrap vector is opened back with the receiver's own signer;
a fulfilment vector's two secrets are checked to derive two different fulfilments; a claim
vector's signature is checked against `verify_evm_balance_proof`) before writing it out.
Regenerating (`cargo run -p connector-vectors --bin generate-vectors`) against an
unchanged implementation is therefore a no-op — same fixtures through the same code always produce
the same data — and `cargo test -p connector-vectors` is the gate: it regenerates the set in
memory and fails if its _data_ (compared as parsed JSON, not raw bytes — this repo's pre-commit
hook reformats staged JSON with `prettier`, which carries no data of its own) no longer matches
`vectors/wire-vectors.json` on disk, so a change to the wire that does not regenerate the committed
vectors fails `cargo test --workspace`.

See `vectors/README.md` for the file's schema, aimed at a reader in another repository who is not
importing any Rust from this one.

[`EnvelopeRequest`]: ../../crates/connector-domain/src/envelope.rs
[`EnvelopeResponse`]: ../../crates/connector-domain/src/envelope.rs
[`seal_request`]: ../../crates/connector-signer/src/giftwrap.rs
[`open_request`]: ../../crates/connector-signer/src/giftwrap.rs
[`seal_response`]: ../../crates/connector-signer/src/giftwrap.rs
[`open_response`]: ../../crates/connector-signer/src/giftwrap.rs
