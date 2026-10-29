//! XRPL's claim wire (ADR 0087 decisions 2 and 3, issue #1611, schema 12):
//! the ledger's own `PaymentChannelClaim` authorisation, the offline channel
//! id, and the claim-state challenge a channel's key signs.
//!
//! Every key here is a published development key -- a family seed anyone can
//! read -- never a real one. Every figure is computed through the same
//! `connector_signer` functions a node uses and **self-verified** before it is
//! emitted. The channel-id derivation and the Ed25519 claim were also
//! compared byte for byte with xrpl.js 4 (`hashPaymentChannel`,
//! `signPaymentChannelClaim`), and a secp256k1 claim signed by xrpl.js
//! verifies here: see `connector_signer`'s `xrpl` tests.

use connector_domain::client_claim::{
    self, ClientClaim, ClientClaimError, SCHEME_BATCH_SETTLEMENT, XRPL_MAX_DROPS,
};
use connector_domain::{Network, XrplNetwork};
use connector_peer_btp::challenge_json::{self, PeerRoleChallenge};
use connector_signer::{
    decode_xrpl_classic_address, sha512_half, verify_xrpl_claim,
    verify_xrpl_voucher_claim_state_challenge, xrpl_channel_id, xrpl_claim_message,
    xrpl_family_seed, xrpl_voucher_claim_state_challenge_message, XrplAlgorithm, XrplError,
    XrplSigner,
};
use serde::Serialize;

use super::{
    client_auth_channel_challenge_frame, hex_of, parse_payout_entry, payout_transfer, role_gate,
    ClientAuthChannelChallengeCase, VoucherInvalidClaimCase, AUTH_CHALLENGE_NOW,
    AUTH_CHALLENGE_PEER_ID, FIXTURE_TIMESTAMP, VOUCHER_CLAIM_STATE_EXPIRES,
};

/// The one network every XRPL case but the cross-network ones is on.
const NETWORK: XrplNetwork = XrplNetwork::Testnet;

/// rippled's genesis account, `masterpassphrase`: secp256k1.
const SECP256K1_ENTROPY_SEED: &str = "snoPBrXtMeMyMHUVTgbuqAfg1SUTb";

/// 16 bytes of `0x07`: Ed25519.
const ED25519_ENTROPY: [u8; 16] = [0x07; 16];

/// A key no fixture channel uses: "wrong key" cases.
const OTHER_ENTROPY: [u8; 16] = [0x42; 16];

/// The connector's own XRPL settlement key, which signs payouts.
const CONNECTOR_ENTROPY: [u8; 16] = [0x61; 16];

fn network() -> Network {
    Network::Xrpl(NETWORK)
}

fn secp_signer() -> XrplSigner {
    XrplSigner::from_family_seed(SECP256K1_ENTROPY_SEED).expect("the fixture seed is a seed")
}

fn ed_signer() -> XrplSigner {
    signer_from(XrplAlgorithm::Ed25519, &ED25519_ENTROPY)
}

fn signer_from(algorithm: XrplAlgorithm, entropy: &[u8; 16]) -> XrplSigner {
    XrplSigner::from_family_seed(&xrpl_family_seed(algorithm, entropy))
        .expect("a seed this crate encoded decodes")
}

fn seed_of(algorithm: XrplAlgorithm, entropy: &[u8; 16]) -> String {
    xrpl_family_seed(algorithm, entropy)
}

fn algorithm_name(algorithm: XrplAlgorithm) -> &'static str {
    match algorithm {
        XrplAlgorithm::Secp256k1 => "secp256k1",
        XrplAlgorithm::Ed25519 => "ed25519",
    }
}

/// A fixture key, as the vector publishes it.
#[derive(Debug, Serialize)]
pub struct XrplKeyFacts {
    /// `secp256k1` or `ed25519`: what the family seed's encoding names.
    pub algorithm: &'static str,
    /// The family seed, exactly as `wallet_propose` emits it -- what a key
    /// file holds.
    pub family_seed: String,
    /// The channel `PublicKey`: 33 bytes, compressed secp256k1 or `ED` ‖
    /// Ed25519, uppercase hex as the ledger spells it. A voucher signer is
    /// spelled this way in config.
    pub public_key_hex: String,
    /// `RIPEMD160(SHA256(public key))`.
    pub account_id_hex: String,
    /// The classic `r…` address: `payTo`.
    pub classic_address: String,
}

