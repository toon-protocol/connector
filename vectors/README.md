# Wire vectors

`wire-vectors.json` is the cross-repo contract for the client-edge termination wire (issue #527,
[ADR 0021](../docs/adr/0021-vectors-are-normative-prose-is-not.md)): reproducing these bytes is
what conformance means for `toon-client`, `rig` and `swap`. It is generated, not hand-written --
see `crates/connector-vectors` and `docs/protocol/wire-vectors.md` for the invariants each section
is evidence of. This file is plain JSON so a client SDK can replay it without importing anything
from this repository.

Regenerate after any change to the envelope (`connector_domain::envelope`), the gift wrap
(`connector_signer::giftwrap`), the fulfilment derivation
(`connector_signer::giftwrap::derive_fulfillment`), or the voucher and challenge signing schemes
(`connector_signer::voucher_signature`, `connector_signer::claim_state_challenge`):

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

`prepare_no_claim` carries the same PREPARE bytes with the voucher removed, and
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

All byte fields are lowercase hex, no `0x` prefix, except where a section says a field is the
literal string that rides the wire. `schema_version` bumps only when a field's meaning changes in a
way that would make existing replay code misread it; a purely additive field does not bump it.

**`schema_version` 7** (issue #1384,
[ADR 0075](../docs/adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md) decision
14): **every claim is a voucher.** The `toon-channel` claim scheme is retired, and every section
that pinned it is gone: the top-level `claim` section (the EIP-712 `BalanceProof`),
`peer_carriage`'s `claim_evm`, `claim_solana`, `claim_digest_hex`, `flush`, `flush_ack`,
`flush_requested`, `claim_retransmit` and `claim_same_nonce_different_bytes`, and the whole
`channel_control_declaration` section (the `TokenNetwork`-domain `auth_channel_proof`, replaced by
the voucher claim-state challenge on the client BTP `auth` entry's `channelChallenge`). New:
`peer_carriage.voucher_evm`/`voucher_solana`, `peer_carriage.zero_value_challenge`, and the
top-level `toon_channel_refused` and `payout_voucher` sections. A claim with no `scheme` is now a
claim every connector refuses by name -- which is why this is a bump, not an addition.

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

### `peer_carriage`

Issue #729, [ADR 0021](../docs/adr/0021-vectors-are-normative-prose-is-not.md), as ADR 0075 decisions
5 and 6 re-base it: the items of `docs/protocol/peer-carriage-spec.md` §10, generated from one fixture
set per concept and self-checked against the same functions that emit and judge them at runtime --
the paying side's own renderers (`connector_runtime::voucher_json`,
`connector_runtime::challenge_entry`), `connector-peer-btp`'s codec and role gate, and
`connector-peer-http`'s headers and evidence reader. **Most items are a pair**: a BTP encoding and
an HTTP encoding of the same fixture, and a replaying SDK should confirm both decode to the same
value (§10.1, spec I1) rather than trust either encoding alone.

Every JSON voucher, challenge and ack value below is the plain string a real interaction carries; a
`btp_raw_hex` field is that string's raw UTF-8 bytes (the BTP `protocolData` entry payload), and an
`http_base64` field is `base64` of the same bytes (the HTTP header value). Header names throughout
are the canonical lower-case forms `docs/protocol/peer-carriage-spec.md` §3 pins; `http_headers`
pairs each name with its value as `[name, value]`.

- **`voucher_evm`** -- `{ name, chain_id, verifying_contract_hex, channel_config, channel_id_hex,
max_claimable_amount, digest_hex, signer_address_hex, signature_hex, json, btp_raw_hex,
http_base64 }`: a peer's voucher on its own outbound `x402BatchSettlement` channel toward the next
  hop, exactly as the paying node renders it into the claim slot. The facts are `claim_voucher.evm`'s
  fields under the same names; here `channel_config.payer_hex == payer_authorizer_hex ==
signer_address_hex` -- the paying node's **settlement key** signs (ADR 0075 decision 3) -- and the
  receiver is the next hop's settlement address in both receiving seats. A peer voucher always
  carries its `channelConfig`. `signature_hex` is `r ‖ s ‖ v`, `v` 27 or 28.
  `crates/connector-settlement-evm/tests/x402_voucher_vector.rs` checks `channel_id_hex` and
  `digest_hex` against the deployed contract's own `getChannelId`/`getVoucherDigest` on every run.
