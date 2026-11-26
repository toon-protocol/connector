//! The Solana scheme, through its interface: the committed wire vectors
//! (ADR 0021) and the scheme's properties.

use super::*;
use crate::LocalEd25519Signer;
use base64::Engine as _;
use connector_domain::client_claim::{parse_client_claim, ClientClaim};
use proptest::prelude::*;
use serde_json::Value;

const WIRE_VECTORS: &str = include_str!("../../../../../vectors/wire-vectors.json");

fn vectors() -> Value {
    serde_json::from_str(WIRE_VECTORS).expect("the committed vectors are JSON")
}

fn base58<const N: usize>(value: &Value) -> [u8; N] {
    bs58::decode(value.as_str().expect("a base58 string"))
        .into_vec()
        .expect("base58")
        .try_into()
        .expect("the vector's width")
}

fn hex_bytes(value: &Value) -> Vec<u8> {
    let text = value.as_str().expect("a hex string");
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

/// The envelope a voucher rides in, around the scheme's own fields, where
/// the vector carries none (a payout's protocol data names only the
/// network and scheme).
fn enveloped(mut fields: serde_json::Map<String, Value>) -> String {
    for (field, value) in [
        ("version", "1.0"),
        ("network", "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1"),
        ("scheme", "batch-settlement"),
        ("messageId", "m"),
        ("timestamp", "2030-01-01T00:00:00.000Z"),
        ("senderId", "s"),
    ] {
        fields.entry(field).or_insert_with(|| value.into());
    }
    Value::Object(fields).to_string()
}

fn wire(json: &str) -> SolanaVoucher {
    match parse_client_claim(json).expect("the vector parses") {
        ClientClaim::SolanaVoucher(voucher) => voucher,
        other => panic!("a Solana voucher, not {other:?}"),
    }
}

/// Each valid Solana voucher vector: its name, its JSON, its wire voucher,
/// the channel the chain records for it, and the 50 bytes it signs.
fn solana_vouchers() -> Vec<(String, Value, SolanaVoucher, RecordedSolanaChannel, Vec<u8>)> {
    let vectors = vectors();
    [
        ("claim_voucher", "solana", "signer_public_key_base58"),
        (
            "peer_carriage",
            "voucher_solana",
            "authorized_signer_base58",
        ),
        ("payout_voucher", "solana", "authorized_signer_base58"),
    ]
    .into_iter()
    .map(|(group, name, signer)| {
        let vector = &vectors[group][name];
        let json: Value =
            serde_json::from_str(vector["json"].as_str().expect("json")).expect("json");
        let voucher = wire(&enveloped(json.as_object().expect("an object").clone()));
        let recorded = RecordedSolanaChannel {
            channel_account: base58::<32>(&vector["channel_account_base58"]),
            authorized_signer: base58::<32>(&vector[signer]),
        };
        let message = hex_bytes(&vector["signed_message_hex"]);
        (format!("{group}.{name}"), json, voucher, recorded, message)
    })
    .collect()
}

#[test]
fn every_valid_solana_voucher_vector_parses_and_verifies() {
    let domain = SolanaVoucherDomain::payment_channels();
    for (name, json, wire, recorded, message) in solana_vouchers() {
        let voucher = SolanaVoucherScheme::parse(&wire).expect("parses");
        assert_eq!(voucher.channel_account, recorded.channel_account, "{name}");
        assert_eq!(
            voucher.max_claimable_amount.to_string(),
            json["maxClaimableAmount"].as_str().expect("amount"),
            "{name}"
        );
        assert_eq!(
            voucher.signature,
            base58::<64>(&json["signature"]),
            "{name}"
        );
        assert_eq!(
            SolanaVoucherScheme::voucher_message(
                &voucher.channel_account,
                voucher.max_claimable_amount
            )
            .to_vec(),
            message,
            "{name}"
        );
        assert_eq!(
            SolanaVoucherScheme::signer(&recorded),
            recorded.authorized_signer,
            "{name}"
        );
        assert_eq!(
            SolanaVoucherScheme::verify(&domain, &voucher, &recorded),
            Ok(()),
            "{name}"
        );
        // The scheme spells the voucher's own fields exactly as the vector
        // does: the envelope is the caller's.
        let spelled = SolanaVoucherScheme::spell(&voucher);
        for (field, value) in &spelled {
            assert_eq!(&json[field], value, "{name}: '{field}'");
        }
        let mut fields = spelled.keys().map(String::as_str).collect::<Vec<_>>();
        fields.sort_unstable();
        assert_eq!(
            fields,
            [
                "blockchain",
                "channelId",
                "expiresAt",
                "maxClaimableAmount",
                "signature"
            ],
            "{name}"
        );
    }
}

#[test]
fn every_mutated_solana_voucher_vector_is_refused() {
    let domain = SolanaVoucherDomain::payment_channels();
    for (name, _, wire, recorded, _) in solana_vouchers() {
        let genuine = SolanaVoucherScheme::parse(&wire).expect("parses");
        let mut flipped_signature = genuine;
        flipped_signature.signature[10] ^= 0x01;
        let mut other_channel = genuine;
        other_channel.channel_account[0] ^= 0x01;
        let mutations = [
            (
                "amount",
                ParsedSolanaVoucher {
                    max_claimable_amount: genuine.max_claimable_amount + 1,
                    ..genuine
                },
                VoucherRefusal::SignatureInvalid,
            ),
            (
                "signature",
                flipped_signature,
                VoucherRefusal::SignatureInvalid,
            ),
            (
                "channel account",
                other_channel,
                VoucherRefusal::ChannelIdMismatch,
            ),
        ];
        for (label, mutated, refusal) in mutations {
            assert_eq!(
                SolanaVoucherScheme::verify(&domain, &mutated, &recorded),
                Err(refusal),
                "{name}: changing the {label} must refuse the voucher"
            );
        }
        // Another authorized signer recorded for the same channel.
        let mut other_signer = recorded;
        other_signer.authorized_signer[0] ^= 0x01;
        assert_eq!(
            SolanaVoucherScheme::verify(&domain, &genuine, &other_signer),
            Err(VoucherRefusal::SignatureInvalid),
            "{name}"
        );
    }
}

#[test]
fn every_solana_claim_state_challenge_vector_verifies_as_committed() {
    let domain = SolanaVoucherDomain::payment_channels();
    let vectors = vectors();
    let cases = vectors["voucher_claim_state_challenge"]["solana"]
        .as_array()
        .expect("cases");
    assert!(!cases.is_empty());
    for case in cases {
        let name = case["name"].as_str().expect("name");
        let channel_account = base58::<32>(&case["channel_account_base58"]);
        let recorded = RecordedSolanaChannel {
            channel_account,
            authorized_signer: base58::<32>(&case["authorized_signer_base58"]),
        };
        let expires = case["expires"].as_u64().expect("expires");
        let signature = base64::engine::general_purpose::STANDARD
            .decode(case["signature_base64"].as_str().expect("signature"))
            .expect("base64");
        assert_eq!(
            SolanaVoucherScheme::challenge(&domain, &channel_account, expires),
            hex_bytes(&case["signed_message_hex"]),
            "{name}"
        );
        let verdict = SolanaVoucherScheme::verify_challenge(
            &domain,
            &channel_account,
            expires,
            &signature,
            &recorded,
        );
        if case["signature_verifies"].as_bool().expect("verdict") {
            assert_eq!(verdict, Ok(()), "{name}");
            // A challenge is never a voucher: the same signature over the
            // same channel does not verify as one for any amount it names.
            let as_voucher = ParsedSolanaVoucher {
                channel_account,
                max_claimable_amount: expires,
                signature: signature.clone().try_into().expect("64 bytes"),
            };
            assert_eq!(
                SolanaVoucherScheme::verify(&domain, &as_voucher, &recorded),
                Err(VoucherRefusal::SignatureInvalid),
                "{name}"
            );
            // Nor is it a challenge on another channel the chain records.
            let mut elsewhere = recorded;
            elsewhere.channel_account[0] ^= 0x01;
            assert_eq!(
                SolanaVoucherScheme::verify_challenge(
                    &domain,
                    &channel_account,
                    expires,
                    &signature,
                    &elsewhere
                ),
                Err(VoucherRefusal::ChannelIdMismatch),
                "{name}"
            );
        } else {
            assert_eq!(verdict, Err(VoucherRefusal::SignatureInvalid), "{name}");
        }
    }
}

#[test]
fn a_signature_or_account_of_the_wrong_width_or_alphabet_decodes_to_nothing() {
    let sixty_four = bs58::encode([0x07u8; 64]).into_string();
    assert_eq!(
        SolanaVoucherScheme::decode_signature(&sixty_four),
        Some([0x07; 64])
    );
    for text in [
        "",
        "0OIl",
        &bs58::encode([0x07u8; 63]).into_string(),
        &bs58::encode([0x07u8; 65]).into_string(),
    ] {
        assert_eq!(SolanaVoucherScheme::decode_signature(text), None, "{text}");
    }
    let thirty_two = bs58::encode([0x09u8; 32]).into_string();
    assert_eq!(
        SolanaVoucherScheme::decode_channel_account(&thirty_two),
        Some([0x09; 32])
    );
    assert_eq!(
        SolanaVoucherScheme::decode_channel_account(&sixty_four),
        None
    );
    assert_eq!(SolanaVoucherScheme::decode_channel_account("0OIl"), None);
}

#[test]
fn a_voucher_whose_signature_is_not_64_bytes_is_malformed_and_an_unreadable_account_names_no_channel(
) {
    let (_, json, _, _, _) = solana_vouchers().remove(0);
    let mut fields = json.as_object().expect("an object").clone();
    fields.insert(
        "signature".into(),
        bs58::encode([0x07u8; 63]).into_string().into(),
    );
    assert!(matches!(
        SolanaVoucherScheme::parse(&wire(&enveloped(fields.clone()))),
        Err(VoucherRefusal::Malformed(_))
    ));
    // Base58 the domain accepts, but 31 bytes: no account by that name.
    fields.insert(
        "signature".into(),
        bs58::encode([0x07u8; 64]).into_string().into(),
    );
    fields.insert(
        "channelId".into(),
        bs58::encode([0xffu8; 31]).into_string().into(),
    );
    assert_eq!(
        SolanaVoucherScheme::parse(&wire(&enveloped(fields))),
        Err(VoucherRefusal::UnreadableChannel)
    );
}

#[test]
fn the_payment_channels_domain_is_the_canonical_program() {
    assert_eq!(
        bs58::encode(SolanaVoucherDomain::payment_channels().program_id).into_string(),
        "CHNLxYvVA28MJP9PrFuDXccuoGXAx7jBacfLEkahyGsX"
    );
}

fn signer_from(seed: [u8; 32]) -> LocalEd25519Signer {
    LocalEd25519Signer::from_secret_bytes(seed).expect("any 32 bytes are an Ed25519 seed")
}

fn seeds_signed_by(key: &LocalEd25519Signer, salt: u64, open_slot: u64) -> SolanaChannelSeeds {
    SolanaChannelSeeds {
        payer: [0x11; 32],
        payee: [0x22; 32],
        mint: [0x33; 32],
        authorized_signer: key.public_key(),
        salt,
        open_slot,
    }
}

/// What the chain records for the channel `seeds` derive.
fn recorded_for(domain: &SolanaVoucherDomain, seeds: &SolanaChannelSeeds) -> RecordedSolanaChannel {
    RecordedSolanaChannel {
        channel_account: SolanaVoucherScheme::channel_id(domain, seeds),
        authorized_signer: seeds.authorized_signer,
    }
}

#[test]
fn an_amount_wider_than_u64_is_never_encoded() {
    let key = signer_from([0x21; 32]);
    let domain = SolanaVoucherDomain::payment_channels();
    assert!(SolanaVoucherScheme::encode(
        &domain,
        &seeds_signed_by(&key, 1, 1),
        u128::from(u64::MAX) + 1,
        &key
    )
    .is_err());
}

proptest! {
    /// Encoding a voucher, spelling it on the wire, parsing it as the
    /// domain does and then as the scheme does gives back the same voucher,
    /// and it verifies against the channel it was encoded for.
    #[test]
    fn encode_then_parse_round_trips(
        seed in proptest::array::uniform32(any::<u8>()),
        salt in any::<u64>(),
        open_slot in any::<u64>(),
        amount in any::<u64>(),
    ) {
        let key = signer_from(seed);
        let domain = SolanaVoucherDomain::payment_channels();
        let seeds = seeds_signed_by(&key, salt, open_slot);
        let voucher = SolanaVoucherScheme::encode(&domain, &seeds, u128::from(amount), &key)
            .expect("signs");
        prop_assert_eq!(voucher.channel_account, SolanaVoucherScheme::channel_id(&domain, &seeds));
        let parsed = SolanaVoucherScheme::parse(&wire(&enveloped(SolanaVoucherScheme::spell(&voucher))))
            .expect("parses");
        prop_assert_eq!(parsed, voucher);
        prop_assert_eq!(
            SolanaVoucherScheme::verify(&domain, &parsed, &recorded_for(&domain, &seeds)),
            Ok(())
        );
    }

    /// A voucher signed by any key but the channel's recorded
    /// `authorized_signer` is refused.
    #[test]
    fn a_voucher_signed_by_any_other_key_is_refused(
        owner in proptest::array::uniform32(any::<u8>()),
        other in proptest::array::uniform32(any::<u8>()),
        amount in any::<u64>(),
    ) {
        prop_assume!(owner != other);
        let (owner, other) = (signer_from(owner), signer_from(other));
        let domain = SolanaVoucherDomain::payment_channels();
        let seeds = seeds_signed_by(&owner, 7, 42);
        let forged = SolanaVoucherScheme::encode(&domain, &seeds, u128::from(amount), &other)
            .expect("signs");
        prop_assert_eq!(
            SolanaVoucherScheme::verify(&domain, &forged, &recorded_for(&domain, &seeds)),
            Err(VoucherRefusal::SignatureInvalid)
        );
    }

    /// A voucher on a channel other than the one the chain records is
    /// refused, however genuine its signature over its own channel.
    #[test]
    fn a_voucher_whose_channel_id_does_not_match_the_recorded_channel_is_refused(
        seed in proptest::array::uniform32(any::<u8>()),
        salt in any::<u64>(),
        other_salt in any::<u64>(),
        amount in any::<u64>(),
    ) {
        prop_assume!(salt != other_salt);
        let key = signer_from(seed);
        let domain = SolanaVoucherDomain::payment_channels();
        let signed_for = seeds_signed_by(&key, salt, 42);
        let recorded = recorded_for(&domain, &seeds_signed_by(&key, other_salt, 42));
        let voucher = SolanaVoucherScheme::encode(&domain, &signed_for, u128::from(amount), &key)
            .expect("signs");
        prop_assert_eq!(
            SolanaVoucherScheme::verify(&domain, &voucher, &recorded),
            Err(VoucherRefusal::ChannelIdMismatch)
        );
        prop_assert_eq!(
            SolanaVoucherScheme::check_channel_id(&domain, &voucher.channel_account, &recorded),
            Err(VoucherRefusal::ChannelIdMismatch)
        );
    }

    /// Arbitrary signature and key bytes never panic and never verify.
    #[test]
    fn arbitrary_signature_and_key_bytes_never_panic(
        signature in proptest::collection::vec(any::<u8>(), 64),
        key in proptest::array::uniform32(any::<u8>()),
    ) {
        let domain = SolanaVoucherDomain::payment_channels();
        let (_, _, wire, recorded, _) = solana_vouchers().remove(0);
        let mut voucher = SolanaVoucherScheme::parse(&wire).expect("parses");
        voucher.signature = signature.clone().try_into().expect("64 bytes");
        let recorded = RecordedSolanaChannel { authorized_signer: key, ..recorded };
        let _ = SolanaVoucherScheme::verify(&domain, &voucher, &recorded);
        let _ = SolanaVoucherScheme::verify_challenge(
            &domain, &voucher.channel_account, 1, &signature, &recorded,
        );
    }
}
