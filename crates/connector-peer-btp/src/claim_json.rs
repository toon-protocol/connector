//! The claim on the wire (`peer-carriage-spec.md` §4): **the client edge's
//! claim JSON, verbatim**.
//!
//! One claim shape, one JSON encoding, two transfer encodings. A peer claim
//! is the same JSON object `client-edge-spec.md` §1.3 defines for a client
//! claim -- `version: "1.0"`, discriminated by `blockchain`, with the same
//! required and chain-specific fields -- and on BTP the
//! `payment-channel-claim` protocolData entry carries
//! `JSON.stringify(claim)` as raw UTF-8, no base64 layer (that is a header
//! artifact and nothing else).
//!
//! This is not a convenience. It is spec I4: one claim codec, one
//! structural validator, one signature verifier serve both edges, so a
//! change to the claim shape cannot land on one and not the other.
//! [`parse`] therefore calls
//! [`connector_domain::client_claim::parse_client_claim`] -- the client
//! edge's own validator -- rather than reading fields itself, and
//! [`encode`] emits exactly what that validator accepts.
//!
//! `connector_runtime::WireClaim::encode`'s length-prefixed binary form is
//! **not used on either carriage** (§3.2). It stays an in-process type
//! above the `PeerTransport` port; this module is the conversion, and
//! nothing here ever puts those bytes on a wire.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use connector_btp::{ProtocolData, CLAIM_PROTOCOL, CONTENT_TYPE_TEXT};
use connector_domain::client_claim::{parse_client_claim, ClientClaim, ClientClaimError};
use connector_runtime::{ClaimSignature, WireClaim};
use connector_signer::Signature;

/// The EIP-712 domain a peering relation's channel is signed under -- the
/// `[[peer_channels]]` row's `chain_id` and `token_network`
/// (`peer-carriage-spec.md` §11). Carried onto the wire as the claim's
/// optional `chainId`/`tokenNetworkAddress`, which is what lets a
/// counterparty's `ClaimBook` check the signature against ADR 0024's
/// digest without any out-of-band agreement beyond the config both
/// operators already wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerClaimDomain {
    pub chain_id: u64,
    pub token_network: [u8; 20],
}

/// Why a `payment-channel-claim` entry could not become a [`WireClaim`].
#[derive(Debug, PartialEq, Eq)]
pub enum ClaimDecodeError {
    /// Not valid UTF-8, so not the raw-UTF-8 JSON §4 requires.
    NotUtf8,
    /// Structurally invalid per the client edge's own validator.
    Structural(String),
    /// A claim on a chain this connector cannot judge. Only `mina`
    /// reaches this now (ADR 0002 drops Mina from the Rust connector, and
    /// `ClientClaim` has no Mina variant at all); `solana` used to, and
    /// no longer does -- `ClaimBook` verifies ed25519 balance proofs
    /// alongside EIP-712 ones since issue #732, which is what the live
    /// devnet peering actually settles on.
    UnsupportedChain(&'static str),
    /// The `signature` field is not the shape its chain requires: `0x` +
    /// 130 hex characters for EVM (§4.2's 65-byte `r ‖ s ‖ v`), or base64
    /// of exactly 64 bytes for Solana's ed25519 -- the same encoding the
    /// client edge's own gate decodes a Solana claim signature from
    /// (`connector_client_edge::claim_gate`), since I4 means one codec
    /// serves both edges.
    Signature,
    /// An x402 `batch-settlement` **voucher** (ADR 0074 decision 1). A
    /// voucher is client edge only: it is never a peer claim and can never
    /// decide the role `peer`, so every peer carriage refuses one here,
    /// structurally, before anything about it is judged. It is reported by
    /// name rather than as [`ClaimDecodeError::Structural`] because nothing
    /// is wrong with its shape -- it is on the wrong edge.
    Voucher,
}

impl std::fmt::Display for ClaimDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClaimDecodeError::NotUtf8 => f.write_str("claim protocolData is not valid UTF-8 JSON"),
            ClaimDecodeError::Structural(reason) => write!(f, "{reason}"),
            ClaimDecodeError::UnsupportedChain(chain) => {
                write!(
                    f,
                    "'{chain}' peer claims are not verifiable by this connector"
                )
            }
            ClaimDecodeError::Signature => f.write_str(
                "'signature' must be 0x-prefixed 130-char hex (r ‖ s ‖ v) for an evm claim, \
                 or base64 of 64 ed25519 bytes for a solana one",
            ),
            ClaimDecodeError::Voucher => f.write_str(
                "a 'batch-settlement' voucher is not a peer claim: vouchers are accepted at the \
                 client edge only (ADR 0074)",
            ),
        }
    }
}

