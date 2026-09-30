# Wire vectors: the invariants behind them

**Status:** **Live — the vector companion, role unchanged** (wayfinder map #1049, issue #1065).
**`schema_version` 8** (issue #1429, ADR 0074 decision 3 amended 2026-09-30): an EVM voucher's
`maxClaimableAmount` widens from `u64` to `u128`, and every `max_claimable_amount` field becomes a
decimal string rather than a JSON number — invariant 7's own paragraph has the detail. Before that,
**`schema_version` 7** (issue #1384, [ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)
decision 14): every claim is an x402 voucher, and every `toon-channel` section — the EIP-712
`BalanceProof` claim, the peer carriage's `toon-channel` claims, FLUSH and nonce cases, and the
`TokenNetwork`-domain `channel_control_declaration` — is gone from the committed set. Its Scope
section was stale and is corrected as part of the vector-coverage work (issue #1073): the committed
set carries a `peer_carriage` section (dual-encoded entries, several of them behavioural), and a
client-edge carriage section only for the `payout-claim` TRANSFER (`payout_voucher`). Issue #1408
added the client BTP auth frame's `channelChallenge` (`client_auth_channel_challenge`, invariant 11)
and the `POST /ilp/claim-state` `"toon-channel-refused"` answer (`claim_state_toon_channel_refused`,
invariant 12) — two elements decision 14 named but #1384 had not yet pinned — purely additively, so
`schema_version` stayed 7. _Originally:_
Non-normative. Per [ADR 0021](../adr/0021-vectors-are-normative-prose-is-not.md), the
committed vector set (`vectors/wire-vectors.json`) is the cross-repo contract; this document only
names the invariants it is evidence of, written down before any vector was generated, per its own
acceptance criterion. A disagreement between this text and the vectors is a bug in this text.
**Consumers:** anyone regenerating or extending the vector set (`crates/connector-vectors`);
`toon-client`, `rig` and `swap`, as background for what the bytes they replay are supposed to mean.

## Scope

This covers the **client edge** termination wire (issue #498): the structured envelope
(`connector_domain::envelope`), the gift wrap sealing it (`connector_signer::giftwrap`), the
fulfilment a terminating connector derives from it (ADR 0019), and — since ADR 0075 made it the only
claim on either edge — the x402 `batch-settlement` **voucher** (`connector_signer::voucher_signature`,
ADR 0074), with the challenge that proves control of a voucher channel without moving value. The
voucher is in scope on the peer carriages too: one parser (`parse_client_claim`) and one verifier
serve both edges, so the peer carriage's voucher, peer-role challenge and refusal cases pin the same
scheme. The retired `toon-channel` claim appears only as what every edge refuses by name.

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

### 5. A `toon-channel` claim is refused by name, everywhere

[ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md) decision 8,
issue #1384: a claim's `scheme` is required. A claim with no `scheme`, or with
`scheme: "toon-channel"`, is the retired `toon-channel` claim (the EIP-712 `BalanceProof` of ADR
0024 and the Solana message of ADR 0053, both retired by ADR 0075), and every edge refuses it **by
name** rather than as malformed: the client edge's parser (`ClientClaimError::ToonChannel`), the BTP
peer carriage with an ERROR frame (`F00`, `NotAcceptedError`, the refusal text as `data`), and the
HTTP peer carriage with a `400` whose body is that text — each before the role is decided. `mina` is
refused by name the same way, and first.

Held open by `connector-domain`'s `client_claim::tests` (including a proptest that no `scheme` or
`"toon-channel"` is always refused), `connector-peer-btp`'s `claim_json::tests`, and pinned cross-repo
by the `toon_channel_refused` section, whose generator runs every case through the real parser, the
BTP evidence reader and the HTTP evidence reader before committing it.

### 6. A route's charge is `base + per_kib * ceil(payload_len / 1024)`, saturating

`connector_domain::Price::charge` counts kibibytes **started** — whole kibibytes plus one for any
remainder, and none at all for an empty payload, which pays the base alone. A flat price is the
same value as a zero-slope schedule (ADR 0065), so it charges its base at every length. The
arithmetic saturates rather than wrapping or panicking: an operator can write a slope that exceeds
a `u64` on a large payload, and the answer is then `u64::MAX` — a charge no claim can cover, which
refuses the packet.

**This invariant is arithmetic, not an encoding, and it is in the committed set anyway.** The
justification is the same one that puts the ILP packet's bytes in scope: `payload_len` is
`Prepare.data.len()`, a property of carriage rather than of content, measurable by every hop
without opening the gift wrap. That is exactly what lets a sender compute its own charge before it
sends, a forwarded route be priced at the client edge (ADR 0028) and a peer arrival be gated (ADR 0029) — one schedule, one number, four implementations of it across this repo, `toon-client`,
`rig` and `swap`. Binding those four with prose alone was tried and failed: `toon-client`'s
`chargeFor` computed `floor(len / 1024) + 1` and overpaid by a whole kibibyte at every exact
multiple of 1024 (toon-client#629), with the correct rule stated in prose three lines above the
code contradicting it.

Held open by `connector-domain`'s `price::tests` — `a_started_kibibyte_is_a_whole_one`,
`an_empty_payload_charges_the_base_alone`, `charging_saturates_rather_than_panicking`, and the
proptests `a_whole_kibibyte_charges_for_exactly_that_many` (which pins both `kib * 1024` and one
byte past it), `charging_never_falls_as_a_payload_grows` and `charging_is_total` — and pinned
cross-repo by the `charge` section, whose generator re-derives every row in checked longhand
arithmetic and asserts it against `Price::charge` before committing it.

### 7. A voucher's freshness is amount-only, and strict

[ADR 0074](../adr/0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md) decision 3, issue
#1347: a client-edge claim under `scheme: "batch-settlement"` — a **voucher** — carries no nonce, so
`connector_domain::validate_voucher` judges its freshness by cumulative amount alone — the only
freshness rule since ADR 0075 deleted the nonce rules. An amount **equal** to the channel's
watermark is refused (`amount_not_advancing`) unless it is byte-identical — same amount, same
signature — to the voucher that set that watermark, in which case it is a retransmission: accepted
again, buying nothing new — and, because it buys nothing, refused as an underpayment where the
charge is not zero. A strictly
higher amount is accepted, by the difference. Also pinned: a Solana voucher's `expiresAt`
must be `0` — x402 requires it, and the program refuses a nonzero one at `settle` with no state
change — so the connector refuses it structurally, before any signature check.

**Amended 2026-09-30 (#1429, `schema_version` 8):** an EVM voucher's `maxClaimableAmount` is
`uint128` on the wire and `u128` in the connector, matching `x402BatchSettlement`'s own type —
nothing above `u64::MAX` is refused any more. Solana's own amount stays `u64`, since the signed
Solana message and SPL amounts are `u64` on chain, so a Solana voucher above `u64::MAX` is still
refused. `claim_voucher` gains `evm_above_u64_max`, a `uint128` amount above `u64::MAX`, signed and
verifying. Every `max_claimable_amount` in `claim_voucher`, `peer_carriage` and `payout_voucher`
becomes a decimal string rather than a JSON number, for both chains, since a value that wide would
lose precision read back as an IEEE double.

Held open by `connector-domain`'s `claim::tests` module (the `voucher_*` property and worked-example
tests over `validate_voucher`) and by `connector-signer`'s `voucher_signature::tests` (the EIP-712
digest and Ed25519 message each voucher scheme signs, checked against the deployed
`x402BatchSettlement` contract and against `payment-channels`' own message layout) — and pinned
cross-repo by the `claim_voucher` section, whose `evm`/`solana` cases are checked against
`x402BatchSettlement.getChannelId`/`.getVoucherDigest` on Base Sepolia — and again on every
workspace run, by `connector-settlement-evm`'s `x402_voucher_vector.rs`, against the deployed
bytecode on an `anvil` at chain 84532 — and whose
`amount_only_watermark`/`invalid` cases are checked against the real `validate_voucher` and claim
parser before being committed. The same `x402_voucher_vector.rs` also checks
`peer_carriage.voucher_evm` and `payout_voucher.evm` — every EVM voucher the set pins — against the
contract's own `getChannelId`/`getVoucherDigest`.

### 8. A voucher channel's claim-state challenge is its voucher signer's, and not a voucher

Issue #1364: `POST /ilp/claim-state` answers for an x402 `batch-settlement` channel only against a
signature by the key the chain checks that channel's vouchers against — never a key the request
names — over a challenge no voucher can stand in for: on EVM the
`ClaimStateChallenge` struct under `x402BatchSettlement`'s domain, on Solana a message tagged
`toon-voucher-claim-state-challenge-v1`. Held open by `connector-signer`'s
`claim_state_challenge::tests` (each challenge against a voucher and against the other scheme's
challenge), by `connector-client-edge`'s `voucher_claims.rs` (the endpoint over the real gate), and
pinned cross-repo by the `voucher_claim_state_challenge` section.

### 9. A peering pays in vouchers, and a zero-value packet proves itself with the challenge

ADR 0075 decisions 5 and 6: a paying node covers every forwarded PREPARE with a voucher on its own
outbound channel, signed by its chain's settlement key (`payerAuthorizer == payer`), rendered as the
client edge's own voucher JSON and carried in the claim slot — raw UTF-8 in the BTP
`payment-channel-claim` entry, base64 in the `Payment-Channel-Claim` header. A packet that moves no
value carries **no voucher**, and carries the voucher claim-state challenge (invariant 8's message)
in a slot of its own — the `peer-role-challenge` entry, the `Toon-Peer-Role-Challenge` header —
signed by the same key; a challenge is never a voucher. Held open by the peer carriages' own tests
and `connector-runtime`'s covering tests, and pinned cross-repo by `peer_carriage.voucher_evm`,
`voucher_solana`, `prepare` and `zero_value_challenge`, each rendered by the paying side's own
functions (`voucher_json`, `challenge_entry`) and read back by the receiving side's evidence readers
on both carriages before it is committed.

### 10. A payout is a voucher the client can land by itself

ADR 0075 decision 7: a connector pays a client with a voucher on its own outbound channel toward the
client's payee key, carried in a BTP TRANSFER's `payout-claim` entry whose `amount` is the voucher's
cumulative amount. The entry is the voucher claim JSON less its envelope, and on EVM always carries
the `channelConfig` landing needs. Pinned by the `payout_voucher` section, whose generator restores
the envelope and parses each case with the client edge's own parser, and verifies each signature.

### 11. A client declares a channel at BTP auth with a genuinely signed challenge, and the 300-second bound alone decides whether it is heard

ADR 0075 decision 5, issue #1408: the client BTP `auth` entry's `channelChallenge` field — invariant
8's message, riding in its own slot beside `peerId`/`secret` — declares a channel before its session
has paid anything. Extraction never depends on the window: a challenge that is expired, or signed too
far ahead, still parses, exactly as one within the bound does. What decides whether it is heard is
`expires` against the connector's clock alone — ahead, and no more than `MAX_PEER_CHALLENGE_LIFETIME_SECS`
(300 seconds) ahead — the same bound a peer's challenge is held to (invariant 9). A challenge outside
it is refused quietly: the session still binds, and simply learns no payee at auth.

Held open by `connector-client-edge`'s `btp::tests` module (`a_channel_challenge_at_auth_credits_a_session_that_never_paid`,
`a_channel_challenge_that_does_not_hold_teaches_nothing`), and pinned cross-repo by the
`client_auth_channel_challenge` section, whose generator signs each challenge genuinely — checked
against `verify_evm_voucher_claim_state_challenge`/`verify_solana_voucher_claim_state_challenge` —
and varies only `expires` relative to a fixed `now` between an accepted and a refused case, each
read back by the real BTP frame decoder and the client edge's own `auth_channel_challenge` extractor
and `challenge_in_window` check before being committed (`connector-client-edge`'s
`btp::tests::the_committed_client_auth_channel_challenge_vectors_match_the_real_parser`).

### 12. A claim-state entry naming a retired scheme is refused by name, and costs no lookup

ADR 0075 decision 8, issue #1408: `POST /ilp/claim-state` requires `scheme` exactly as a claim does
(invariant 5's `declared_scheme`). An entry with no `scheme`, or with `scheme: "toon-channel"`, asks
about the retired `toon-channel` channel and is answered `"toon-channel-refused"` by name — distinct
from invariant 5's refusal of a claim riding a PREPARE — before the settlement backend is ever asked,
even for a channel its own voucher signer genuinely controls. The response's channel field is always
`channelId`, on both chains: a Solana refusal's `channelId` carries the channel account's base58 text,
since the endpoint's one refused-entry shape has no separate `channelAccount` field.

Held open by `connector-client-edge`'s `claim_state::a_toon_channel_entry_is_refused_by_name_without_a_lookup`,
and pinned cross-repo by the `claim_state_toon_channel_refused` section, checked against
`connector_domain::client_claim::declared_scheme` — the same function the endpoint branches on —
before being committed, and replayed against the real route, over a backend admitting nothing, by
`connector-client-edge`'s `claim_state::the_committed_claim_state_toon_channel_refused_vectors_match_the_real_endpoint`.

## Generation

`crates/connector-vectors` builds the committed set from **fixed literal fixtures** — hardcoded
keys, secrets, nonces and payloads, not values sampled anew each run — and self-verifies each
entry against the same functions these invariants name (an envelope vector is decoded back and
compared before being serialized; a giftwrap vector is opened back with the receiver's own signer;
a fulfilment vector's two secrets are checked to derive two different fulfilments; a voucher
vector's signature is checked against `verify_evm_voucher`/`verify_solana_voucher`, and every peer
and refusal case is read back by the carriages' own evidence readers) before writing it out.
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