fn key_facts(family_seed: String, signer: &XrplSigner) -> XrplKeyFacts {
    XrplKeyFacts {
        algorithm: algorithm_name(signer.algorithm()),
        family_seed,
        public_key_hex: hex_of(&signer.public_key()).to_ascii_uppercase(),
        account_id_hex: hex_of(&signer.account_id()),
        classic_address: signer.classic_address(),
    }
}

// ---------------------------------------------------------------------
// Channel ids
// ---------------------------------------------------------------------

/// `SHA512Half(0x0078 ‖ source ‖ destination ‖ sequence)`, derived offline.
#[derive(Debug, Serialize)]
pub struct XrplChannelIdCase {
    pub name: &'static str,
    pub source_classic_address: String,
    pub source_account_id_hex: String,
    pub destination_classic_address: String,
    pub destination_account_id_hex: String,
    /// The creating transaction's `Sequence`, a `u32`.
    pub sequence: u32,
    /// `0x0078 ‖ source ‖ destination ‖ sequence (u32 BE)`: 46 bytes.
    pub preimage_hex: String,
    pub channel_id_hex: String,
}

fn channel_id_case(
    name: &'static str,
    source: [u8; 20],
    destination: [u8; 20],
    sequence: u32,
    source_address: String,
    destination_address: String,
) -> XrplChannelIdCase {
    let mut preimage = vec![0x00, 0x78];
    preimage.extend_from_slice(&source);
    preimage.extend_from_slice(&destination);
    preimage.extend_from_slice(&sequence.to_be_bytes());
    let channel_id = xrpl_channel_id(&source, &destination, sequence);
    assert_eq!(channel_id, sha512_half(&preimage));
    XrplChannelIdCase {
        name,
        source_classic_address: source_address,
        source_account_id_hex: hex_of(&source),
        destination_classic_address: destination_address,
        destination_account_id_hex: hex_of(&destination),
        sequence,
        preimage_hex: hex_of(&preimage),
        channel_id_hex: hex_of(&channel_id),
    }
}

fn generate_channel_id_cases() -> Vec<XrplChannelIdCase> {
    // Compared with xrpl.js's `hashPaymentChannel` when this was written.
    let reference_source = "rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh";
    let reference_destination = "rLNaPoKeeBjZe2qs6x52yVPZpZ8td4dc6w";
    let reference = channel_id_case(
        "xrpl_channel_id_reference",
        decode_xrpl_classic_address(reference_source).unwrap(),
        decode_xrpl_classic_address(reference_destination).unwrap(),
        5,
        reference_source.to_string(),
        reference_destination.to_string(),
    );
    assert_eq!(
        reference.channel_id_hex,
        "d53c24f11bea8ec2256fcb3f85371265dacb031b39d1da676a22393497c0ed9f"
    );
    let (secp, ed) = (secp_signer(), ed_signer());
    vec![
        reference,
        channel_id_case(
            "xrpl_channel_id_secp256k1_pays_ed25519",
            secp.account_id(),
            ed.account_id(),
            SECP_CHANNEL_SEQUENCE,
            secp.classic_address(),
            ed.classic_address(),
        ),
        channel_id_case(
            "xrpl_channel_id_ed25519_pays_secp256k1",
            ed.account_id(),
            secp.account_id(),
            ED_CHANNEL_SEQUENCE,
            ed.classic_address(),
            secp.classic_address(),
        ),
    ]
}

/// The `Sequence` each claim fixture's channel was "created" at.
const SECP_CHANNEL_SEQUENCE: u32 = 7;
const ED_CHANNEL_SEQUENCE: u32 = 8;