/// The canonical form of an EVM channel id (§4.1, `client-edge-spec.md`
/// §1.3 step 2): `0x` + 64 **lower-case** hex.
///
/// Applied before a watermark is read or written, on both the emitting and
/// the receiving side. A connector that keyed a peer watermark by literal
/// text would grant a fresh watermark per spelling, and one signed claim
/// would buy carriage once per casing it was retyped in. Anything that is
/// not that shape is returned unchanged -- it will fail structural
/// validation on its own, and quietly rewriting it here would only hide
/// where.
pub fn canonical_evm_channel_id(channel_id: &str) -> String {
    let is_bytes32_hex = channel_id.len() == 66
        && channel_id.starts_with("0x")
        && channel_id[2..].bytes().all(|b| b.is_ascii_hexdigit());
    if is_bytes32_hex {
        channel_id.to_ascii_lowercase()
    } else {
        channel_id.to_string()
    }
}

/// `0x` + 64 lower-case hex, the shape `locksRoot` takes when locks are
/// hashed as zeros -- which is always, per `peer-semantics-pre-868.md` §3.5 and
/// ADR 0024. Neither field enters the digest as anything else, on either
/// edge.
const ZERO_BYTES32: &str = "0x0000000000000000000000000000000000000000000000000000000000000000";

/// Render `claim` as the §4 JSON, signed by `signer_address` (EVM) or
/// `solana_signer_public_key` (Solana), on `domain` where the claim is EVM
/// or under `solana_program_id` where it is Solana.
///
/// Which arm renders is decided by `claim.signature`'s own discriminant,
/// not by which parameter is `Some` -- mirroring `ClaimBook::record_fulfillment`
/// (issue #742), which never produces a `ClaimSignature::Solana` without a
/// `solana_signer` configured to have signed it. `solana_signer_public_key`
/// or `solana_program_id` being `None` while `claim.signature` is
/// `ClaimSignature::Solana` is therefore a caller bug (a transport driving a
/// claim it has no identity, or no `[[peer_channels]]` program id, to
/// render), not a reachable production state -- panics with a message
/// naming which side is missing rather than rendering a claim with no
/// `signerPublicKey` or a placeholder `programId`, either of which
/// `parse_solana` would refuse or silently mis-describe (issue #759).
///
/// `message_id` and `timestamp` are the caller's, deliberately: §6.3
/// requires a payer's retransmission of an unacknowledged claim to be
/// **byte-identical**, and a timestamp sampled here would make that
/// impossible. The dial side therefore caches the string it emitted for a
/// `(channel, nonce, cumulative)` triple and reuses it (see
/// [`crate::dial`]), rather than re-rendering with a fresh `now`.
pub fn encode(
    claim: &WireClaim,
    signer_address: &[u8; 20],
    solana_signer_public_key: Option<&[u8; 32]>,
    solana_program_id: Option<&str>,
    domain: Option<PeerClaimDomain>,
    message_id: &str,
    timestamp: &str,
) -> String {
    match claim.signature {
        ClaimSignature::Evm(signature) => {
            let signer = format!("0x{}", hex::encode(signer_address));
            let mut json = serde_json::json!({
                "version": "1.0",
                "blockchain": "evm",
                "messageId": message_id,
                "timestamp": timestamp,
                "senderId": signer,
                "channelId": canonical_evm_channel_id(&claim.channel_id),
                "nonce": claim.nonce,
                "transferredAmount": claim.cumulative_amount.to_string(),
                // Hashed as zeros, always (§3.1, ADR 0024): the deployed
                // `TokenNetwork.sol` typehash requires the fields, and this
                // connector has no locks to put in them.
                "lockedAmount": "0",
                "locksRoot": ZERO_BYTES32,
                "signature": format!("0x{}", hex::encode(signature.to_bytes())),
                "signerAddress": signer,
            });
            // `chainId`/`tokenNetworkAddress` are optional in the claim
            // shape (`client-edge-spec.md` §1.3), so a channel this
            // connector has no `[[peer_channels]]` row for is rendered
            // **without** them rather than with a zero domain that would
            // fail structural validation at the far end. The counterparty
            // then judges it against its own record, which is where a
            // channel neither end has bound gets its honest
            // `unknown_channel`.
            if let Some(domain) = domain {
                let object = json.as_object_mut().expect("a json! object");
                object.insert("chainId".to_string(), domain.chain_id.into());
                object.insert(
                    "tokenNetworkAddress".to_string(),
                    format!("0x{}", hex::encode(domain.token_network)).into(),
                );
            }
            serde_json::to_string(&json).expect("a json! object always serializes")
        }
        ClaimSignature::Solana(signature) => {
            let signer_public_key = solana_signer_public_key.unwrap_or_else(|| {
                panic!(
                    "a Solana claim was handed to the dial side with no solana_signer_public_key \
                     configured on the transport -- ClaimBook signed a claim this carriage has no \
                     identity to render (issue #742)"
                )
            });
            let program_id = solana_program_id.unwrap_or_else(|| {
                panic!(
                    "a Solana claim on channel '{}' was handed to the dial side with no \
                     solana_program_id -- render it only for a channel with a Solana \
                     '[[peer_channels]]' row, which config load gives a program id from \
                     '[settlement.solana]' (issues #759, #1128)",
                    claim.channel_id
                )
            });
            // Deliberately no `chainId`/`tokenNetworkAddress`-style domain
            // fields here: a Solana claim's signature covers
            // `solana_balance_proof_message`'s 96 bytes, which bind the
            // settlement program id (ADR 0053) and carry no EIP-712 domain
            // to render (`SolanaChannel`'s own doc,
            // `connector_runtime::claim`).
            let signer = bs58::encode(signer_public_key).into_string();
            let json = serde_json::json!({
                "version": "1.0",
                "blockchain": "solana",
                "messageId": message_id,
                "timestamp": timestamp,
                "senderId": signer,
                "programId": program_id,
                "channelAccount": claim.channel_id,
                "nonce": claim.nonce,
                "transferredAmount": claim.cumulative_amount.to_string(),
                "signature": BASE64.encode(signature),
                "signerPublicKey": signer,
            });
            serde_json::to_string(&json).expect("a json! object always serializes")
        }
    }
}