- **`voucher_solana`** -- `{ name, channel_account_base58, authorized_signer_base58,
signer_secret_hex, max_claimable_amount, signed_message_hex, signature_base58, json, btp_raw_hex,
http_base64 }`: the Solana twin, on a `payment-channels` channel whose `authorized_signer` is the
  paying node's Solana settlement key. `signed_message_hex` is the 50-byte voucher message
  (`claim_voucher.solana`'s layout). Its `senderId` is the channel account: a label, as a voucher's
  `senderId` always is.
- **`prepare`** / **`prepare_no_claim`** (items 5, 6) -- `{ name, prepare, claim_json,
challenge_json, btp_message_hex, http_headers, http_body_hex }`. `prepare` is
  `{ amount, expires_at, greeting, destination, data_hex }`, the OER `Prepare` both
  `btp_message_hex` (a complete BTP MESSAGE frame: type, `requestId`, the `payment-channel-claim`
  protocolData entry, then the OER PREPARE) and `http_body_hex` (the same OER bytes as a POST body)
  carry. `claim_json` is `voucher_evm.json`. `prepare_no_claim` is the same fixture with the voucher
  removed; `claim_json` and `challenge_json` are `null`. **Those OER bytes are also this file's pin
  of the packet encoding itself** -- see [The ILP packet encoding](#the-ilp-packet-encoding) above.
- **`zero_value_challenge`** -- `{ name, channel_id_hex, expires, digest_hex, signer_address_hex,
signature_hex, packet }`: a PREPARE whose `amount` is `0` (ADR 0075 decision 5). It carries **no
  voucher** and carries the voucher claim-state challenge instead, in a slot of its own -- the
  `peer-role-challenge` protocolData entry on BTP, the `Toon-Peer-Role-Challenge` header (base64) on
  HTTP -- so the receiver can attribute it to the peering. `packet` is a `prepare`-shaped pair with
  `claim_json` `null` and `challenge_json` the challenge: a `POST /ilp/claim-state` entry's JSON
  (`scheme: "batch-settlement"` required), signed by the channel's voucher signer (the paying node's
  settlement key). `digest_hex` is `voucher_claim_state_challenge.evm`'s construction over
  `voucher_evm`'s channel. A receiver honours a challenge only while `expires` is ahead of its clock
  and **no more than 300 seconds** ahead (`peer-carriage-spec.md` §1.2); this fixture's `expires`
  (2100-01-01T00:00:00Z) pins reproducible bytes, not a window a live receiver would accept -- a
  replaying SDK applies the window itself.
- **`fulfill_ack_accepted`**, **`fulfill_ack_rejected`**, **`ack_rejected_reasons[]`**,
  **`reject_with_cost`**, **`ack_absent`** (items 7-11) -- one shape,
  `{ name, packet ("fulfill"|"reject"), packet_hex, ack, accumulated_cost, btp_response_hex,
http_status, http_headers, http_body_hex }`. `ack` is `null` (absent, item 11) or `{ result,
reason }` (`reason` only when `result` is `"rejected"`). `http_status` is always `200` -- §6.2's
  independence of the packet's own verdict from the voucher's. `fulfill_ack_rejected` is **the single
  most important vector in this set** (§10.2 item 8): a `FULFILL` answer carrying a _rejected_
  claim-ack on the one response, proving the two verdicts never couple. `ack_rejected_reasons[]` has
  one entry per reason a voucher's verdict carries (`signature_invalid`, `amount_not_advancing`,
  `unknown_channel`), named `peer_ack_rejected_<reason>`; `nonce_not_advancing` is no longer pinned,
  since no voucher verdict produces it. `reject_with_cost` carries both `accumulated_cost` and `ack`
  on one response.
- **`ack_malformed`** (item 12) -- `{ name, malformed_json, btp_raw_hex, http_base64 }`: an ack
  whose JSON does not decode to either verdict. Both carriages must read this as **not
  acknowledged** (§6.3), the same as `ack_absent` -- never an error, never a verdict.