/// The channel `payer` opened toward `payee` at `sequence`.
fn channel_between(payer: &XrplSigner, payee: &XrplSigner, sequence: u32) -> [u8; 32] {
    xrpl_channel_id(&payer.account_id(), &payee.account_id(), sequence)
}

// ---------------------------------------------------------------------
// Claims
// ---------------------------------------------------------------------

/// One XRPL voucher: the claim JSON, the 44-byte message its signature
/// covers, and the signature.
#[derive(Debug, Serialize)]
pub struct VoucherXrplCase {
    pub name: &'static str,
    /// The CAIP-2 network the voucher names. The signed message binds none.
    pub network: String,
    #[serde(flatten)]
    pub key: XrplKeyFacts,
    /// The account that opened the channel; the voucher's `senderId`.
    pub payer_classic_address: String,
    /// The channel's destination: the node the voucher pays.
    pub destination_classic_address: String,
    /// The creating transaction's `Sequence`.
    pub channel_sequence: u32,
    pub channel_id_hex: String,
    /// A decimal string, in drops, at most 10^17.
    pub max_claimable_amount: String,
    /// `"CLM\0" ‖ channelId ‖ drops (u64 BE)`: 44 bytes. What an Ed25519
    /// signature covers raw and a secp256k1 one covers as `SHA512Half`.
    pub signed_message_hex: String,
    /// `SHA512Half(signed_message)` for secp256k1; `null` for Ed25519, which
    /// signs the message itself.
    pub signed_digest_hex: Option<String>,
    /// DER (secp256k1, low-`s`) or 64 raw bytes (Ed25519).
    pub signature_hex: String,
    /// The full claim, as it rides the `ILP-Payment-Channel-Claim`
    /// header/protocolData entry.
    pub json: String,
}

fn voucher_case(
    name: &'static str,
    message_id: &str,
    payer: (&XrplSigner, String),
    payee: &XrplSigner,
    sequence: u32,
    amount: u128,
) -> VoucherXrplCase {
    let (payer, seed) = payer;
    let channel_id = channel_between(payer, payee, sequence);
    let message = xrpl_claim_message(&channel_id, amount).expect("a fixture amount is in range");
    let signature = payer
        .sign_claim(&channel_id, amount)
        .expect("a fixture amount is in range");
    assert!(
        verify_xrpl_claim(&payer.public_key(), &channel_id, amount, &signature),
        "the fixture voucher's signature verifies against its own channel key"
    );
    let json = serde_json::json!({
        "version": "1.0",
        "blockchain": "xrpl",
        "network": network().to_string(),
        "scheme": SCHEME_BATCH_SETTLEMENT,
        "messageId": message_id,
        "timestamp": FIXTURE_TIMESTAMP,
        "senderId": payer.classic_address(),
        "channelId": hex_of(&channel_id),
        "maxClaimableAmount": amount.to_string(),
        "signature": hex_of(&signature),
    })
    .to_string();
    let parsed = client_claim::parse_client_claim(&json).expect("the emitted voucher parses");
    let ClientClaim::XrplVoucher(voucher) = &parsed else {
        panic!("expected an XRPL voucher, got {parsed:?}");
    };
    assert_eq!(voucher.channel_id, hex_of(&channel_id));
    assert_eq!(voucher.network, network());
    assert_eq!(voucher.max_claimable_amount, amount);
    VoucherXrplCase {
        name,
        network: network().to_string(),
        key: key_facts(seed, payer),
        payer_classic_address: payer.classic_address(),
        destination_classic_address: payee.classic_address(),
        channel_sequence: sequence,
        channel_id_hex: hex_of(&channel_id),
        max_claimable_amount: amount.to_string(),
        signed_message_hex: hex_of(&message),
        signed_digest_hex: (payer.algorithm() == XrplAlgorithm::Secp256k1)
            .then(|| hex_of(&sha512_half(&message))),
        signature_hex: hex_of(&signature),
        json,
    }
}

fn secp_voucher() -> VoucherXrplCase {
    voucher_case(
        "claim_voucher_xrpl_secp256k1",
        "vector-fixture:voucher:xrpl:secp256k1",
        (&secp_signer(), SECP256K1_ENTROPY_SEED.to_string()),
        &ed_signer(),
        SECP_CHANNEL_SEQUENCE,
        5_000_000,
    )
}

