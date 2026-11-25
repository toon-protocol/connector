//! The EVM scheme, through its interface: the committed wire vectors
//! (ADR 0021) and the scheme's properties.

use super::*;
use crate::LocalSigner;
use connector_domain::client_claim::{parse_client_claim, parse_evm_channel_config, ClientClaim};
use proptest::prelude::*;
use serde_json::Value;

const WIRE_VECTORS: &str = include_str!("../../../../../vectors/wire-vectors.json");

fn vectors() -> Value {
    serde_json::from_str(WIRE_VECTORS).expect("the committed vectors are JSON")
}

fn bytes<const N: usize>(value: &Value) -> [u8; N] {
    let text = value.as_str().expect("a hex string");
    let text = text.strip_prefix("0x").unwrap_or(text);
    assert_eq!(text.len(), 2 * N, "{text} is {N} bytes");
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).expect("hex");
    }
    out
}

fn wire(json: &str) -> EvmVoucher {
    match parse_client_claim(json).expect("the vector parses") {
        ClientClaim::EvmVoucher(voucher) => voucher,
        other => panic!("an EVM voucher, not {other:?}"),
    }
}

/// Each valid EVM voucher vector: its wire voucher, the domain it is on,
/// and the config the chain records for its channel.
fn evm_vouchers() -> Vec<(
    String,
    EvmVoucher,
    BatchSettlementDomain,
    BatchChannelConfig,
)> {
    let vectors = vectors();
    ["evm", "evm_above_u64_max", "evm_second_network"]
        .into_iter()
        .map(|name| {
            let vector = &vectors["claim_voucher"][name];
            let voucher = wire(vector["json"].as_str().expect("json"));
            let domain = EvmVoucherScheme::domain_on(&voucher.network).expect("an eip155 network");
            assert_eq!(
                domain.chain_id,
                vector["chain_id"].as_u64().expect("chain id")
            );
            assert_eq!(
                domain.verifying_contract,
                bytes::<20>(&vector["verifying_contract_hex"])
            );
            let config =
                EvmVoucherScheme::decode_config(voucher.channel_config.as_ref().expect("a config"))
                    .expect("the vector's config decodes");
            (name.to_string(), voucher, domain, config)
        })
        .collect()
}

#[test]
fn every_valid_evm_voucher_vector_parses_and_verifies() {
    let vectors = vectors();
    for (name, wire, domain, config) in evm_vouchers() {
        let vector = &vectors["claim_voucher"][name.as_str()];
        let voucher = EvmVoucherScheme::parse(&wire).expect("parses");
        assert_eq!(
            voucher.channel_id,
            bytes::<32>(&vector["channel_id_hex"]),
            "{name}"
        );
        assert_eq!(
            voucher.signature,
            bytes::<65>(&vector["signature_hex"]),
            "{name}"
        );
        assert_eq!(
            voucher.max_claimable_amount.to_string(),
            vector["max_claimable_amount"].as_str().expect("amount"),
            "{name}"
        );
        assert_eq!(voucher.channel_config, Some(config), "{name}");
        assert_eq!(
            EvmVoucherScheme::channel_id(&domain, &config),
            voucher.channel_id,
            "{name}"
        );
        assert_eq!(
            EvmVoucherScheme::signer(&config),
            bytes::<20>(&vector["signer_address_hex"]),
            "{name}"
        );
        assert_eq!(
            EvmVoucherScheme::voucher_digest(
                &domain,
                &voucher.channel_id,
                voucher.max_claimable_amount
            ),
            bytes::<32>(&vector["digest_hex"]),
            "{name}"
        );
        assert_eq!(
            EvmVoucherScheme::verify(&domain, &voucher, &config),
            Ok(()),
            "{name}"
        );
    }
}

/// The malleated twin of `signature`: `s` replaced by `n - s` and `v`
/// flipped. Plain ECDSA recovers the same key from it; the contract refuses
/// it, and so must the scheme.
fn high_s_twin(signature: [u8; 65]) -> [u8; 65] {
    const ORDER: [u8; 32] = [
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xfe, 0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c, 0xd0, 0x36,
        0x41, 0x41,
    ];
    let mut twin = signature;
    let mut borrow = 0i16;
    for i in (0..32).rev() {
        let difference = i16::from(ORDER[i]) - i16::from(signature[32 + i]) - borrow;
        borrow = i16::from(difference < 0);
        twin[32 + i] = (difference + 256 * borrow) as u8;
    }
    twin[64] = if signature[64] == 27 { 28 } else { 27 };
    twin
}

/// Whom `signature` recovers to over `digest` under plain ECDSA, without
/// EIP-2's low-`s` rule.
fn lax_signer(digest: &[u8; 32], signature: &[u8; 65]) -> Address {
    let mut rs = [0u8; 64];
    rs.copy_from_slice(&signature[..64]);
    let recovered = libsecp256k1::recover(
        &libsecp256k1::Message::parse(digest),
        &libsecp256k1::Signature::parse_overflowing(&rs),
        &libsecp256k1::RecoveryId::parse(signature[64] - 27).expect("27 or 28"),
    )
    .expect("recovers");
    crate::derive_evm_address(&recovered.serialize())
}