- **`forwarded_data_unchanged`** (item 20) -- `{ name, sealed_data_hex, btp_ilp_packet_prepare_hex,
http_body_hex }`: one sealed request wrap from this file's own `giftwrap` section (§8.1), carried
  as a PREPARE's `data` on both carriages. `sealed_data_hex` must appear byte-for-byte inside both
  -- a forwarding hop never re-encodes, re-wraps or truncates a payload it holds no key for.
- **Deleted** at `schema_version` 7 (ADR 0075): `claim_evm`, `claim_solana`, `claim_digest_hex`
  (items 2-4, the `toon-channel` claim), `flush`, `flush_ack`, `flush_requested` (items 13, 14, 17,
  the `toon-channel` FLUSH), and `claim_retransmit`, `claim_same_nonce_different_bytes` (items 15,
  16, the nonce rule; a voucher's retransmission is `claim_voucher.amount_only_watermark`'s
  `"retransmission"` case). Earlier: `credential` (item 1, `schema_version` 4) and
  `minimum_delivery_*` (items 18, 19, `schema_version` 3). The item numbers are not reused.

### `charge`

Issue toon-client#629, [ADR 0065](../docs/adr/0065-a-price-is-a-schedule-over-payload-length.md):
what a route priced `base + per_kib/KiB` charges for one packet, as a function of that packet's
payload length.

**This is the one section that pins arithmetic rather than bytes**, and it is here because the
arithmetic is a cross-repo contract in exactly the way an encoding is. A payload length is a
property of _carriage_, not of content -- every hop can measure `Prepare.data.len()` without
opening the gift wrap -- so a sender computes its own charge before it sends, and a client edge, a
peer price gate and a termination all evaluate the same schedule over the same number. Four
implementations, one answer required. When only prose bound them, one of them drifted: `toon-client`
computed `floor(len / 1024) + 1`, overpaid by a whole kibibyte at every exact multiple of 1024, and
went unnoticed for as long as it did because the error's direction was an overpay -- which a
connector accepts in silence, so there was no reject to read.

- `cases[]`: `{ name, base, per_kib, payload_len, kib, charge, saturated }`.
- **`base`, `per_kib` and `charge` are decimal strings, not JSON numbers**, unlike `claim`'s
  `transferred_amount`. The saturating rows reach `u64::MAX`, which is past 2^53 and would be
  rounded by any reader parsing JSON numbers into IEEE doubles. It is the same reason `GET /ilp`
  publishes `price` as a string. `payload_len` and `kib` stay numbers: both are small by
  construction.
- `payload_len` is `Prepare.data.len()` -- the length of the sealed gift wrap ([ADR 0018]), never
  anything inside it, and never the encoded ILP packet around it. For a client that means the
  bytes it just sealed, before base64 or OER framing: those add nothing to what is measured.
- `kib` is `ceil(payload_len / 1024)` -- kibibytes **started**, and **zero** for an empty payload.
  It is stated separately from `charge` so an SDK that arrives at the right total by a wrong route
  still fails on the unit count. The rows at 0, 1023, 1024, 1025, 2048 and 2049 are the whole
  point of this section: a boundary is the only place two plausible readings of "per kibibyte"
  differ, and a suite that samples only middles cannot tell `ceil` from `floor + 1` at all.
- `charge` is `base + per_kib * kib`, in **saturating** `u64` arithmetic. A flat price is a
  schedule whose slope is zero -- the same value, not an equivalent one -- so the `flat_*` rows
  charge `base` at every length, including the lengths where the metered rows all differ.
- `saturated` is `true` exactly where the exact arithmetic would exceed `u64::MAX` and the answer
  was clamped to it. An SDK computing in arbitrary-precision integers (JavaScript `BigInt`, Python
  `int`) will not clamp on its own and **must** apply this ceiling: an amount past `u64::MAX`
  cannot be encoded into the packet it would be paying for. The clamp is deliberate -- a charge no
  claim can cover refuses the packet, where wrapping would silently make it nearly free.
- The metered rows use `1000 + 10/KiB` because that is what the fleet's deployed store node
  charges, so these are not invented figures. `metered_live_measured_5161` is independently
  confirmed against that live node, whose x402 greeting quotes `price.charge(prepare.data.len())`
  for the packet it was handed.

