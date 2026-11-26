//! The XRPL scheme, through its interface: the committed wire vectors
//! (ADR 0021) and the scheme's properties.

use super::*;
use crate::{decode_xrpl_classic_address, xrpl_family_seed, XrplAlgorithm};
use connector_domain::client_claim::{parse_client_claim, ClientClaim};
use proptest::prelude::*;
use serde_json::Value;

const WIRE_VECTORS: &str = include_str!("../../../../../vectors/wire-vectors.json");

fn vectors() -> Value {
    serde_json::from_str(WIRE_VECTORS).expect("the committed vectors are JSON")
}

fn bytes<const N: usize>(value: &Value) -> [u8; N] {
    let text = value.as_str().expect("a hex string");
    assert_eq!(text.len(), 2 * N, "{text} is {N} bytes");
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).expect("hex");
    }
    out
}

fn byte_vec(value: &Value) -> Vec<u8> {
    let text = value.as_str().expect("a hex string");
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).expect("hex"))
        .collect()
}

fn wire(json: &str) -> XrplVoucher {
    match parse_client_claim(json).expect("the vector parses") {
        ClientClaim::XrplVoucher(voucher) => voucher,
        other => panic!("an XRPL voucher, not {other:?}"),
    }
}

fn account(value: &Value) -> [u8; 20] {
    decode_xrpl_classic_address(value.as_str().expect("an address")).expect("a classic address")
}

/// Each valid XRPL voucher vector: its name, its wire voucher, the domain it
/// is on, its channel's parts, and the key the ledger records for it.
fn xrpl_vouchers() -> Vec<(
    String,
    XrplVoucher,
    XrplVoucherDomain,
    XrplChannelParts,
    [u8; 33],
)> {
    let vectors = vectors();
    ["secp256k1", "ed25519"]
        .into_iter()
        .map(|name| {
            let vector = &vectors["claim_voucher"]["xrpl"][name];
            let voucher = wire(vector["json"].as_str().expect("json"));
            let domain = XrplVoucherScheme::domain_on(&voucher.network).expect("an xrpl network");
            assert_eq!(
                domain.network(),
                vector["network"].as_str().expect("network")
            );
            let parts = XrplChannelParts {
                source: account(&vector["payer_classic_address"]),
                destination: account(&vector["destination_classic_address"]),
                sequence: u32::try_from(vector["channel_sequence"].as_u64().expect("sequence"))
                    .expect("a u32"),
            };
            let key = bytes::<33>(&Value::from(
                vector["public_key_hex"]
                    .as_str()
                    .expect("key")
                    .to_ascii_lowercase(),
            ));
            (name.to_string(), voucher, domain, parts, key)
        })
        .collect()
}

#[test]
fn every_valid_xrpl_voucher_vector_parses_and_verifies() {
    let vectors = vectors();
    for (name, wire, domain, parts, key) in xrpl_vouchers() {
        let vector = &vectors["claim_voucher"]["xrpl"][name.as_str()];
        let voucher = XrplVoucherScheme::parse(&wire).expect("parses");
        assert_eq!(
            voucher.channel_id,
            bytes::<32>(&vector["channel_id_hex"]),
            "{name}"
        );
        assert_eq!(
            voucher.signature,
            byte_vec(&vector["signature_hex"]),
            "{name}"
        );
        assert_eq!(
            voucher.max_claimable_amount.to_string(),
            vector["max_claimable_amount"].as_str().expect("amount"),
            "{name}"
        );
        assert_eq!(
            XrplVoucherScheme::channel_id(&domain, &parts),
            voucher.channel_id,
            "{name}"
        );
        assert_eq!(XrplVoucherScheme::signer(&key), key, "{name}");
        assert_eq!(
            XrplVoucherScheme::check_channel_id(&domain, &voucher.channel_id, &key),
            Ok(()),
            "{name}"
        );
        assert_eq!(
            XrplVoucherScheme::verify(&domain, &voucher, &key),
            Ok(()),
            "{name}"
        );
    }
}