fn ed_voucher() -> VoucherXrplCase {
    voucher_case(
        "claim_voucher_xrpl_ed25519",
        "vector-fixture:voucher:xrpl:ed25519",
        (
            &ed_signer(),
            seed_of(XrplAlgorithm::Ed25519, &ED25519_ENTROPY),
        ),
        &secp_signer(),
        ED_CHANNEL_SEQUENCE,
        5_000_000,
    )
}

/// An amount above the 10^17-drop ceiling: refused by name when parsed, and
/// no signature exists to refuse, since signing refuses it too.
#[derive(Debug, Serialize)]
pub struct XrplAboveCeilingCase {
    pub name: &'static str,
    /// The ceiling, a decimal string: XRP's total supply in drops.
    pub max_drops: String,
    /// One above it, a decimal string. Inside a `u64`, so a reader that
    /// truncated to `u64` would still not catch it: the ceiling is lower.
    pub amount: String,
    /// `u64::MAX + 1`, a decimal string: past even the wire's `u64`.
    pub amount_above_u64: String,
    /// `claim_voucher.xrpl_secp256k1`'s claim with `amount` swapped in.
    pub claim_json: String,
    /// The same claim at `amount_above_u64`.
    pub claim_above_u64_json: String,
    /// What both are refused as.
    pub expected_error: &'static str,
    /// Signing `amount` is refused too: no `CLM\0` message exists for it.
    pub signing_refused: bool,
    /// The ceiling itself is accepted: the case that proves the boundary.
    pub at_ceiling_claim_json: String,
}

pub(crate) fn above_ceiling_case(secp: &VoucherXrplCase) -> XrplAboveCeilingCase {
    assert_eq!(
        XRPL_MAX_DROPS,
        connector_signer::XRPL_MAX_DROPS,
        "the domain and the signer agree on the ceiling"
    );
    let with_amount = |amount: u128| {
        let mut claim: serde_json::Value = serde_json::from_str(&secp.json).expect("JSON");
        claim["maxClaimableAmount"] = amount.to_string().into();
        claim.to_string()
    };
    let max = u128::from(XRPL_MAX_DROPS);
    let above = max + 1;
    let above_u64 = u128::from(u64::MAX) + 1;
    for amount in [above, above_u64] {
        assert_eq!(
            client_claim::parse_client_claim(&with_amount(amount)),
            Err(ClientClaimError::AmountExceedsChain {
                amount,
                max: XRPL_MAX_DROPS
            })
        );
        assert!(matches!(
            xrpl_claim_message(&[0; 32], amount),
            Err(XrplError::AmountExceedsChain { .. })
        ));
    }
    assert!(client_claim::parse_client_claim(&with_amount(max)).is_ok());
    XrplAboveCeilingCase {
        name: "claim_voucher_xrpl_above_ceiling",
        max_drops: max.to_string(),
        amount: above.to_string(),
        amount_above_u64: above_u64.to_string(),
        claim_json: with_amount(above),
        claim_above_u64_json: with_amount(above_u64),
        expected_error: "amount_exceeds_chain",
        signing_refused: true,
        at_ceiling_claim_json: with_amount(max),
    }
}

/// An XRPL voucher with no `network`: refused by name, as on every chain.
fn network_missing_case(secp: &VoucherXrplCase) -> VoucherInvalidClaimCase {
    let mut claim: serde_json::Value = serde_json::from_str(&secp.json).expect("JSON");
    claim.as_object_mut().unwrap().remove("network");
    let claim_json = claim.to_string();
    assert_eq!(
        client_claim::parse_client_claim(&claim_json),
        Err(ClientClaimError::NetworkMissing)
    );
    VoucherInvalidClaimCase {
        name: "claim_voucher_xrpl_network_missing",
        claim_json,
        expected_error: "network_missing",
    }
}