### `claim_voucher`

[ADR 0074](../docs/adr/0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md) decision 7,
issue #1347 (`schema_version` 6); since `schema_version` 7 (ADR 0075) the **only** claim. A claim
carries a **required** `scheme` discriminator, `"batch-settlement"`: a **voucher** -- x402's own
claim, verified against a signature scheme per chain, with no nonce: its freshness is an
amount-only watermark (`connector_domain::validate_voucher`). A claim with no `scheme`, or
`"toon-channel"`, is refused by name (`toon_channel_refused`). This section pins the client edge's
shape; `peer_carriage.voucher_evm`/`voucher_solana` pin a peer's, with their BTP/HTTP framing pair.

- **`evm`** -- `{ name, chain_id, verifying_contract_hex, channel_config, channel_id_hex,
max_claimable_amount, digest_hex, signer_address_hex, signature_hex, json }`. `channel_config` is
  `{ payer_hex, payer_authorizer_hex, receiver_hex, receiver_authorizer_hex, token_hex,
withdraw_delay, salt_hex }` -- x402's `ChannelConfig`, the seven fields a channel's id is the
  EIP-712 hash of. `channel_id_hex` is `getChannelId(channel_config)` under the domain
  `("x402 Batch Settlement", "1", chain_id, verifying_contract_hex)`; `digest_hex` is
  `getVoucherDigest(channel_id_hex, max_claimable_amount)` -- what `signature_hex` (`r ‖ s ‖ v`, 65
  bytes) actually signs, recovering to `signer_address_hex` (`payerAuthorizer`, since this
  fixture's is nonzero -- ADR 0074 decision 4). `json` is the full claim, exactly as it rides the
  claim header/protocolData entry -- note its wire field names are camelCase
  (`channelId`, `maxClaimableAmount`, `channelConfig.payerAuthorizer`, ...) where this file's own
  `_hex` fields are snake_case.

  **Independently confirmed against the deployed contract.** `channel_id_hex` and `digest_hex` are
  not only this repository's own arithmetic: they equal `x402BatchSettlement.getChannelId` and
  `.getVoucherDigest` called live against the contract's one deployed address
  (`0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003`) on Base Sepolia (chain 84532), over
  `https://sepolia.base.org` at block 47289378 -- the exact `cast call` commands and their output
  are recorded next to where this section is generated
  (`crates/connector-vectors/src/lib.rs`, above `generate_voucher_evm_case`), so this vector cannot
  silently drift from the chain it names. The workspace gate re-checks it on every run:
  `crates/connector-settlement-evm/tests/x402_voucher_vector.rs` reads this committed case, places
  Base Sepolia's `x402BatchSettlement` runtime bytecode at that address on an `anvil` running as chain
  84532, and asserts the contract's own `getChannelId` and `getVoucherDigest` return `channel_id_hex`
  and `digest_hex`. The same test checks `peer_carriage.voucher_evm` and `payout_voucher.evm` --
  every EVM voucher this file pins.

- **`solana`** -- `{ name, channel_account_hex, channel_account_base58, signer_public_key_hex,
signer_public_key_base58, max_claimable_amount, expires_at, signed_message_hex, signature_hex,
signature_base58, json }`. `signed_message_hex` is payment-channels' 50-byte voucher message --
  `0x5601 ‖ channel_account ‖ cumulative_amount (u64 LE) ‖ expires_at (i64 LE)` -- what
  `signature_hex`/`signature_base58` (the same 64 bytes, Ed25519) covers, verifying against
  `signer_public_key_hex`, the channel's `authorized_signer`. `expires_at` is `0`: see `invalid[]`
  for what a nonzero one costs. `json`'s wire fields (`channelId`, `signature`) are base58 on
  Solana, unlike EVM's hex.

- **`amount_only_watermark[]`** -- `{ name, watermark_amount, watermark_signature_hex,
presented_amount, presented_signature_hex, charge, outcome, advanced }`: the outcomes of
  [`connector_domain::validate_voucher`], the amount-only rule a voucher's freshness is judged by
  (ADR 0074 decision 3) -- the only freshness rule since ADR 0075 deleted the nonce rules. `watermark_amount`/
  `watermark_signature_hex` are `null` for a channel that has never accepted a voucher; otherwise
  they are the amount and signature of the voucher that set the watermark, and
  `presented_amount`/`presented_signature_hex` are the voucher now being judged against it.
  `outcome` is one of:
  - `"amount_not_advancing"` -- the presented amount equals the watermark's under a **different**
    signature. Refused: for a voucher, not advancing means _not strictly greater_.
  - `"advances"` -- the presented amount is strictly above the watermark's, `advanced` (the
    difference) covers `charge`, and the voucher is accepted; `advanced` carries the figure.
  - `"retransmission"` -- the presented amount **and** signature are byte-identical to the
    voucher at the watermark: a retransmission, not a new claim -- accepted again, buying nothing
    new. Byte identity is the test: an equal amount under a
    different signature is `"amount_not_advancing"` instead, not a retransmission. Pinned at a
    `charge` of `0`.
  - `"underpayment"` -- the same byte-identical retransmission against a nonzero `charge`. It buys
    nothing, so it covers none of the charge: refused as an underpayment, advancing by `0` (ADR
    0074 decision 3, amended 2026-09-25).

