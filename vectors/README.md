# Wire vectors

`wire-vectors.json` is the cross-repo contract for the client-edge termination wire (issue #527,
[ADR 0021](../docs/adr/0021-vectors-are-normative-prose-is-not.md)): reproducing these bytes is
what conformance means for `toon-client`, `rig` and `swap`. It is generated, not hand-written --
see `crates/connector-vectors` and `docs/protocol/wire-vectors.md` for the invariants each section
is evidence of. This file is plain JSON so a client SDK can replay it without importing anything
from this repository.

Regenerate after any change to the envelope (`connector_domain::envelope`), the gift wrap
(`connector_signer::giftwrap`), the fulfilment derivation
(`connector_signer::giftwrap::derive_fulfillment`), or the claim signing scheme
(`connector_signer::claim_signature`):

```
cargo run -p connector-vectors --bin generate-vectors
```

`cargo test -p connector-vectors` (part of the workspace gate) fails if this file is stale --
regenerating it against an unchanged implementation is a no-op, so a diff here always means the
wire actually changed.

## The ILP packet encoding

**This connector's ILPv4 packet is not byte-compatible with RFC 0027, and never has been.** The
semantics are Interledger's; the encoding is TOON's own dialect, ratified by
[ADR 0063](../docs/adr/0063-the-ilp-packet-is-toons-dialect-not-rfc-0027s.md). An encoder written
from RFC 0027's §Packet Format will not produce bytes this connector accepts, and a packet this
connector emits will not decode in a conforming ILPv4 implementation. Four things differ, and
nothing else does:

| RFC 0027 §Packet Format                                 | This connector                                 |
| ------------------------------------------------------- | ---------------------------------------------- |
| Outer type-length wrapper: `type` then a VarOctetString | Type byte, then fields inline — no wrapper     |
| `amount` is a fixed `UInt64` (8 bytes)                  | VarUInt (the `envelope` section defines it)    |
| `expiresAt` is a 17-byte Interledger Timestamp          | 19-byte GeneralizedTime, `YYYYMMDDHHMMSS.fffZ` |
| `executionCondition`, a 32-byte `UInt256`               | Gone (issue #1269); a one-byte `greeting` flag |

The fourth is a semantic departure, not an encoding one: RFC 0027 requires an execution condition
on every PREPARE, and this connector's PREPARE carries none at all -- see
[ADR 0069](../docs/adr/0069-the-execution-condition-leaves-the-wire.md). Everything else is RFC
0027's: the three type bytes, the remaining field order and meanings, `Fulfill.fulfillment` and its
relation to a request's shared secret (ADR 0019), and the `F`/`T`/`R` error taxonomy. RFC 0027's
reason for `amount`/`expiresAt` being fixed-length is that a forwarding connector can then rewrite
them in place; this connector forgoes that and re-encodes (ADR 0063 D4).

### Where it is pinned

In the `peer_carriage` section below, which is where the packet bytes have lived since those
vectors landed — the fixtures are a peer-carriage example, but the OER packet inside each one is
this contract, and [ADR 0021](../docs/adr/0021-vectors-are-normative-prose-is-not.md) makes it
binding:

| Packet    | Fixture                                                                               |
| --------- | ------------------------------------------------------------------------------------- |
| `PREPARE` | `peer_carriage.prepare.http_body_hex` (and byte-identically inside `btp_message_hex`) |
| `FULFILL` | `peer_carriage.fulfill_ack_accepted.packet_hex`                                       |
| `REJECT`  | `peer_carriage.reject_with_cost.packet_hex`                                           |

`prepare_no_claim` carries the same PREPARE bytes with the claim removed, and
`forwarded_data_unchanged` carries a different PREPARE whose `data` is a real sealed gift wrap.
There is no separate top-level `packet` section: replay these.

### The grammar

Written in the same style as the `envelope` production in the Schema section below, with `VarUInt`
and `VarOctetString` as defined there:

```text
prepare = 0x0c || VarUInt(amount)
             || GeneralizedTime(expires_at)      -- 19 ASCII bytes, no length prefix
             || greeting (1 byte: 0x00 or 0x01)
             || VarOctetString(destination)      -- UTF-8 ILP address
             || VarOctetString(data)

fulfill = 0x0d || fulfilment (32 bytes, no length prefix)
             || VarOctetString(data)

reject  = 0x0e || code (3 ASCII bytes, no length prefix)
             || VarOctetString(triggered_by)     -- UTF-8, may be empty
             || VarOctetString(message)          -- UTF-8
             || VarOctetString(data)
```

There is no length prefix around the whole packet and no trailing anything: a decoder that has
consumed the last field must be at the end of the buffer, or the packet is `trailing_bytes`
(ADR 0023). A REJECT's `accumulated_cost` is **not** in these bytes — it rides beside the packet,
as the `TOON-Accumulated-Cost` header or the `toon-accumulated-cost` protocol-data entry (ADR
0011); see `reject_with_cost`'s own `accumulated_cost` field.

### `peer_carriage.prepare.http_body_hex`, byte by byte

77 bytes (31 fewer than before issue #1269 deleted `executionCondition`). The decoded values are
`peer_carriage.prepare.prepare` in this file, so a replaying SDK can check both directions: that
these bytes decode to those values, and that encoding those values reproduces these bytes exactly.

```text
0c                                 type 12 = PREPARE
83                                 VarUInt determinant: long form, 0x80 | 3 -> 3 value bytes follow
   03d090                          amount = 250000
                                     (RFC 0027 would be 8 fixed bytes: 000000000003d090)
32303330303130313030303130302e3030305a
                                   expires_at, 19 ASCII bytes = "20300101000100.000Z"
                                     = 2030-01-01T00:01:00.000Z
                                     (RFC 0027 would be 17 bytes: "20300101000100000")
00                                 greeting = false
                                     (RFC 0027 has no such field, and has a 32-byte
                                     executionCondition here instead -- issue #1269 / ADR 0069)
17                                 VarUInt determinant: short form, 23 bytes follow
   672e746f6f6e2e73746f72652d626f782e736574746c65
                                   destination = "g.toon.store-box.settle"
1b                                 VarUInt determinant: short form, 27 bytes follow
   766563746f722d666978747572652d707265706172652d64617461
                                   data = "vector-fixture-prepare-data"
```

The fixture's `data` is plain ASCII so the walk stays readable. A **real** packet's `data` is a
gift wrap sealed to the terminating connector (ADR 0018) and is opaque to every hop — see
`forwarded_data_unchanged`, whose `sealed_data_hex` is a genuine one and must appear byte-for-byte
inside its PREPARE.

The other two, for completeness:

```text
0d                                 type 13 = FULFILL
5152535455565758595a5b5c5d5e5f60
6162636465666768696a6b6c6d6e6f70   fulfilment, 32 bytes, no length prefix
1b 766563746f722d666978747572652d66756c66696c6c2d64617461
                                   data = "vector-fixture-fulfill-data"   (61 bytes total)

0e                                 type 14 = REJECT
543034                             code = "T04", 3 ASCII bytes, no length prefix
10 672e746f6f6e2e73746f72652d626f78
                                   triggered_by = "g.toon.store-box"
15 766563746f7220666978747572652072656a656374
                                   message = "vector fixture reject"
00                                 data, empty                            (44 bytes total)
```

## Schema

All byte fields are lowercase hex, no `0x` prefix. `schema_version` bumps only when a field's
meaning changes in a way that would make existing replay code misread it; a purely additive field
does not bump it.

### `envelope`

The structured request/response envelope every terminated packet carries once opened
(`docs/protocol/client-edge-spec.md` §1.8).

**Encoding** (everything a replaying SDK needs to _produce_ `encoded_hex`, not merely check it).
Two OER primitives (RFC-0030), both defined in `connector_domain::oer`:

- **VarUInt** — a value in `0..=127` is that single byte. Anything larger is `0x80 | n`, where `n`
  is the number of bytes in the value's _minimal_ big-endian representation, followed by those `n`
  bytes. Decoding is canonical-only (ADR 0023): a determinant is refused unless it is byte-identical
  to what encoding the decoded value would produce, so a non-minimal long form (`0x81 0x03` for
  `3`), a zero-length long form (`0x80` aliasing `0x00`), and any `n > 8` are all
  `non_canonical_length`/`length_determinant_overflow` rather than accepted synonyms.
- **VarOctetString** — a VarUInt byte count, then exactly that many bytes.

An envelope is then, with no framing or padding of its own:

```text
request  = 0x01 || VarOctetString(method) || VarOctetString(target)
              || VarUInt(header_count) || header_count × ( VarOctetString(name)
                                                        || VarOctetString(value) )
              || VarOctetString(body)

response = 0x02 || status (2 bytes, big-endian uint16)
              || VarUInt(header_count) || header_count × ( VarOctetString(name)
                                                        || VarOctetString(value) )
              || VarOctetString(body)
```

`method`, `target`, and every header name and value are UTF-8 (a non-UTF-8 sequence is
`invalid_utf8`); `body` is arbitrary bytes. Headers are a **sequence, never a map** — order and
duplicate names are both part of the encoding. The leading type byte is the only discriminator
between the two directions, and any byte after the body is `trailing_bytes`, never ignored.

- `valid[]`: `{ name, encoded_hex, decoded }`. `encoded_hex` is the canonical OER encoding;
  decoding it must reproduce `decoded` exactly, and re-encoding `decoded` must reproduce
  `encoded_hex` exactly. `decoded.direction` is `"request"` (`method`, `target`, `headers`,
  `body_hex`) or `"response"` (`status`, `headers`, `body_hex`). `headers` is an ordered list of
  `[name, value]` pairs -- order and duplicate names are both significant and must survive.
- `invalid[]`: `{ name, direction, bytes_hex, expected_error }`. Decoding `bytes_hex` as the named
  `direction` must fail with `expected_error` (one of: `buffer_underflow`, `non_canonical_length`,
  `length_determinant_overflow`, `invalid_type`, `invalid_utf8`, `trailing_bytes`) -- never
  succeed, never panic, never fail with a different reason.

### `giftwrap`

The sealed wrap around an envelope (ADR 0018). `receiver_identity_secret_hex` /
`receiver_identity_public_hex` are a fixture keypair -- not a real operator key -- for a client SDK
to test against; `receiver_identity_public_hex` is the value a real connector would report from
`GET /ilp/identity`.

**Mechanism** (everything a replaying SDK needs and cannot get from this file's field names alone):

- **Two different 32-byte secrets are in play, and they are not the same thing.** The **ECDH
  result** is derived: the sender ECDHs a fresh, per-packet secp256k1 ephemeral key against the
  receiver's identity public key, and takes the result's raw X-coordinate (32 bytes, not hashed or
  otherwise processed before going into HKDF). The **shared secret** (`shared_secret_hex`) is
  _drawn at random_ by the sender, independently of ECDH, and is carried encrypted inside the
  request. The ECDH result exists only to protect that carriage; everything after the request --
  the response's key and the fulfilment -- comes from the random shared secret.
- The receiver's identity key is a secp256k1 keypair; `receiver_identity_public_hex` is the
  65-byte uncompressed form (`0x04 || X || Y`).
- Every derived value uses the same construction — **HKDF-SHA256, no salt** (`HKDF-Extract` is
  called with an all-zero salt, i.e. `Hkdf::new(None, ikm)`), expanded to exactly 32 bytes with a
  fixed ASCII `info` string. **The three uses do not share an `ikm`**, and getting this wrong is
  the easiest way to fail a replay:

  | Derived value          | `ikm`                                     | `info`                      |
  | ---------------------- | ----------------------------------------- | --------------------------- |
  | request AEAD key       | the ECDH result's X-coordinate (32 bytes) | `toon-giftwrap-request`     |
  | response AEAD key      | the 32-byte shared secret                 | `toon-giftwrap-response`    |
  | fulfilment (see below) | the 32-byte shared secret                 | `toon-giftwrap-fulfillment` |

  Only the request direction touches ECDH. Once the receiver has recovered the shared secret from
  inside the request, the response and the fulfilment are derived from that secret alone, which is
  why the response needs no second key exchange.

- The AEAD is **ChaCha20-Poly1305** (RFC 8439), 12-byte nonce, no additional authenticated data.
  "Ciphertext" below always means the AEAD output including its trailing 16-byte Poly1305 tag, so
  a sealed blob is 16 bytes longer than its plaintext.
- Wire framing:
  - A **request** (`request_wrap_hex`) is `0x01 || ephemeral_public_key (65 bytes, uncompressed) ||
nonce (12 bytes) || ciphertext`. The plaintext AEAD encrypts is `shared_secret (32 bytes) ||
encoded_envelope` -- the 32-byte shared secret rides _inside_ the encrypted request, not
    alongside it, which is what lets the response be sealed with no second key exchange.
  - A **response** (`response_wrap_hex`) is `0x02 || nonce (12 bytes) || ciphertext`, where the
    plaintext is just the encoded response envelope -- no embedded secret and no ephemeral key,
    since the response's AEAD key comes from `shared_secret_hex` alone (per the table above).

Each `cases[]` entry pins every input a real seal draws at random (`ephemeral_secret_hex`,
`shared_secret_hex`, `request_nonce_hex`, `response_nonce_hex`) so the output is reproducible:

- `request_envelope` / `request_envelope_hex`: the plaintext envelope, structured and encoded.
- `request_wrap_hex`: `request_envelope_hex` and `shared_secret_hex` sealed to
  `receiver_identity_public_hex` -- the bytes that ride as `Prepare.data`. Opening it with the
  fixture's secret key must recover `request_envelope_hex` and `shared_secret_hex` exactly.
- `response_envelope` / `response_envelope_hex`: the plaintext response envelope.
- `response_wrap_hex`: `response_envelope_hex` sealed with `shared_secret_hex` (no second key
  exchange) -- the bytes that ride as `Fulfill.data`. Opening it with `shared_secret_hex` must
  recover `response_envelope_hex` exactly, and must fail to open under any other secret.

### `fulfilment`

The fulfilment a terminating connector derives from a request's shared secret (ADR 0019).
`fulfilment_hex` is `HKDF-SHA256(salt=none, ikm=shared_secret_hex,
info="toon-giftwrap-fulfillment")`, expanded to 32 bytes -- the same HKDF construction as the
`giftwrap` section above, domain-separated from both its AEAD keys by this section's own `info`
string.

Issue #1269 / ADR 0069 removed the execution condition from the wire (`schema_version` 5): there is
no condition left to derive a fulfilment from or to check one against, so this section pins only
`derive_fulfillment`'s own determinism.

- `cases[]`: `{ name, shared_secret_hex, fulfilment_hex }`. `fulfilment_hex` is what a real
  connector derives from `shared_secret_hex` -- two cases, over two different secrets, so a
  replaying SDK can check that its own derivation reproduces both rather than one it could get
  right by accident.

### `claim`

A signed EIP-712 `BalanceProof` (ADR 0024) -- the digest and signature scheme both a peer-role
claim (`docs/protocol/peer-semantics-pre-868.md` §3.5) and a client-edge claim (`client-edge-spec.md` §1.3
step 4) are checked against. This is the scheme that replaced a SHA-256 tuple nothing on chain ever
verified; a client that still signs the old tuple has nothing else in this repository that would
tell it.

- `cases[]`: `{ name, chain_id, token_network_address_hex, channel_id_hex, nonce,
transferred_amount, locked_amount, locks_root_hex, digest_hex, signer_secret_hex,
signer_address_hex, signature_hex }`.
- `chain_id` / `token_network_address_hex` are the EIP-712 domain's `chainId` and
  `verifyingContract` -- configured **per channel** (`ClaimBook::set_channel_domain`), never a
  node-wide default. A vector hardcoding one real chain's values is not evidence of what another
  chain's channel signs; treat this case's `chain_id`/`token_network_address_hex` as one example
  domain, not the only one a real claim can be signed under.
- `channel_id_hex` is the channel's on-chain `bytes32` identifier -- the exact 32 bytes hashed into
  the struct, not a peering relation's own string label for the channel.
- `locked_amount` and `locks_root_hex` are always zero on the wire today (ADR 0004) but are still
  part of the hashed struct -- omitting them computes a different digest than the one a real
  signer signs.
- `digest_hex` is the EIP-712 digest: `keccak256(0x1901 || domainSeparator || structHash)`, where

  ```text
  domainSeparator = keccak256(abi.encode(
                        keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"),
                        keccak256("TokenNetwork"), keccak256("1"), chainId, verifyingContract))
  structHash       = keccak256(abi.encode(
                        keccak256("BalanceProof(bytes32 channelId,uint256 nonce,uint256 transferredAmount,uint256 lockedAmount,bytes32 locksRoot)"),
                        channelId, nonce, transferredAmount, lockedAmount, locksRoot))
  ```

  every integer field (`chainId`, `nonce`, `transferredAmount`, `lockedAmount`) is a 32-byte
  big-endian ABI word, matching Solidity's `uint256` encoding, and `verifyingContract` is a 20-byte
  address right-aligned into a 32-byte word.

- `signer_secret_hex` / `signer_address_hex` are a fixture secp256k1 keypair -- not a real
  operator or counterparty key -- so a replaying SDK can check both directions: that it computes
  the same `digest_hex` from the other fields, and that a 65-byte `r || s || recovery_id` signature
  over that digest (`signature_hex`; `recovery_id` is raw `0`/`1`, not the `27`/`28` a wallet's own
  signature carries) recovers to `signer_address_hex`.

### `peer_carriage`

Issue #729, [ADR 0021](../docs/adr/0021-vectors-are-normative-prose-is-not.md): the 20 items of
`docs/protocol/peer-carriage-spec.md` §10, generated from one fixture set per concept and
self-checked against the same functions -- `connector-peer-btp`'s codec and
`connector-peer-http`'s wrappers over it -- that judge them at runtime. **Most items are a pair**:
a BTP encoding and an HTTP encoding of the same fixture, and a replaying SDK should confirm both
decode to the same value (§10.1, spec I1) rather than trust either encoding alone.

Every JSON claim, ack and credential value below is the plain string a real interaction carries; a
`btp_raw_hex` field is that string's raw UTF-8 bytes (the BTP `protocolData` entry payload), and an
`http_base64` field is `base64` of the same bytes (the HTTP header value) -- never a second
encoding of a different value (§4, §1.4). Header names throughout are the canonical lower-case
forms `docs/protocol/peer-carriage-spec.md` §3 pins; header values in `http_headers` pair each
name with its value as `[name, value]`, in the order a real response would carry them.

- **`credential`** (item 1) -- `{ name, peer_id, secret, btp_raw_hex, http_base64 }`: the peer
  credential JSON of §1.4 (`{"peerId": ..., "secret": ...}`), on the `auth` protocolData entry and
  the `Toon-Peer-Auth` header.
- **`claim_evm`** (item 2) -- `{ name, blockchain, json, btp_raw_hex, http_base64, wire_channel_id,
wire_nonce, wire_cumulative_amount, wire_signature_hex }`: an EVM peer claim, the same JSON shape
  `client-edge-spec.md` §1.3 defines for a client claim (spec I4). `wire_*` fields are what the
  claim decodes to in-process (`connector_runtime::WireClaim`) -- the value both carriage decoders
  must agree on.
- **`claim_digest_hex`** (item 3) -- the same string as this file's `claim.cases[0].digest_hex`,
  repeated here rather than recomputed, demonstrating ADR 0024's EIP-712 digest is untouched by
  carriage.
- **`claim_solana`** (item 4) -- shaped like `claim_evm`, over a Solana claim, plus
  `signed_message_hex`: the 96-byte balance proof ADR 0053 defines, which is what this claim's
  `signature` covers (`TOON-BALPROOF-V2` || `programId` || `channelAccount` || `nonce` ||
  `transferredAmount`). **Aspirational**
  (`peer-semantics-pre-868.md` §3.5): this connector's outbound peer claims are EVM-only, so nothing today
  emits this shape, but `claim_json::parse` already accepts it inbound (issue #732) and this vector
  pins that shape before an emitter exists.

  `programId` names **the settlement program the claim's `channelAccount` lives under**
  (`docs/protocol/client-edge-spec.md` §1.3) -- byte-for-byte the value at offset 16 of
  `signed_message_hex`, which `the_solana_claim_vector_declares_the_program_its_signature_is_bound_to`
  asserts rather than leaves to the reader. It is not a free-form label: a payer who writes anything
  else is declaring a program no channel of theirs lives under. This fixture used to declare the
  **system program**, and that is why `schema_version` reached `2` (issue #1127) -- an SDK that
  carried version 1's reading of this field into a real claim builder is emitting a non-conforming
  claim. The value here is the deployed public-devnet payment-channel program
  (`packages/solana-program/deployments/devnet-public.md`), an example settlement program in the
  same way `claim.cases[0].chain_id` is Base Sepolia's real id -- a channel on another deployment
  names that deployment's program instead.

- **`prepare`** / **`prepare_no_claim`** (items 5, 6) -- `{ name, prepare, claim_json,
btp_message_hex, http_headers, http_body_hex }`: a claim-bearing PREPARE.
  `prepare` is `{ amount, expires_at, greeting, destination, data_hex }`, the OER
  `Prepare` both `btp_message_hex` (a complete BTP MESSAGE frame: type, `requestId`, the
  `payment-channel-claim` protocolData entry, then the OER PREPARE)
  and `http_body_hex` (the same OER bytes as a POST body) carry. `prepare_no_claim` is the same
  fixture with the claim entry/header removed -- "claimless is legal" pinned rather than assumed.
  `claim_json` is `null` there. **Those OER bytes are also this file's pin of the packet encoding
  itself** -- not only of peer carriage: see [The ILP packet encoding](#the-ilp-packet-encoding)
  above, which walks `http_body_hex` byte by byte and says where the encoding departs from
  RFC 0027 ([ADR 0063](../docs/adr/0063-the-ilp-packet-is-toons-dialect-not-rfc-0027s.md)).
- **`fulfill_ack_accepted`**, **`fulfill_ack_rejected`**, **`ack_rejected_reasons[]`**,
  **`reject_with_cost`**, **`ack_absent`**, **`flush_ack`** (items 7-11, 14) -- one shape,
  `{ name, packet ("fulfill"|"reject"|"none"), packet_hex, ack, accumulated_cost, btp_response_hex,
http_status, http_headers, http_body_hex }`. `ack` is `null` (absent, item 11) or `{ result,
reason }` (`reason` only when `result` is `"rejected"`). `http_status` is always `200` -- §6.2's
  independence of the packet's own verdict from the claim's. `fulfill_ack_rejected` is **the single
  most important vector in this set** (§10.2 item 8): a `FULFILL` answer carrying a _rejected_
  claim-ack on the one response, proving the two verdicts never couple. `ack_rejected_reasons[]` has
  one entry per §6.1 reason (`signature_invalid`, `nonce_not_advancing`, `amount_not_advancing`,
  `unknown_channel`), named `peer_ack_rejected_<reason>`. `reject_with_cost` carries both
  `accumulated_cost` and `ack` on one response. `flush_ack` answers an empty packet
  (`packet: "none"`, `packet_hex: ""`) -- the answer to a FLUSH.
- **`ack_malformed`** (item 12) -- `{ name, malformed_json, btp_raw_hex, http_base64 }`: an ack
  whose JSON does not decode to either verdict (here, an unrecognised `result`). Both carriages must
  read this as **not acknowledged** (§6.3), the same as `ack_absent` -- never an error, never a
  verdict.
- **`flush`** (item 13) -- `{ name, claim_json, transfer_amount, btp_transfer_hex, http_headers,
http_body_hex }`: `btp_transfer_hex` is a complete BTP TRANSFER frame whose `amount` equals
  `transfer_amount` (the claim's own cumulative amount -- the generator asserts this equality, not
  just a reader) and carries the claim entry with **no** `ilpPacket`. `http_body_hex` is empty; the
  claim rides the `ILP-Payment-Channel-Claim` header alone.
- **`claim_retransmit`**, **`claim_same_nonce_different_bytes`** (items 15, 16) -- `{ name,
first_claim_json, second_claim_json, first_ack, second_ack, second_ack_reason }`: §6.3's
  idempotent re-ack and its boundary. In `claim_retransmit`, `second_claim_json` is
  byte-identical to `first_claim_json` and both acks are `"accepted"` -- a retransmission of the
  claim already at the watermark is accepted again, not refused. In
  `claim_same_nonce_different_bytes`, `second_claim_json` carries the same nonce but a different
  (still validly signed) amount, and `second_ack` is `"rejected"` with `second_ack_reason:
"nonce_not_advancing"`.
- **`flush_requested`** (item 17) -- `{ name, channel_id, http_header_value, note }`. **HTTP
  only**: `note` records that BTP has no counterpart (§6.4) -- on BTP the payee can originate a
  request of its own, so the hint has nothing to ride.
- ~~**`minimum_delivery_absent`**, **`minimum_delivery_malformed`** (items 18, 19)~~ -- **deleted**
  in `schema_version` 3 (issue #1143). Minimum delivery is retired
  ([ADR 0057](../docs/adr/0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md)): no packet
  declares a floor, and the `toon-minimum-delivery` entry and `Toon-Minimum-Delivery` header are
  gone from both carriages. `R01` stays in the reject vocabulary with RFC 0027's own meaning -- a
  hop's fee alone exceeding the arriving amount -- and answers an unmet floor no longer, because
  there is no floor (ADR 0051 as corrected). The item numbers are not reused.
- **`forwarded_data_unchanged`** (item 20) -- `{ name, sealed_data_hex, btp_ilp_packet_prepare_hex,
http_body_hex }`: one sealed request wrap from this file's own `giftwrap` section (§8.1), carried
  as a PREPARE's `data` on both carriages. `sealed_data_hex` must appear byte-for-byte inside both
  `btp_ilp_packet_prepare_hex`'s OER PREPARE and `http_body_hex` -- a forwarding hop never
  re-encodes, re-wraps or truncates a payload it holds no key for.

### `channel_control_declaration`

Issue #792, `client-edge-spec.md` §1.9 step 1 (issue #790): the BTP auth entry's
`channelId`/`expires`/`signature` fields, which bind a client session to a channel it controls
_before_ that session has ever presented a claim. The signature scheme is the identical
domain-separated `ClaimStateChallenge` `POST /ilp/claim-state` verifies for a read
(`connector_signer::claim_state_challenge`), reused rather than a claim's own `BalanceProof`
scheme (this file's `claim` section above) -- deliberately a different EIP-712 typehash, so a
captured claim-state proof and a captured claim can never stand in for each other.

**Unlike every other section in this file, three fields here (`channel_id_hex`, `signature_hex`,
and the corresponding values embedded in `auth_json`) carry a `0x` prefix.** This is deliberate:
these are the literal strings that ride on the wire inside the auth entry's JSON body (matching
`peer_carriage.claim_evm.wire_channel_id`'s same convention for the same reason), not this file's
usual internal byte encoding.

- `cases[]`: `{ name, peer_id, chain_id, token_network_address_hex, channel_id_hex, expires,
counterparty_address_hex, signer_secret_hex, signer_address_hex, digest_hex, signature_hex,
auth_json, btp_message_hex, signature_verifies }`.
- `chain_id` / `token_network_address_hex` are the channel's own registered EIP-712 domain --
  same rule as the `claim` section: configured per channel, never a node-wide default.
- `digest_hex` is `keccak256(0x1901 || domainSeparator || structHash)`, where `domainSeparator` is
  the identical `EIP712Domain(name: "TokenNetwork", version: "1", chainId, verifyingContract)`
  construction the `claim` section's `BalanceProof` digest uses (see that section for the exact
  ABI encoding), and

  ```text
  structHash = keccak256(abi.encode(
                   keccak256("ClaimStateChallenge(bytes32 channelId,uint256 expires)"),
                   channelId, expires))
  ```

  -- a distinct type hash and a distinct field set from `BalanceProof`, so the two digests can
  never collide for any input.

- `counterparty_address_hex` is the channel's registered counterparty -- what `signature_hex` must
  recover to for `signature_verifies` to be `true`. `signer_secret_hex`/`signer_address_hex` are
  the keypair that actually produced `signature_hex`: for `channel_control_declaration_valid` and
  `channel_control_declaration_expired`, this is the same keypair as `counterparty_address_hex`
  (a genuine proof); for `channel_control_declaration_wrong_key`, it is a different, unrelated
  keypair, and `signature_verifies` is `false`.
- `expires` is unix seconds, compared by the verifier as `expires <= now` -> rejected -- a
  wall-clock fact at verification time that this static file cannot itself encode. Instead,
  `channel_control_declaration_valid`/`_wrong_key` use an `expires` far enough in the future
  (2100-01-01T00:00:00Z) to still be valid against any reasonable clock, and
  `channel_control_declaration_expired` uses `1` (1970-01-01T00:00:01Z) to be expired against any
  reasonable clock. `signature_verifies` is about the signature alone (`true` for
  `channel_control_declaration_expired` too, since its signature is genuine) -- a replaying SDK
  must apply the `expires` check itself, separately, exactly as
  `verify_and_record_declared_channel` (`connector-client-edge::btp`) does.
- `auth_json` is the auth entry's full JSON body -- `{peerId, secret, channelId, expires,
signature}` -- byte-for-byte what rides as the BTP `auth` protocolData entry's `data`.
  `btp_message_hex` is the complete BTP MESSAGE frame carrying it (no `ilpPacket`), decoded and
  re-checked against `auth_json` by the generator before being emitted.