/// The `payment-channel-claim` protocolData entry carrying `json` as **raw
/// UTF-8** (§4) -- verbatim the client edge's existing convention
/// (`client-edge-spec.md` §1.9 step 2), with no base64 layer.
pub fn protocol_data(json: &str) -> ProtocolData {
    ProtocolData {
        name: CLAIM_PROTOCOL.to_string(),
        content_type: CONTENT_TYPE_TEXT,
        data: json.as_bytes().to_vec(),
    }
}

/// More than one claim was presented on one frame or one request.
///
/// Refused, never resolved (§1.5). The connector MUST NOT pick the first,
/// the last, or a concatenation: this is the smuggling defence, and its
/// absence is how "which claim did we verify?" becomes unanswerable. The
/// carriage maps it -- BTP: an ERROR frame (`code F00`, `name
/// NotAcceptedError`); HTTP: `400` with no ILP body.
///
/// It is role-independent on purpose. An ambiguous claim is refused whether
/// or not any of its candidates would have proven a peering, because
/// deciding *which* to verify is the thing that cannot be done safely.
/// Until ADR 0060 this rule guarded the `{peerId, secret}` credential; the
/// defect it names is a property of duplicated authentication material
/// rather than of the credential in particular, and the claim is now that
/// material.
#[derive(Debug, PartialEq, Eq)]
pub struct AmbiguousClaim {
    /// How many were presented. Two or more, by construction.
    pub presented: usize,
}

impl std::fmt::Display for AmbiguousClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "more than one claim presented on one interaction ({}): ambiguous claims are \
             refused, not resolved (peer-carriage-spec.md §1.5)",
            self.presented
        )
    }
}

impl std::error::Error for AmbiguousClaim {}

/// The raw JSON of a frame's `payment-channel-claim` entry, if it carried
/// one. A frame with no claim is legal on both carriages (§10.2 item 6),
/// so `None` is an ordinary outcome and not a refusal.
///
/// **First-wins, and only safe behind [`present_from_protocol_data`].** A
/// caller that reaches for this directly is answering "which claim did we
/// verify?" with "whichever came first"; §1.5 requires the frame be refused
/// instead. It stays public for the one caller that is genuinely only
/// peeking -- the client edge's shared front door, deciding whether an
/// arrival is peer handling's at all before the carriage that owns the rule
/// applies it.
pub fn from_protocol_data(protocol_data: &[ProtocolData]) -> Option<&[u8]> {
    protocol_data
        .iter()
        .find(|pd| pd.name == CLAIM_PROTOCOL)
        .map(|pd| pd.data.as_slice())
}