- **`invalid[]`** -- `{ name, claim_json, expected_error }`, the same shape as `envelope`'s
  `invalid[]`: parsing `claim_json` as a client-edge claim must fail with `expected_error`, never
  succeed. Today's one entry, `claim_voucher_solana_expires_at_nonzero`
  (`expected_error: "voucher_expires"`), is ADR 0074 decision 3's Solana rule: `expiresAt` must be
  `0`. x402 itself requires this and the program refuses a nonzero one at `settle` with no state
  change, so the connector refuses it structurally, **before any signature check** -- `claim_json`
  here carries `solana`'s own genuine channel, signer and signature bytes with only `expiresAt`
  changed, which is what makes this a structural refusal rather than a signature failure.

### `voucher_claim_state_challenge`

Issue #1364, `client-edge-spec.md` §1.10: the signature a `POST /ilp/claim-state` entry under
`scheme: "batch-settlement"` carries, proving control of an x402 channel so the connector will
report its voucher watermark (added at `schema_version` 6). Since ADR 0075 the same message also
proves the peer role for a zero-value packet (`peer_carriage.zero_value_challenge`) and declares a
client's channel on its BTP `auth` entry (`channelChallenge`, replacing the retired
`auth_channel_proof`). It is signed by the channel's **voucher signer** -- `payerAuthorizer`, else
`payer`, of the verified `ChannelConfig` on EVM; `authorized_signer` on Solana (ADR 0074 decision 4) -- over a challenge kept apart from that key's vouchers. Both chains' cases are on the
`claim_voucher` section's channels. `signature_verifies` is about the signature alone, and every
case's `expires` is 2100-01-01T00:00:00Z; a replaying SDK applies the `expires <= now` check itself.

- **`evm[]`** -- `{ name, chain_id, verifying_contract_hex, channel_id_hex, expires,
voucher_signer_address_hex, signer_secret_hex, signer_address_hex, digest_hex, signature_hex,
signature_verifies, entry_json }`. `digest_hex` is `keccak256(0x1901 || domainSeparator ||
  structHash)`, where `structHash = keccak256(abi.encode(keccak256("ClaimStateChallenge(bytes32
  channelId,uint256 expires)"), channelId, expires))`, under **`x402BatchSettlement`'s** domain,
  `("x402 Batch Settlement", "1", chain_id, verifying_contract_hex)`, the one `claim_voucher.evm`'s
  digest uses. A distinct type hash from `Voucher`, so it never stands in for a voucher.
  `signature_hex` is `0x`-prefixed `r ‖ s ‖ v` with `v` 27 or 28. `voucher_claim_state_evm_valid`
  is signed by `voucher_signer_address_hex` (anvil's published account 1, the fixture's
  `payerAuthorizer`); `voucher_claim_state_evm_wrong_key` by an unrelated key, and does not verify.