/// `claim_voucher`'s XRPL half.
#[derive(Debug, Serialize)]
pub struct ClaimVoucherXrplVectors {
    pub secp256k1: VoucherXrplCase,
    pub ed25519: VoucherXrplCase,
    /// Offline `PayChan` id derivation.
    pub channel_id: Vec<XrplChannelIdCase>,
    /// The same signed claim names a different network and verifies all the
    /// same: a claim binds none (ADR 0087 decision 4).
    pub claim_binds_no_network: XrplClaimBindsNoNetworkCase,
    pub invalid: Vec<VoucherInvalidClaimCase>,
}

#[derive(Debug, Serialize)]
pub struct XrplClaimBindsNoNetworkCase {
    pub name: &'static str,
    /// `secp256k1.json` relabelled onto mainnet.
    pub claim_json: String,
    pub network: String,
    /// Whether the signature still verifies: it does, which is why the
    /// account, not the claim, is bound to a network.
    pub signature_verifies: bool,
}

pub(crate) fn generate_claim_voucher_xrpl_vectors() -> ClaimVoucherXrplVectors {
    let secp256k1 = secp_voucher();
    let ed25519 = ed_voucher();
    let mut relabelled: serde_json::Value = serde_json::from_str(&secp256k1.json).expect("JSON");
    relabelled["network"] = Network::Xrpl(XrplNetwork::Mainnet).to_string().into();
    let claim_json = relabelled.to_string();
    let parsed = client_claim::parse_client_claim(&claim_json).expect("parses on mainnet too");
    assert_eq!(parsed.network(), &Network::Xrpl(XrplNetwork::Mainnet));
    let invalid = vec![network_missing_case(&secp256k1)];
    ClaimVoucherXrplVectors {
        channel_id: generate_channel_id_cases(),
        claim_binds_no_network: XrplClaimBindsNoNetworkCase {
            name: "claim_voucher_xrpl_binds_no_network",
            claim_json,
            network: parsed.network().to_string(),
            signature_verifies: true,
        },
        invalid,
        secp256k1,
        ed25519,
    }
}

// ---------------------------------------------------------------------
// Payout
// ---------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct PayoutVoucherXrplCase {
    pub name: &'static str,
    pub network: String,
    #[serde(flatten)]
    pub key: XrplKeyFacts,
    pub channel_id_hex: String,
    pub max_claimable_amount: String,
    pub signed_message_hex: String,
    pub signature_hex: String,
    pub json: String,
    pub btp_transfer_hex: String,
}

pub(crate) fn generate_payout_voucher_xrpl_case() -> PayoutVoucherXrplCase {
    let connector = signer_from(XrplAlgorithm::Secp256k1, &CONNECTOR_ENTROPY);
    let client = ed_signer();
    let channel_id = channel_between(&connector, &client, 9);
    let amount: u64 = 42_000;
    let message = xrpl_claim_message(&channel_id, u128::from(amount)).expect("in range");
    let signature = connector
        .sign_claim(&channel_id, u128::from(amount))
        .expect("in range");
    assert!(verify_xrpl_claim(
        &connector.public_key(),
        &channel_id,
        u128::from(amount),
        &signature
    ));
    let json = serde_json::to_string(&serde_json::json!({
        "network": network().to_string(),
        "scheme": SCHEME_BATCH_SETTLEMENT,
        "blockchain": "xrpl",
        "channelId": hex_of(&channel_id),
        "maxClaimableAmount": amount.to_string(),
        "signature": hex_of(&signature),
    }))
    .expect("a json! object serializes");
    assert!(matches!(
        parse_payout_entry(&json),
        ClientClaim::XrplVoucher(_)
    ));
    PayoutVoucherXrplCase {
        name: "payout_voucher_xrpl",
        network: network().to_string(),
        key: key_facts(
            seed_of(XrplAlgorithm::Secp256k1, &CONNECTOR_ENTROPY),
            &connector,
        ),
        channel_id_hex: hex_of(&channel_id),
        max_claimable_amount: amount.to_string(),
        signed_message_hex: hex_of(&message),
        signature_hex: hex_of(&signature),
        btp_transfer_hex: hex_of(&payout_transfer(9_503, amount, &json)),
        json,
    }
}