/// The claim a BTP frame presents, from every `payment-channel-claim` entry
/// on it (§1.5).
///
/// Zero entries is `None` -- a frame that presents no claim is a client
/// frame, which is an ordinary outcome and not an error. Two or more is
/// [`AmbiguousClaim`], **counted before anything is parsed**, so a second
/// undecodable entry cannot be quietly discarded to leave one unambiguous
/// claim standing.
pub fn present_from_protocol_data(
    protocol_data: &[ProtocolData],
) -> Result<Option<&[u8]>, AmbiguousClaim> {
    let entries: Vec<&ProtocolData> = protocol_data
        .iter()
        .filter(|pd| pd.name == CLAIM_PROTOCOL)
        .collect();
    if entries.len() > 1 {
        return Err(AmbiguousClaim {
            presented: entries.len(),
        });
    }
    Ok(entries.first().map(|pd| pd.data.as_slice()))
}

/// Parse a §4 claim into the in-process [`WireClaim`] the pipeline below
/// the port judges, through the client edge's own structural validator
/// (I4).
///
/// The channel id is canonicalised here, before the caller can reach a
/// watermark with it (§4.1).
pub fn parse(raw: &[u8]) -> Result<WireClaim, ClaimDecodeError> {
    let json = std::str::from_utf8(raw).map_err(|_| ClaimDecodeError::NotUtf8)?;
    let claim = parse_client_claim(json).map_err(|error| match error {
        ClientClaimError::Mina => ClaimDecodeError::UnsupportedChain("mina"),
        other => ClaimDecodeError::Structural(other.to_string()),
    })?;
    match claim {
        ClientClaim::Evm(claim) => Ok(WireClaim {
            channel_id: canonical_evm_channel_id(&claim.channel_id),
            nonce: claim.nonce,
            cumulative_amount: claim.transferred_amount,
            signature: ClaimSignature::Evm(parse_evm_signature(&claim.signature)?),
        }),
        // The channel id is the base58 `channelAccount`, uncanonicalised
        // and deliberately so: base58 of an exact 32-byte decode has one
        // spelling, so unlike EVM hex there is no second spelling a
        // second watermark could hide behind, and normalising anyway
        // would risk merging two accounts that are not the same account
        // (`connector_domain::client_claim::canonical_channel_key`'s own
        // reasoning, and the reason its `solana:` arm is the identity).
        //
        // `programId`/`signerPublicKey` are validated by the structural
        // pass above and then dropped here, exactly as the EVM branch
        // drops `signerAddress`: whose signature a claim is checked
        // against comes from `ClaimBook`'s own per-channel record
        // (`set_solana_channel`), never from the claim.
        //
        // For `signerPublicKey` that is the whole story -- it rides the
        // wire and carries no authority, and no value is pinned for it.
        // `programId` is no longer the same case and the analogy above
        // must not be read that far (issue #1127): since PR #1133 what a
        // payer MUST write there is pinned -- the settlement program the
        // `channelAccount` lives under -- and the client edge **reports**
        // a claim that writes something else (`connector_client_edge`'s
        // `claim_gate`, at `warn`). Dropping it here means the peer edge
        // does not, and that difference is a decision rather than an
        // oversight: `peer-carriage-spec.md` §4.1 states it and argues it,
        // and `client-edge-spec.md` §1.3 step 4 records that the reporting
        // duty is the client edge's alone.
        //
        // The short of it. Since issue #1128 a Solana peering has exactly
        // one program it can be judged under, `[settlement.solana]
        // program_id`, and that one value both renders `programId` on an
        // outbound peer claim (`encode` above, from
        // `PeerRelation::solana_program_ids`) and keys the `SolanaChannel`
        // an inbound one is verified against. A peer that declares a
        // program it did not sign under is a disagreement this connector
        // cannot produce; a peer that signs under a program this node does
        // not settle with already fails `SignatureInvalid` and moves no
        // traffic at all. Neither leaves a report anyone could act on, and
        // the price of one would be an authority-free field on `WireClaim`.
        // `tests/a_peer_claims_declared_program_is_not_consulted.rs` holds
        // both halves of that.
        ClientClaim::Solana(claim) => Ok(WireClaim {
            channel_id: claim.channel_account,
            nonce: claim.nonce,
            cumulative_amount: claim.transferred_amount,
            signature: ClaimSignature::Solana(parse_solana_signature(&claim.signature)?),
        }),
        // ADR 0074 decision 1: a voucher is never a peer claim. `WireClaim`
        // has no voucher shape to put one in -- `ClaimSignature` carries no
        // voucher variant -- so this is where both carriages refuse it.
        ClientClaim::EvmVoucher(_) | ClientClaim::SolanaVoucher(_) => {
            Err(ClaimDecodeError::Voucher)
        }
    }
}