- **`solana[]`** -- `{ name, channel_account_base58, expires, authorized_signer_base58,
signer_secret_hex, signer_public_key_base58, signed_message_hex, signature_base64,
signature_verifies, entry_json }`. `signed_message_hex` is
  `"toon-voucher-claim-state-challenge-v1" ‖ channel_account (32 bytes) ‖ expires (u64 LE)`, and
  `signature_base64` its Ed25519 signature, base64 as every Solana claim-state signature is (not
  base58, as a voucher's is). `signer_secret_hex` is the Ed25519 seed that signed.
- **`entry_json`** is the entry byte-for-byte as it rides in the request's `channels[]`. The EVM
  entry carries the channel's `channelConfig`, in a voucher's own spelling: a node that has
  accepted a voucher on the channel already holds it and ignores this, and one that has not needs
  it, because the contract stores a channel by id alone.

### `toon_channel_refused`

[ADR 0075](../docs/adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md) decision
8, issue #1384: a claim with no `scheme`, or with `scheme: "toon-channel"`, is the retired
`toon-channel` claim, and every edge refuses it **by name**, never as merely malformed -- the way a
`mina` claim is refused.

- `cases[]`: `{ name, claim_json, client_edge_error, client_edge_message, btp_message_hex,
btp_error_hex, http_headers, http_body_hex, http_status, http_response_body_hex, refusal_text }`.
  `claim_json` is a retired claim exactly as a pre-ADR 0075 client or peer sends it
  (`toon_channel_evm_no_scheme`, `toon_channel_evm_explicit_scheme`, `toon_channel_solana_no_scheme`).
- **Client edge:** parsing it fails with `client_edge_error` `"toon_channel"` (a stable tag);
  `client_edge_message` is the message, which names the retirement and is informational.
- **BTP peer carriage:** `btp_message_hex` is the claim riding a PREPARE; it is answered with
  `btp_error_hex`, an ERROR frame (`code F00`, `name NotAcceptedError`, `data` = `refusal_text`),
  before the role is decided.
- **HTTP peer carriage:** the same claim in the `Payment-Channel-Claim` header beside the same
  PREPARE (`http_body_hex`) is answered `http_status` `400`, with no ILP body and `refusal_text` as a
  `text/plain` body (`http_response_body_hex`).

### `payout_voucher`

ADR 0075 decision 7 (issue #1381): a connector pays a client back with a voucher on its own
outbound channel toward the client's payee key, signed by the chain's settlement key, and delivers
it as a BTP TRANSFER on the client's session (`client-edge-spec.md` §1.9 step 7). ADR 0026's #1073
correction records the client BTP dialect as uncovered by this contract; this section covers the one
entry of it a client must parse to be paid.

- **`evm`** -- `{ name, chain_id, verifying_contract_hex, channel_config, channel_id_hex,
max_claimable_amount, digest_hex, signer_address_hex, signature_hex, json, btp_transfer_hex }`: the
  voucher facts under `claim_voucher.evm`'s names (checked against the deployed contract by the same
  `x402_voucher_vector.rs`), with the connector as `payer == payerAuthorizer` and the client's payee
  in both receiving seats. `json` is the `payout-claim` entry: the voucher claim JSON **less its
  envelope** (`version`, `messageId`, `timestamp`, `senderId`) -- `blockchain`, `scheme`,
  `channelId`, `maxClaimableAmount` (decimal string), `signature`, and always the `channelConfig`
  landing needs. `btp_transfer_hex` is the complete TRANSFER frame: its `amount` is
  `max_claimable_amount`, its one protocolData entry is `payout-claim` carrying `json` as raw UTF-8,
  and it has no `ilpPacket`.
- **`solana`** -- `{ name, channel_account_base58, authorized_signer_base58, signer_secret_hex,
max_claimable_amount, signed_message_hex, signature_base58, json, btp_transfer_hex }`: the same,
  on `payment-channels`, with `expiresAt: 0` and a base58 signature over the 50-byte message.
- A client lands a payout itself: it can restore the envelope and parse `json` as its own voucher
  claim (the generator does exactly this before committing each case), and only the channel's
  receiver can land it.

[ADR 0018]: [ADR 0018]: ../docs/adr/0018-a-payload-is-sealed-to-the-terminating-connector.md