// ---------------------------------------------------------------------
// Claim-state challenge
// ---------------------------------------------------------------------

/// One XRPL claim-state entry.
#[derive(Debug, Serialize)]
pub struct VoucherClaimStateXrplCase {
    pub name: &'static str,
    /// The CAIP-2 network the entry names, and the challenge binds.
    pub network: String,
    /// The channel key the challenge must verify against.
    pub channel_public_key_hex: String,
    pub channel_id_hex: String,
    pub expires: u64,
    /// The seed of the key that signed (the channel's, or a wrong one).
    pub signer_family_seed: String,
    /// The network the signer signed for, when it is not the entry's.
    pub signed_for_network: String,
    /// `"toon-xrpl-voucher-claim-state-challenge-v1" ‖ len(network): u8 ‖
    /// network ‖ channelId ‖ expires (u64 BE)` -- what the signature covers
    /// (raw for Ed25519, as `SHA512Half` for secp256k1).
    pub signed_message_hex: String,
    pub signature_hex: String,
    /// Whether the signature verifies against the channel key for the entry's
    /// network.
    pub signature_verifies: bool,
    /// Whether the same signature verifies as a *claim* on this channel for
    /// `expires` drops. Always `false`: a challenge is never a claim.
    pub verifies_as_claim: bool,
    pub entry_json: String,
}

fn claim_state_case(
    name: &'static str,
    channel_key: &XrplSigner,
    channel_id: [u8; 32],
    signer: (&XrplSigner, String),
    signed_for: Network,
    expect_verifies: bool,
) -> VoucherClaimStateXrplCase {
    let (signer, signer_seed) = signer;
    let expires = VOUCHER_CLAIM_STATE_EXPIRES;
    let message =
        xrpl_voucher_claim_state_challenge_message(&signed_for.to_string(), &channel_id, expires)
            .expect("a short network");
    let signature = signer.sign(&message);
    let entry_network = network().to_string();
    let signature_verifies = verify_xrpl_voucher_claim_state_challenge(
        &channel_key.public_key(),
        &entry_network,
        &channel_id,
        expires,
        &signature,
    );
    assert_eq!(
        signature_verifies, expect_verifies,
        "vector {name} computed the wrong verification verdict"
    );
    let verifies_as_claim = verify_xrpl_claim(
        &channel_key.public_key(),
        &channel_id,
        u128::from(expires),
        &signature,
    );
    assert!(!verifies_as_claim, "a challenge is never a claim");
    let entry_json = serde_json::json!({
        "blockchain": "xrpl",
        "network": entry_network,
        "scheme": SCHEME_BATCH_SETTLEMENT,
        "channelId": hex_of(&channel_id),
        "expires": expires,
        "signature": hex_of(&signature),
    })
    .to_string();
    VoucherClaimStateXrplCase {
        name,
        network: entry_network,
        channel_public_key_hex: hex_of(&channel_key.public_key()).to_ascii_uppercase(),
        channel_id_hex: hex_of(&channel_id),
        expires,
        signer_family_seed: signer_seed,
        signed_for_network: signed_for.to_string(),
        signed_message_hex: hex_of(&message),
        signature_hex: hex_of(&signature),
        signature_verifies,
        verifies_as_claim,
        entry_json,
    }
}