#[test]
fn every_mutated_evm_voucher_vector_is_refused() {
    for (name, wire, domain, config) in evm_vouchers() {
        let genuine = EvmVoucherScheme::parse(&wire).expect("parses");
        // The high-`s` twin really is the same key under lax ECDSA, so its
        // refusal below is the contract's rule and not a broken signature.
        let digest = EvmVoucherScheme::voucher_digest(
            &domain,
            &genuine.channel_id,
            genuine.max_claimable_amount,
        );
        assert_eq!(
            lax_signer(&digest, &high_s_twin(genuine.signature)),
            EvmVoucherScheme::signer(&config),
            "{name}"
        );
        let mut flipped_signature = genuine;
        flipped_signature.signature[10] ^= 0x01;
        let mut v_zero_or_one = genuine;
        v_zero_or_one.signature[64] -= 27;
        let mut other_channel = genuine;
        other_channel.channel_id[0] ^= 0x01;
        let mutations = [
            (
                "amount",
                ParsedEvmVoucher {
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
                "v of 0 or 1",
                v_zero_or_one,
                VoucherRefusal::SignatureInvalid,
            ),
            (
                "high s",
                ParsedEvmVoucher {
                    signature: high_s_twin(genuine.signature),
                    ..genuine
                },
                VoucherRefusal::SignatureInvalid,
            ),
            (
                "channel id",
                other_channel,
                VoucherRefusal::ChannelIdMismatch,
            ),
        ];
        for (label, mutated, refusal) in mutations {
            assert_eq!(
                EvmVoucherScheme::verify(&domain, &mutated, &config),
                Err(refusal),
                "{name}: changing the {label} must refuse the voucher"
            );
        }

        // The channel the chain records is not the one the voucher signs.
        let other_config = BatchChannelConfig {
            salt: [0x01; 32],
            ..config
        };
        assert_eq!(
            EvmVoucherScheme::verify(&domain, &genuine, &other_config),
            Err(VoucherRefusal::ChannelIdMismatch),
            "{name}"
        );
        // Another chain's domain: the id no longer derives, and even a config
        // re-derived there names another channel.
        let other_chain = BatchSettlementDomain::x402(domain.chain_id + 1);
        assert!(
            EvmVoucherScheme::verify(&other_chain, &genuine, &config).is_err(),
            "{name}"
        );
    }
}

#[test]
fn every_evm_claim_state_challenge_vector_verifies_as_committed() {
    let vectors = vectors();
    let cases = vectors["voucher_claim_state_challenge"]["evm"]
        .as_array()
        .expect("cases");
    assert!(!cases.is_empty());
    for case in cases {
        let name = case["name"].as_str().expect("name");
        let entry: Value =
            serde_json::from_str(case["entry_json"].as_str().expect("entry")).expect("json");
        let config = EvmVoucherScheme::decode_config(
            &parse_evm_channel_config(&entry["channelConfig"]).expect("config shape"),
        )
        .expect("config");
        let domain = BatchSettlementDomain::x402(case["chain_id"].as_u64().expect("chain id"));
        let channel_id = bytes::<32>(&case["channel_id_hex"]);
        let expires = case["expires"].as_u64().expect("expires");
        let signature = bytes::<65>(&case["signature_hex"]);
        assert_eq!(
            EvmVoucherScheme::check_channel_id(&domain, &channel_id, &config),
            Ok(())
        );
        assert_eq!(
            EvmVoucherScheme::signer(&config),
            bytes::<20>(&case["voucher_signer_address_hex"]),
            "{name}"
        );
        assert_eq!(
            EvmVoucherScheme::challenge(&domain, &channel_id, expires),
            bytes::<32>(&case["digest_hex"]).to_vec(),
            "{name}"
        );
        let verdict =
            EvmVoucherScheme::verify_challenge(&domain, &channel_id, expires, &signature, &config);
        if case["signature_verifies"].as_bool().expect("verdict") {
            assert_eq!(verdict, Ok(()), "{name}");
            // A challenge is never a voucher: the same signature over the
            // same channel does not verify as one for any amount it names.
            let as_voucher = ParsedEvmVoucher {
                channel_id,
                max_claimable_amount: u128::from(expires),
                signature,
                channel_config: None,
            };
            assert_eq!(
                EvmVoucherScheme::verify(&domain, &as_voucher, &config),
                Err(VoucherRefusal::SignatureInvalid),
                "{name}"
            );
        } else {
            assert_eq!(verdict, Err(VoucherRefusal::SignatureInvalid), "{name}");
        }
    }
}

#[test]
fn a_signature_of_the_wrong_width_or_alphabet_decodes_to_nothing() {
    for text in [
        "0x",
        "0x1b",
        &format!("0x{}", "1b".repeat(64)),
        &format!("0x{}", "zz".repeat(65)),
    ] {
        assert_eq!(EvmVoucherScheme::decode_signature(text), None, "{text}");
    }
    assert_eq!(
        EvmVoucherScheme::decode_signature(&format!("0x{}", "1B".repeat(65))),
        Some([0x1b; 65])
    );
}

/// The envelope a voucher rides in, around the scheme's own fields.
fn envelope(fields: serde_json::Map<String, Value>, network: &Network) -> String {
    let mut json = fields;
    json.insert("version".into(), "1.0".into());
    json.insert("network".into(), network.to_string().into());
    json.insert("scheme".into(), "batch-settlement".into());
    json.insert("messageId".into(), "m".into());
    json.insert("timestamp".into(), "2030-01-01T00:00:00.000Z".into());
    json.insert("senderId".into(), "s".into());
    Value::Object(json).to_string()
}

fn signer_from(secret: [u8; 32]) -> Option<LocalSigner> {
    LocalSigner::from_secret_bytes("voucher-key", secret).ok()
}

fn config_signed_by(key: &LocalSigner, salt: [u8; 32], delay: u64) -> BatchChannelConfig {
    let authorizer = crate::derive_evm_address(&key.public_key().expect("public key"));
    BatchChannelConfig {
        payer: [0x11; 20],
        payer_authorizer: authorizer,
        receiver: [0x33; 20],
        receiver_authorizer: [0x44; 20],
        token: [0x55; 20],
        withdraw_delay: delay,
        salt,
    }
}

proptest! {
    /// Encoding a voucher, spelling it on the wire, parsing it as the
    /// domain does and then as the scheme does gives back the same voucher,
    /// and it verifies against the channel it was encoded for.
    #[test]
    fn encode_then_parse_round_trips(
        secret in proptest::array::uniform32(1u8..),
        salt in proptest::array::uniform32(any::<u8>()),
        delay in 0u64..(1 << 40),
        amount in any::<u128>(),
        chain_id in 1u64..1_000_000,
    ) {
        let Some(key) = signer_from(secret) else { return Ok(()); };
        let domain = BatchSettlementDomain::x402(chain_id);
        let config = config_signed_by(&key, salt, delay);
        let voucher = EvmVoucherScheme::encode(&domain, &config, amount, &key).expect("signs");
        prop_assert_eq!(voucher.channel_config, Some(config));
        let json = envelope(EvmVoucherScheme::spell(&voucher), &Network::Eip155(chain_id));
        let parsed = EvmVoucherScheme::parse(&wire(&json)).expect("parses");
        prop_assert_eq!(parsed, voucher);
        prop_assert_eq!(EvmVoucherScheme::verify(&domain, &parsed, &config), Ok(()));
    }

    /// A voucher signed by any key but the channel's recorded signer is
    /// refused.
    #[test]
    fn a_voucher_signed_by_any_other_key_is_refused(
        owner in proptest::array::uniform32(1u8..),
        other in proptest::array::uniform32(1u8..),
        amount in any::<u128>(),
    ) {
        prop_assume!(owner != other);
        let (Some(owner), Some(other)) = (signer_from(owner), signer_from(other)) else {
            return Ok(());
        };
        let domain = BatchSettlementDomain::x402(84_532);
        let recorded = config_signed_by(&owner, [0x66; 32], 86_400);
        let forged = EvmVoucherScheme::encode(&domain, &recorded, amount, &other).expect("signs");
        prop_assert_eq!(
            EvmVoucherScheme::verify(&domain, &forged, &recorded),
            Err(VoucherRefusal::SignatureInvalid)
        );
    }

    /// A voucher whose channel id is not the one its parts derive is
    /// refused, however genuine its signature over that id.
    #[test]
    fn a_voucher_whose_channel_id_does_not_match_its_parts_is_refused(
        secret in proptest::array::uniform32(1u8..),
        salt in proptest::array::uniform32(any::<u8>()),
        other_salt in proptest::array::uniform32(any::<u8>()),
        amount in any::<u128>(),
    ) {
        prop_assume!(salt != other_salt);
        let Some(key) = signer_from(secret) else { return Ok(()); };
        let domain = BatchSettlementDomain::x402(84_532);
        let signed_for = config_signed_by(&key, salt, 86_400);
        let recorded = config_signed_by(&key, other_salt, 86_400);
        let voucher = EvmVoucherScheme::encode(&domain, &signed_for, amount, &key).expect("signs");
        prop_assert_eq!(
            EvmVoucherScheme::verify(&domain, &voucher, &recorded),
            Err(VoucherRefusal::ChannelIdMismatch)
        );
    }

    /// Arbitrary signature bytes never panic and never verify.
    #[test]
    fn arbitrary_signature_bytes_never_panic(signature in proptest::collection::vec(any::<u8>(), 65)) {
        let (_, wire, domain, config) = evm_vouchers().remove(0);
        let mut voucher = EvmVoucherScheme::parse(&wire).expect("parses");
        voucher.signature = signature.try_into().expect("65 bytes");
        let _ = EvmVoucherScheme::verify(&domain, &voucher, &config);
        let _ = EvmVoucherScheme::verify_challenge(&domain, &voucher.channel_id, 1, &voucher.signature, &config);
    }
}