#[test]
fn every_mutated_xrpl_voucher_vector_is_refused() {
    let keys: Vec<[u8; 33]> = xrpl_vouchers().iter().map(|case| case.4).collect();
    for (name, wire, domain, _, key) in xrpl_vouchers() {
        let genuine = XrplVoucherScheme::parse(&wire).expect("parses");
        let mut flipped_signature = genuine.clone();
        flipped_signature.signature[10] ^= 0x01;
        let mut other_channel = genuine.clone();
        other_channel.channel_id[0] ^= 0x01;
        let mutations = [
            (
                "amount",
                ParsedXrplVoucher {
                    max_claimable_amount: genuine.max_claimable_amount + 1,
                    ..genuine.clone()
                },
            ),
            ("signature", flipped_signature),
            ("channel id", other_channel),
            (
                "amount, past the ledger's ceiling",
                ParsedXrplVoucher {
                    max_claimable_amount: u128::from(crate::XRPL_MAX_DROPS) + 1,
                    ..genuine.clone()
                },
            ),
        ];
        for (label, mutated) in mutations {
            assert_eq!(
                XrplVoucherScheme::verify(&domain, &mutated, &key),
                Err(VoucherRefusal::SignatureInvalid),
                "{name}: changing the {label} must refuse the voucher"
            );
        }
        // The ledger records another key for the channel.
        for other in keys.iter().filter(|other| **other != key) {
            assert_eq!(
                XrplVoucherScheme::verify(&domain, &genuine, other),
                Err(VoucherRefusal::SignatureInvalid),
                "{name}"
            );
        }
    }
}

/// ADR 0087 decision 2: the ledger's claim binds no network, so the same
/// voucher verifies under another network's domain.
#[test]
fn an_xrpl_voucher_binds_no_network() {
    let vectors = vectors();
    let vector = &vectors["claim_voucher"]["xrpl"]["claim_binds_no_network"];
    let wire = wire(vector["claim_json"].as_str().expect("json"));
    let domain = XrplVoucherScheme::domain_on(&wire.network).expect("an xrpl network");
    assert_eq!(domain.network(), "xrpl:0");
    let key = xrpl_vouchers()[0].4;
    let voucher = XrplVoucherScheme::parse(&wire).expect("parses");
    assert_eq!(
        XrplVoucherScheme::verify(&domain, &voucher, &key).is_ok(),
        vector["signature_verifies"].as_bool().expect("verdict")
    );
}

#[test]
fn every_xrpl_channel_id_vector_derives_from_its_parts() {
    let vectors = vectors();
    let domain = XrplVoucherDomain::for_network_id(1);
    for case in vectors["claim_voucher"]["xrpl"]["channel_id"]
        .as_array()
        .expect("cases")
    {
        let parts = XrplChannelParts {
            source: bytes::<20>(&case["source_account_id_hex"]),
            destination: bytes::<20>(&case["destination_account_id_hex"]),
            sequence: u32::try_from(case["sequence"].as_u64().expect("sequence")).expect("u32"),
        };
        assert_eq!(
            XrplVoucherScheme::channel_id(&domain, &parts),
            bytes::<32>(&case["channel_id_hex"]),
            "{}",
            case["name"]
        );
    }
}