pub(crate) fn generate_voucher_claim_state_xrpl_cases() -> Vec<VoucherClaimStateXrplCase> {
    let (secp, ed) = (secp_signer(), ed_signer());
    let secp_seed = SECP256K1_ENTROPY_SEED.to_string();
    let ed_seed = seed_of(XrplAlgorithm::Ed25519, &ED25519_ENTROPY);
    let other = signer_from(XrplAlgorithm::Secp256k1, &OTHER_ENTROPY);
    let other_seed = seed_of(XrplAlgorithm::Secp256k1, &OTHER_ENTROPY);
    let secp_channel = channel_between(&secp, &ed, SECP_CHANNEL_SEQUENCE);
    let ed_channel = channel_between(&ed, &secp, ED_CHANNEL_SEQUENCE);
    vec![
        claim_state_case(
            "voucher_claim_state_xrpl_secp256k1_valid",
            &secp,
            secp_channel,
            (&secp, secp_seed.clone()),
            network(),
            true,
        ),
        claim_state_case(
            "voucher_claim_state_xrpl_ed25519_valid",
            &ed,
            ed_channel,
            (&ed, ed_seed),
            network(),
            true,
        ),
        claim_state_case(
            "voucher_claim_state_xrpl_wrong_key",
            &secp,
            secp_channel,
            (&other, other_seed),
            network(),
            false,
        ),
        // A challenge signed for mainnet proves nothing on testnet, though a
        // claim would verify on both.
        claim_state_case(
            "voucher_claim_state_xrpl_other_network",
            &secp,
            secp_channel,
            (&secp, secp_seed),
            Network::Xrpl(XrplNetwork::Mainnet),
            false,
        ),
    ]
}

// ---------------------------------------------------------------------
// Client BTP auth's channelChallenge
// ---------------------------------------------------------------------

fn client_auth_case(
    name: &'static str,
    request_id: u32,
    expires: u64,
    accepted: bool,
) -> ClientAuthChannelChallengeCase {
    let (secp, ed) = (secp_signer(), ed_signer());
    let channel_id = channel_between(&secp, &ed, SECP_CHANNEL_SEQUENCE);
    let message =
        xrpl_voucher_claim_state_challenge_message(&network().to_string(), &channel_id, expires)
            .expect("a short network");
    let signature = secp.sign(&message);
    assert!(
        verify_xrpl_voucher_claim_state_challenge(
            &secp.public_key(),
            &network().to_string(),
            &channel_id,
            expires,
            &signature,
        ),
        "vector {name}'s challenge must genuinely verify against the channel's key"
    );
    let in_window = role_gate::challenge_in_window(expires, AUTH_CHALLENGE_NOW);
    assert_eq!(
        in_window, accepted,
        "vector {name} computed the wrong window verdict"
    );
    let challenge = PeerRoleChallenge::Xrpl {
        network: network(),
        channel_id,
        expires,
        signature,
    };
    let challenge_json_text = challenge_json::encode(&challenge);
    let parsed = challenge_json::parse(challenge_json_text.as_bytes())
        .expect("the channelChallenge parses through the real peer-carriage reader");
    assert_eq!(parsed, challenge);
    assert_eq!(parsed.channel(), hex_of(&channel_id));
    let challenge_value: serde_json::Value =
        serde_json::from_str(&challenge_json_text).expect("encode makes JSON");
    let auth_entry = serde_json::json!({
        "peerId": AUTH_CHALLENGE_PEER_ID,
        "secret": "",
        "channelChallenge": challenge_value,
    });
    ClientAuthChannelChallengeCase {
        name,
        blockchain: "xrpl",
        now: AUTH_CHALLENGE_NOW,
        expires,
        accepted,
        challenge_json: challenge_value.to_string(),
        auth_entry_json: auth_entry.to_string(),
        btp_message_hex: hex_of(&client_auth_channel_challenge_frame(
            request_id,
            &auth_entry,
        )),
    }
}

pub(crate) fn generate_client_auth_xrpl_cases() -> Vec<ClientAuthChannelChallengeCase> {
    vec![
        client_auth_case(
            "client_auth_channel_challenge_xrpl_accepted",
            9_607,
            AUTH_CHALLENGE_NOW + 60,
            true,
        ),
        client_auth_case(
            "client_auth_channel_challenge_xrpl_refused_expired",
            9_608,
            AUTH_CHALLENGE_NOW - 1,
            false,
        ),
        client_auth_case(
            "client_auth_channel_challenge_xrpl_refused_too_far_ahead",
            9_609,
            AUTH_CHALLENGE_NOW + role_gate::MAX_PEER_CHALLENGE_LIFETIME_SECS + 1,
            false,
        ),
    ]
}