/// §4.2: 65 bytes `r ‖ s ‖ v`, with `v` as libsecp256k1 emits it
/// (`{0, 1}`), never the wallet `{27, 28}` convention. The one place that
/// conversion happens is immediately before on-chain submission, and it is
/// not here.
fn parse_evm_signature(signature: &str) -> Result<Signature, ClaimDecodeError> {
    let hex_body = signature
        .strip_prefix("0x")
        .ok_or(ClaimDecodeError::Signature)?;
    let bytes = hex::decode(hex_body).map_err(|_| ClaimDecodeError::Signature)?;
    if bytes.len() != 65 {
        return Err(ClaimDecodeError::Signature);
    }
    let mut r = [0u8; 32];
    r.copy_from_slice(&bytes[0..32]);
    let mut s = [0u8; 32];
    s.copy_from_slice(&bytes[32..64]);
    Ok(Signature {
        r,
        s,
        recovery_id: bytes[64],
    })
}

/// A Solana claim's signature: base64 of exactly 64 ed25519 bytes -- the
/// encoding `connector_client_edge`'s claim gate already decodes a client
/// Solana claim's `signature` from, and therefore the one a peer claim
/// uses too (I4: one codec, both edges). Never hex: the client edge has
/// never emitted hex here and the live TypeScript fleet does not either.
fn parse_solana_signature(signature: &str) -> Result<[u8; 64], ClaimDecodeError> {
    let bytes = BASE64
        .decode(signature)
        .map_err(|_| ClaimDecodeError::Signature)?;
    bytes.try_into().map_err(|_| ClaimDecodeError::Signature)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire_claim() -> WireClaim {
        WireClaim {
            channel_id: format!("0x{:064x}", 7),
            nonce: 4,
            cumulative_amount: 12_500,
            signature: ClaimSignature::Evm(Signature {
                r: [0x11; 32],
                s: [0x22; 32],
                recovery_id: 1,
            }),
        }
    }

    /// A base58 32-byte Solana account, mixed case on purpose -- base58
    /// is case-*sensitive*, so this is one account and not a spelling of
    /// another.
    const CHANNEL_ACCOUNT: &str = "GDDMwNyyx8uB6zrqwBFHjLLG3TBYk2F1Mh6usnNPUsqk";

    /// A conforming inbound fixture: `programId` is the settlement program
    /// `CHANNEL_ACCOUNT` lives under, which is what a payer MUST write
    /// there (`client-edge-spec.md` §1.3, issue #1127). It used to be the
    /// **system program** -- no channel lives under that, so the fixture
    /// was an example of the one value the pinned rule excludes, sitting in
    /// the codec both edges share. Nothing here consults the field, so this
    /// changes no assertion; it stops the file teaching the wrong shape.
    fn solana_claim_json(signature: &str) -> String {
        serde_json::json!({
            "version": "1.0",
            "blockchain": "solana",
            "messageId": "m",
            "timestamp": "2030-01-01T00:00:00Z",
            "senderId": "s",
            "programId": SOLANA_PROGRAM_ID,
            "channelAccount": CHANNEL_ACCOUNT,
            "nonce": 1,
            "transferredAmount": "10",
            "signature": signature,
            "signerPublicKey": "11111111111111111111111111111113",
        })
        .to_string()
    }

    fn domain() -> PeerClaimDomain {
        PeerClaimDomain {
            chain_id: 84_532,
            token_network: [0x33; 20],
        }
    }

    /// ADR 0074 decision 1: a well-formed voucher -- on either chain -- is
    /// refused by every peer carriage, by name, and so can never become a
    /// `WireClaim` or decide the role `peer`. Both carriages decode through
    /// this one function (`accept::decode_claim`, `claim_on`).
    #[test]
    fn a_voucher_on_a_peer_carriage_is_refused_by_name() {
        let evm = serde_json::json!({
            "version": "1.0",
            "blockchain": "evm",
            "scheme": "batch-settlement",
            "messageId": "m",
            "timestamp": "2030-01-01T00:00:00Z",
            "senderId": "s",
            "channelId": format!("0x{:064x}", 7),
            "maxClaimableAmount": "12500",
            "signature": format!("0x{}1b", "ab".repeat(64)),
        });
        let solana = serde_json::json!({
            "version": "1.0",
            "blockchain": "solana",
            "scheme": "batch-settlement",
            "messageId": "m",
            "timestamp": "2030-01-01T00:00:00Z",
            "senderId": "s",
            "channelId": CHANNEL_ACCOUNT,
            "maxClaimableAmount": "10",
            "expiresAt": 0,
            "signature": "5VERv8NMvzbJMEkV8xnrLkEaWRtSz9CosKDYjCJjBRnbJLgp8uirBgmQpjKhoR4tjF3ZpRzrFmBV6UjKdiSZkQUW",
        });
        for voucher in [evm, solana] {
            assert_eq!(
                parse(voucher.to_string().as_bytes()),
                Err(ClaimDecodeError::Voucher),
                "{voucher}"
            );
        }
        assert!(ClaimDecodeError::Voucher
            .to_string()
            .contains("client edge only"));
    }

    /// I4, mechanically: what the peer carriage emits is what the *client
    /// edge's* validator accepts, and the value survives the round trip.
    #[test]
    fn an_emitted_claim_parses_back_through_the_client_edges_own_validator() {
        let claim = wire_claim();
        let json = encode(
            &claim,
            &[0x44; 20],
            None,
            None,
            Some(domain()),
            "0x…:4",
            "2030-01-01T00:00:00.000Z",
        );

        let parsed = parse(json.as_bytes()).expect("the client edge validator accepts it");

        assert_eq!(parsed, claim);
    }

    /// §4: the entry is raw UTF-8 JSON, not base64 -- base64 is an HTTP
    /// header artifact and appears nowhere on this carriage.
    #[test]
    fn the_claim_entry_carries_raw_utf8_json() {
        let json = encode(
            &wire_claim(),
            &[0x44; 20],
            None,
            None,
            Some(domain()),
            "m",
            "2030-01-01T00:00:00Z",
        );

        let entry = protocol_data(&json);

        assert_eq!(entry.name, CLAIM_PROTOCOL);
        assert_eq!(String::from_utf8(entry.data).expect("utf-8"), json);
    }

    /// §3.1/ADR 0024: `lockedAmount`/`locksRoot` are hashed as zeros, and
    /// the domain the counterparty verifies under rides the claim.
    #[test]
    fn an_emitted_claim_pins_zero_locks_and_carries_its_eip712_domain() {
        let json = encode(
            &wire_claim(),
            &[0x44; 20],
            None,
            None,
            Some(domain()),
            "m",
            "2030-01-01T00:00:00Z",
        );

        let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(value["lockedAmount"], "0");
        assert_eq!(value["locksRoot"], ZERO_BYTES32);
        assert_eq!(value["chainId"], 84_532);
        assert_eq!(
            value["tokenNetworkAddress"],
            format!("0x{}", hex::encode([0x33u8; 20]))
        );
        assert_eq!(value["blockchain"], "evm");
        assert_eq!(value["version"], "1.0");
    }

    /// §4.1: the channel id is canonicalised before it can reach a
    /// watermark, so one signed claim cannot buy carriage once per casing
    /// it was retyped in.
    #[test]
    fn a_recased_channel_id_canonicalises_to_one_watermark_key() {
        let upper = format!("0x{}", "AB".repeat(32));
        let lower = format!("0x{}", "ab".repeat(32));

        assert_eq!(canonical_evm_channel_id(&upper), lower);
        assert_eq!(canonical_evm_channel_id(&lower), lower);
        // Not a bytes32 at all: left alone, to fail structural validation
        // where the reason is legible.
        assert_eq!(canonical_evm_channel_id("CHANNEL-A"), "CHANNEL-A");
    }

    /// §4.2: `v` rides as libsecp256k1 emits it, unchanged in both
    /// directions.
    #[test]
    fn the_recovery_id_rides_raw_and_is_never_shifted_to_the_wallet_convention() {
        for recovery_id in [0u8, 1] {
            let ClaimSignature::Evm(base) = wire_claim().signature else {
                unreachable!("the fixture is an evm claim")
            };
            let claim = WireClaim {
                signature: ClaimSignature::Evm(Signature {
                    recovery_id,
                    ..base
                }),
                ..wire_claim()
            };
            let json = encode(
                &claim,
                &[0x44; 20],
                None,
                None,
                Some(domain()),
                "m",
                "2030-01-01T00:00:00Z",
            );
            let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
            let signature = value["signature"].as_str().expect("a string");
            assert_eq!(
                &signature[signature.len() - 2..],
                format!("{recovery_id:02x}")
            );

            assert_eq!(parse(json.as_bytes()).expect("round trip"), claim);
        }
    }

    /// The named regression (issue #732). Both live devnet peer configs
    /// carry `chain: solana:devnet`, and the TypeScript fleet's
    /// claim-receiver logs `blockchain: "solana"` every five minutes --
    /// so a carriage that refused the chain outright could not do the one
    /// thing the inter-node link actually does.
    #[test]
    fn a_solana_claim_parses_into_its_channel_account_nonce_amount_and_ed25519_signature() {
        let signature = [0x5au8; 64];
        let json = solana_claim_json(&BASE64.encode(signature));

        assert_eq!(
            parse(json.as_bytes()),
            Ok(WireClaim {
                channel_id: CHANNEL_ACCOUNT.to_string(),
                nonce: 1,
                cumulative_amount: 10,
                signature: ClaimSignature::Solana(signature),
            })
        );
    }

    /// §4.1's canonicalisation is EVM-only, deliberately: base58 of an
    /// exact 32-byte decode has one spelling, and case-folding it would
    /// merge accounts that are not the same account.
    #[test]
    fn a_solana_channel_account_reaches_the_watermark_key_verbatim() {
        let json = solana_claim_json(&BASE64.encode([0x5au8; 64]));

        let parsed = parse(json.as_bytes()).expect("a valid solana claim");

        assert_eq!(parsed.channel_id, CHANNEL_ACCOUNT);
        assert_ne!(parsed.channel_id, CHANNEL_ACCOUNT.to_ascii_lowercase());
    }

    /// Issue #742: the outbound half. `ClaimBook::record_fulfillment` can
    /// now produce a `ClaimSignature::Solana` `WireClaim`; this is the
    /// same round trip `an_emitted_claim_parses_back_through_the_client_edges_own_validator`
    /// proves for EVM, over the Solana arm `encode` used to `unreachable!`
    /// on.
    fn wire_solana_claim() -> WireClaim {
        WireClaim {
            channel_id: CHANNEL_ACCOUNT.to_string(),
            nonce: 4,
            cumulative_amount: 12_500,
            signature: ClaimSignature::Solana([0x5au8; 64]),
        }
    }

    const SIGNER_PUBLIC_KEY: [u8; 32] = [0x77u8; 32];

    /// A real base58-encoded 32-byte Solana program id (the deployed SPL
    /// Token program's, reused here only as a well-formed fixture) --
    /// issue #759's config-sourced replacement for the deleted
    /// `PLACEHOLDER_SOLANA_PROGRAM_ID`.
    const SOLANA_PROGRAM_ID: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

    #[test]
    fn a_solana_claim_encodes_and_parses_back_through_the_client_edges_own_validator() {
        let claim = wire_solana_claim();
        let json = encode(
            &claim,
            &[0x44; 20],
            Some(&SIGNER_PUBLIC_KEY),
            Some(SOLANA_PROGRAM_ID),
            None,
            "m",
            "2030-01-01T00:00:00Z",
        );

        let parsed = parse(json.as_bytes()).expect("the client edge validator accepts it");

        assert_eq!(parsed, claim);
    }

    /// Issue #759's AC, mechanically: a rendered outbound Solana claim
    /// carries the *configured* `programId` -- never the deleted
    /// `PLACEHOLDER_SOLANA_PROGRAM_ID` -- and carries no EIP-712 domain,
    /// since there is none to render. `senderId`/`signerPublicKey` are this
    /// connector's own ed25519 identity, base58, never the EVM
    /// `signer_address` also passed in.
    #[test]
    fn a_solana_claim_carries_its_configured_program_id_and_no_evm_domain() {
        let json = encode(
            &wire_solana_claim(),
            &[0x44; 20],
            Some(&SIGNER_PUBLIC_KEY),
            Some(SOLANA_PROGRAM_ID),
            Some(domain()),
            "m",
            "2030-01-01T00:00:00Z",
        );

        let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let expected = bs58::encode(SIGNER_PUBLIC_KEY).into_string();
        assert_eq!(value["blockchain"], "solana");
        assert_eq!(value["senderId"], expected);
        assert_eq!(value["signerPublicKey"], expected);
        assert_eq!(value["programId"], SOLANA_PROGRAM_ID);
        assert!(value.get("chainId").is_none());
        assert!(value.get("tokenNetworkAddress").is_none());
    }

    /// The `unreachable!()` this replaced (issue #699/#742) treated a
    /// Solana claim as impossible to receive at all; now that `ClaimBook`
    /// can sign one, a transport that was never given an identity to
    /// render it under is a caller bug, not a silent no-op -- caught here
    /// rather than shipping a claim with no `signerPublicKey`.
    #[test]
    #[should_panic(expected = "no solana_signer_public_key configured")]
    fn rendering_a_solana_claim_with_no_configured_identity_panics() {
        encode(
            &wire_solana_claim(),
            &[0x44; 20],
            None,
            Some(SOLANA_PROGRAM_ID),
            None,
            "m",
            "2030-01-01T00:00:00Z",
        );
    }

    /// Issue #759's counterpart of the panic above: `programId` is a
    /// required wire field (unlike an EVM claim's optional `chainId`), so a
    /// Solana claim rendered for a channel with no `[[peer_channels]]`
    /// program id is exactly as much a caller bug as one with no signing
    /// identity -- caught here rather than reintroducing a placeholder.
    #[test]
    #[should_panic(expected = "no solana_program_id")]
    fn rendering_a_solana_claim_with_no_configured_program_id_panics() {
        encode(
            &wire_solana_claim(),
            &[0x44; 20],
            Some(&SIGNER_PUBLIC_KEY),
            None,
            None,
            "m",
            "2030-01-01T00:00:00Z",
        );
    }

    /// The signature is base64 of exactly 64 ed25519 bytes -- the client
    /// edge's own encoding (I4). Neither hex, nor a short string padded
    /// into shape.
    #[test]
    fn a_solana_signature_that_is_not_base64_of_64_bytes_is_refused() {
        for bad in [
            BASE64.encode([0x5au8; 63]),
            BASE64.encode([0x5au8; 65]),
            "not base64!!".to_string(),
            format!("0x{}", hex::encode([0x5au8; 64])),
        ] {
            assert_eq!(
                parse(solana_claim_json(&bad).as_bytes()),
                Err(ClaimDecodeError::Signature),
                "accepted {bad:?}"
            );
        }
    }

    /// ADR 0002 drops Mina from the Rust connector, and #732 does not
    /// bring it back: `mina` is still refused by chain, and still
    /// distinguishably from a malformed claim.
    #[test]
    fn a_mina_claim_is_still_refused_by_chain_after_solana_landed() {
        let json = serde_json::json!({
            "version": "1.0",
            "blockchain": "mina",
            "messageId": "m",
            "timestamp": "2030-01-01T00:00:00Z",
            "senderId": "s",
        })
        .to_string();

        assert_eq!(
            parse(json.as_bytes()),
            Err(ClaimDecodeError::UnsupportedChain("mina"))
        );
    }
    #[test]
    fn a_malformed_claim_is_refused_with_the_validators_own_reason() {
        assert!(matches!(
            parse(b"{\"version\":\"2.0\"}"),
            Err(ClaimDecodeError::Structural(_))
        ));
        assert_eq!(parse(&[0xff, 0xfe]), Err(ClaimDecodeError::NotUtf8));
    }

    #[test]
    fn a_short_signature_is_refused_rather_than_zero_padded() {
        let json = encode(
            &wire_claim(),
            &[0x44; 20],
            None,
            None,
            Some(domain()),
            "m",
            "2030-01-01T00:00:00Z",
        )
        .replace(
            &format!("0x{}", hex::encode(wire_claim().signature.to_bytes())),
            "0xdeadbeef",
        );

        assert_eq!(parse(json.as_bytes()), Err(ClaimDecodeError::Signature));
    }
}