#[test]
fn every_xrpl_claim_state_challenge_vector_verifies_as_committed() {
    let vectors = vectors();
    let cases = vectors["voucher_claim_state_challenge"]["xrpl"]
        .as_array()
        .expect("cases");
    assert!(!cases.is_empty());
    for case in cases {
        let name = case["name"].as_str().expect("name");
        let network: Network = case["network"]
            .as_str()
            .expect("network")
            .parse()
            .expect("a network");
        let domain = XrplVoucherScheme::domain_on(&network).expect("an xrpl network");
        let key = bytes::<33>(&Value::from(
            case["channel_public_key_hex"]
                .as_str()
                .expect("key")
                .to_ascii_lowercase(),
        ));
        let channel_id = bytes::<32>(&case["channel_id_hex"]);
        let expires = case["expires"].as_u64().expect("expires");
        let signature = byte_vec(&case["signature_hex"]);
        if case["signed_for_network"] == case["network"] {
            assert_eq!(
                XrplVoucherScheme::challenge(&domain, &channel_id, expires),
                byte_vec(&case["signed_message_hex"]),
                "{name}"
            );
        }
        let verdict =
            XrplVoucherScheme::verify_challenge(&domain, &channel_id, expires, &signature, &key);
        if case["signature_verifies"].as_bool().expect("verdict") {
            assert_eq!(verdict, Ok(()), "{name}");
        } else {
            assert_eq!(verdict, Err(VoucherRefusal::SignatureInvalid), "{name}");
        }
        // A challenge is never a voucher, for any amount it could name.
        assert!(!case["verifies_as_claim"].as_bool().expect("verdict"));
        let as_voucher = ParsedXrplVoucher {
            channel_id,
            max_claimable_amount: u128::from(expires),
            signature: signature.clone(),
        };
        assert_eq!(
            XrplVoucherScheme::verify(&domain, &as_voucher, &key),
            Err(VoucherRefusal::SignatureInvalid),
            "{name}"
        );
    }
}

/// The payout vector's own fields are exactly what the scheme spells.
#[test]
fn the_xrpl_payout_vector_is_what_the_scheme_spells() {
    let vectors = vectors();
    let vector = &vectors["payout_voucher"]["xrpl"];
    let voucher = ParsedXrplVoucher {
        channel_id: bytes::<32>(&vector["channel_id_hex"]),
        max_claimable_amount: vector["max_claimable_amount"]
            .as_str()
            .expect("amount")
            .parse()
            .expect("decimal"),
        signature: byte_vec(&vector["signature_hex"]),
    };
    let mut expected: serde_json::Map<String, Value> =
        serde_json::from_str(vector["json"].as_str().expect("json")).expect("json");
    expected.remove("network");
    expected.remove("scheme");
    assert_eq!(XrplVoucherScheme::spell(&voucher), expected);
    let key = bytes::<33>(&Value::from(
        vector["public_key_hex"]
            .as_str()
            .expect("key")
            .to_ascii_lowercase(),
    ));
    assert_eq!(
        XrplVoucherScheme::verify(&XrplVoucherDomain::for_network_id(1), &voucher, &key),
        Ok(())
    );
}

#[test]
fn a_signature_of_the_wrong_width_or_alphabet_decodes_to_nothing() {
    for text in [
        String::new(),
        "ab".repeat(63),
        "ab".repeat(73),
        "zz".repeat(64),
        format!("0x{}", "ab".repeat(64)),
        "a".repeat(129),
    ] {
        assert_eq!(XrplVoucherScheme::decode_signature(&text), None, "{text}");
    }
    assert_eq!(
        XrplVoucherScheme::decode_signature(&"AB".repeat(64)),
        Some(vec![0xab; 64])
    );
    assert_eq!(
        XrplVoucherScheme::decode_signature(&"ab".repeat(72)),
        Some(vec![0xab; 72])
    );
}

#[test]
fn a_channel_id_decodes_with_or_without_its_prefix_and_only_at_its_width() {
    let text = "5D".repeat(32);
    assert_eq!(
        XrplVoucherScheme::decode_channel_id(&text),
        Some([0x5d; 32])
    );
    assert_eq!(
        XrplVoucherScheme::decode_channel_id(&format!("0x{text}")),
        Some([0x5d; 32])
    );
    assert_eq!(XrplVoucherScheme::decode_channel_id(&"5d".repeat(31)), None);
    assert_eq!(XrplVoucherScheme::decode_channel_id(&"zz".repeat(32)), None);
}

#[test]
fn a_domain_is_an_xrpl_network_only() {
    for network in ["xrpl:0", "xrpl:1", "xrpl:2", "xrpl:1000"] {
        let parsed: Network = network.parse().expect("a network");
        assert_eq!(
            XrplVoucherScheme::domain_on(&parsed).map(|domain| domain.network().to_string()),
            Some(network.to_string())
        );
    }
    let evm: Network = "eip155:8453".parse().expect("a network");
    assert_eq!(XrplVoucherScheme::domain_on(&evm), None);
    assert_eq!(
        XrplVoucherDomain::for_network_id(21338).network(),
        "xrpl:21338"
    );
}

/// The envelope a voucher rides in, around the scheme's own fields.
fn envelope(fields: serde_json::Map<String, Value>, network: &str) -> String {
    let mut json = fields;
    json.insert("version".into(), "1.0".into());
    json.insert("network".into(), network.into());
    json.insert("scheme".into(), "batch-settlement".into());
    json.insert("messageId".into(), "m".into());
    json.insert("timestamp".into(), "2030-01-01T00:00:00.000Z".into());
    json.insert("senderId".into(), "s".into());
    Value::Object(json).to_string()
}

fn key_from(ed25519: bool, entropy: [u8; 16]) -> XrplSigner {
    let algorithm = if ed25519 {
        XrplAlgorithm::Ed25519
    } else {
        XrplAlgorithm::Secp256k1
    };
    XrplSigner::from_family_seed(&xrpl_family_seed(algorithm, &entropy)).expect("a seed")
}

fn parts() -> impl Strategy<Value = XrplChannelParts> {
    (any::<[u8; 20]>(), any::<[u8; 20]>(), any::<u32>()).prop_map(
        |(source, destination, sequence)| XrplChannelParts {
            source,
            destination,
            sequence,
        },
    )
}

fn network() -> impl Strategy<Value = &'static str> {
    prop_oneof![
        Just("xrpl:0"),
        Just("xrpl:1"),
        Just("xrpl:2"),
        Just("xrpl:1000")
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// Encode, spell, parse in the domain, parse in the scheme: the same
    /// voucher, and it verifies against the key that signed it.
    #[test]
    fn encode_then_parse_round_trips(
        ed25519 in any::<bool>(),
        entropy in any::<[u8; 16]>(),
        parts in parts(),
        amount in 0..=crate::XRPL_MAX_DROPS,
        network in network(),
    ) {
        let key = key_from(ed25519, entropy);
        let parsed: Network = network.parse().expect("a network");
        let domain = XrplVoucherScheme::domain_on(&parsed).expect("an xrpl network");
        let encoded = XrplVoucherScheme::encode(&domain, &parts, u128::from(amount), &key)
            .expect("encodes");
        prop_assert_eq!(encoded.channel_id, XrplVoucherScheme::channel_id(&domain, &parts));
        let json = envelope(XrplVoucherScheme::spell(&encoded), network);
        let wire = wire(&json);
        let reparsed = XrplVoucherScheme::parse(&wire).expect("parses");
        prop_assert_eq!(&reparsed, &encoded);
        prop_assert_eq!(XrplVoucherScheme::verify(&domain, &reparsed, &key.public_key()), Ok(()));
    }

    /// A voucher signed by any key but the one the ledger records is refused.
    #[test]
    fn a_wrong_signer_is_refused(
        ed25519 in any::<bool>(),
        other_ed25519 in any::<bool>(),
        entropy in any::<[u8; 16]>(),
        other_entropy in any::<[u8; 16]>(),
        parts in parts(),
        amount in 0..=crate::XRPL_MAX_DROPS,
    ) {
        prop_assume!((ed25519, entropy) != (other_ed25519, other_entropy));
        let domain = XrplVoucherDomain::for_network_id(1);
        let key = key_from(ed25519, entropy);
        let recorded = key_from(other_ed25519, other_entropy).public_key();
        let voucher = XrplVoucherScheme::encode(&domain, &parts, u128::from(amount), &key)
            .expect("encodes");
        prop_assert_eq!(
            XrplVoucherScheme::verify(&domain, &voucher, &recorded),
            Err(VoucherRefusal::SignatureInvalid)
        );
    }

    /// A voucher naming a channel id its parts do not derive is refused:
    /// the ledger's claim message signs the id.
    #[test]
    fn a_mismatched_channel_id_is_refused(
        ed25519 in any::<bool>(),
        entropy in any::<[u8; 16]>(),
        parts in parts(),
        other in parts(),
        amount in 0..=crate::XRPL_MAX_DROPS,
    ) {
        let domain = XrplVoucherDomain::for_network_id(1);
        let expected = XrplVoucherScheme::channel_id(&domain, &parts);
        prop_assume!(XrplVoucherScheme::channel_id(&domain, &other) != expected);
        let key = key_from(ed25519, entropy);
        let voucher = XrplVoucherScheme::encode(&domain, &parts, u128::from(amount), &key)
            .expect("encodes");
        let mismatched = ParsedXrplVoucher {
            channel_id: XrplVoucherScheme::channel_id(&domain, &other),
            ..voucher
        };
        prop_assert_eq!(
            XrplVoucherScheme::verify(&domain, &mismatched, &key.public_key()),
            Err(VoucherRefusal::SignatureInvalid)
        );
    }

    /// A challenge signed through the scheme verifies on its own network,
    /// channel and expiry, and on no other; and it is never a voucher.
    #[test]
    fn a_challenge_binds_its_network_channel_and_expiry(
        ed25519 in any::<bool>(),
        entropy in any::<[u8; 16]>(),
        channel_id in any::<[u8; 32]>(),
        expires in any::<u64>(),
        network_id in any::<u32>(),
        flip in any::<usize>(),
    ) {
        let key = key_from(ed25519, entropy);
        let recorded = key.public_key();
        let domain = XrplVoucherDomain::for_network_id(network_id);
        let signature = XrplVoucherScheme::sign_challenge(&domain, &channel_id, expires, &key);
        prop_assert_eq!(
            XrplVoucherScheme::verify_challenge(&domain, &channel_id, expires, &signature, &recorded),
            Ok(())
        );
        let other_network = XrplVoucherDomain::for_network_id(network_id.wrapping_add(1));
        prop_assert!(XrplVoucherScheme::verify_challenge(
            &other_network, &channel_id, expires, &signature, &recorded
        ).is_err());
        let mut other_channel = channel_id;
        other_channel[flip % 32] ^= 1;
        prop_assert!(XrplVoucherScheme::verify_challenge(
            &domain, &other_channel, expires, &signature, &recorded
        ).is_err());
        prop_assert!(XrplVoucherScheme::verify_challenge(
            &domain, &channel_id, expires ^ (1 << (flip % 64)), &signature, &recorded
        ).is_err());
        let as_voucher = ParsedXrplVoucher {
            channel_id,
            max_claimable_amount: u128::from(expires),
            signature,
        };
        prop_assert!(XrplVoucherScheme::verify(&domain, &as_voucher, &recorded).is_err());
    }

    /// Attacker-chosen signature bytes are refused, never a panic.
    #[test]
    fn arbitrary_signature_bytes_never_panic(
        signature in proptest::collection::vec(any::<u8>(), 0..80),
        channel_id in any::<[u8; 32]>(),
        amount in any::<u128>(),
        key in any::<[u8; 32]>(),
        prefix in prop_oneof![Just(0x02u8), Just(0x03), Just(0xED), any::<u8>()],
    ) {
        let mut recorded = [prefix; 33];
        recorded[1..].copy_from_slice(&key);
        let voucher = ParsedXrplVoucher { channel_id, max_claimable_amount: amount, signature };
        let _ = XrplVoucherScheme::verify(&XrplVoucherDomain::for_network_id(1), &voucher, &recorded);
    }
}
