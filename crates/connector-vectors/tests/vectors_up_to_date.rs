//! The gate issue #527's own acceptance criterion requires: a change to the
//! envelope, giftwrap or condition/fulfilment code that does not also
//! regenerate `vectors/wire-vectors.json` fails `cargo test --workspace`.
//!
//! Compared as parsed JSON, not raw bytes: this repo's pre-commit hook runs
//! `prettier --write` over staged `*.json` files, which reflows short
//! arrays onto one line and would make a byte-exact comparison flag a
//! difference that carries no data -- the invariant this gate protects is
//! that the *data* is unchanged, not this generator's own indentation
//! choices.

use std::path::PathBuf;

#[test]
fn committed_vectors_match_what_the_implementation_generates_today() {
    let regenerated: serde_json::Value =
        serde_json::from_str(&connector_vectors::to_json(&connector_vectors::generate()))
            .expect("generate() always produces valid JSON");

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vectors/wire-vectors.json");
    let committed_text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    let committed: serde_json::Value = serde_json::from_str(&committed_text)
        .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()));

    assert_eq!(
        regenerated, committed,
        "vectors/wire-vectors.json is stale -- run \
         `cargo run -p connector-vectors --bin generate-vectors` from the repo root and commit \
         the result"
    );
}

/// ADR 0074 decision 7 (issue #1347): the committed voucher vectors replay
/// against the real verification and parsing code, not merely against the
/// generator that produced them -- both voucher shapes, the amount-only watermark rule and the nonzero-`expiresAt`
/// refusal. Asserts on the **committed artifact** read fresh from disk,
/// because that artifact -- not the generator that produced it -- is what
/// `toon-client`, `rig` and `swap` replay (ADR 0021).
#[test]
fn the_committed_voucher_vectors_replay_against_the_real_implementation() {
    use connector_domain::client_claim::{parse_client_claim, ClientClaim, ClientClaimError};
    use connector_domain::{validate_voucher, ClaimError, VoucherAdmission, VoucherWatermark};
    use connector_signer::{
        evm_batch_channel_id, evm_voucher_digest, solana_voucher_message, verify_evm_voucher,
        verify_solana_voucher, BatchChannelConfig, BatchSettlementDomain,
    };

    fn hex_bytes<const N: usize>(s: &str) -> [u8; N] {
        hex::decode(s)
            .unwrap_or_else(|e| panic!("{s} is not hex: {e}"))
            .try_into()
            .unwrap_or_else(|v: Vec<u8>| panic!("{s} is not {N} bytes, got {}", v.len()))
    }

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vectors/wire-vectors.json");
    let committed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read the committed vectors"))
            .expect("the committed vectors are valid JSON");
    let voucher = &committed["claim_voucher"];

    // -- EVM: recompute the channel id and digest from the committed
    // `ChannelConfig`, and check the committed signature against them --
    // exactly what a connector does at admission (ADR 0074 decision 2), not
    // merely comparing two committed strings to each other.
    let evm = &voucher["evm"];
    let config_field = |field: &str| -> [u8; 20] {
        hex_bytes(evm["channel_config"][field].as_str().expect(field))
    };
    let config = BatchChannelConfig {
        payer: config_field("payer_hex"),
        payer_authorizer: config_field("payer_authorizer_hex"),
        receiver: config_field("receiver_hex"),
        receiver_authorizer: config_field("receiver_authorizer_hex"),
        token: config_field("token_hex"),
        withdraw_delay: evm["channel_config"]["withdraw_delay"]
            .as_u64()
            .expect("withdraw_delay"),
        salt: hex_bytes(
            evm["channel_config"]["salt_hex"]
                .as_str()
                .expect("salt_hex"),
        ),
    };
    let chain_id = evm["chain_id"].as_u64().expect("chain_id");
    let domain = BatchSettlementDomain::x402(chain_id);

    let channel_id = evm_batch_channel_id(&domain, &config);
    assert_eq!(
        hex::encode(channel_id),
        evm["channel_id_hex"].as_str().expect("channel_id_hex"),
        "the committed channelId must be what the committed channelConfig hashes to"
    );

    let amount = evm["max_claimable_amount"].as_u64().expect("amount");
    let digest = evm_voucher_digest(&domain, &channel_id, u128::from(amount));
    assert_eq!(
        hex::encode(digest),
        evm["digest_hex"].as_str().expect("digest_hex"),
        "the committed digest must be getVoucherDigest(channel_id, amount)"
    );

    let signature: [u8; 65] = hex_bytes(evm["signature_hex"].as_str().expect("signature_hex"));
    let signer: [u8; 20] = hex_bytes(
        evm["signer_address_hex"]
            .as_str()
            .expect("signer_address_hex"),
    );
    assert!(
        verify_evm_voucher(
            &domain,
            &channel_id,
            u128::from(amount),
            &signature,
            &signer
        ),
        "the committed signature must verify against the committed signer"
    );

    let evm_json = evm["json"].as_str().expect("evm carries its claim as JSON");
    assert!(matches!(
        parse_client_claim(evm_json).expect("the committed voucher parses"),
        ClientClaim::EvmVoucher(_)
    ));

    // -- Solana: recompute the 50-byte message and check the committed
    // signature against it.
    let solana = &voucher["solana"];
    let channel_account: [u8; 32] = hex_bytes(
        solana["channel_account_hex"]
            .as_str()
            .expect("channel_account_hex"),
    );
    let signer_public_key: [u8; 32] = hex_bytes(
        solana["signer_public_key_hex"]
            .as_str()
            .expect("signer_public_key_hex"),
    );
    let solana_amount = solana["max_claimable_amount"].as_u64().expect("amount");
    let expires_at = solana["expires_at"].as_i64().expect("expires_at");
    let solana_signature: [u8; 64] =
        hex_bytes(solana["signature_hex"].as_str().expect("signature_hex"));

    let message = solana_voucher_message(&channel_account, solana_amount, expires_at);
    assert_eq!(
        hex::encode(message),
        solana["signed_message_hex"]
            .as_str()
            .expect("signed_message_hex"),
        "the committed message must be solana_voucher_message(channel, amount, expires_at)"
    );
    assert!(
        verify_solana_voucher(
            &channel_account,
            solana_amount,
            expires_at,
            &solana_signature,
            &signer_public_key,
        ),
        "the committed signature must verify against the committed authorized_signer"
    );

    let solana_json = solana["json"]
        .as_str()
        .expect("solana carries its claim as JSON");
    assert!(matches!(
        parse_client_claim(solana_json).expect("the committed voucher parses"),
        ClientClaim::SolanaVoucher(_)
    ));

    // -- The amount-only watermark: replay every case through the real
    // `validate_voucher`, not just read back what the generator said it
    // would say.
    for case in voucher["amount_only_watermark"]
        .as_array()
        .expect("amount_only_watermark is an array")
    {
        let name = case["name"].as_str().expect("name");
        let watermark_amount = case["watermark_amount"].as_u64();
        let watermark_signature = case["watermark_signature_hex"]
            .as_str()
            .map(|s| hex::decode(s).expect("watermark_signature_hex is hex"));
        let watermark = watermark_amount.map(|amount| VoucherWatermark {
            cumulative_amount: amount,
            signature: watermark_signature
                .as_deref()
                .expect("a present watermark_amount carries a watermark_signature_hex"),
        });
        let presented_amount = case["presented_amount"].as_u64().expect("presented_amount");
        let presented_signature = hex::decode(
            case["presented_signature_hex"]
                .as_str()
                .expect("presented_signature_hex"),
        )
        .expect("presented_signature_hex is hex");
        let charge = case["charge"].as_u64().expect("charge");
        let outcome = case["outcome"].as_str().expect("outcome");

        let result = validate_voucher(watermark, presented_amount, &presented_signature, charge);
        match outcome {
            "advances" => {
                let advanced = case["advanced"]
                    .as_u64()
                    .expect("an advancing case carries `advanced`");
                assert_eq!(
                    result,
                    Ok(VoucherAdmission::Advances { advanced }),
                    "case {name}"
                );
            }
            "retransmission" => {
                assert_eq!(result, Ok(VoucherAdmission::Retransmission), "case {name}");
            }
            "amount_not_advancing" => {
                assert!(
                    matches!(result, Err(ClaimError::AmountNotAdvancing { .. })),
                    "case {name}: {result:?}"
                );
            }
            "underpayment" => {
                assert!(
                    matches!(result, Err(ClaimError::Underpayment { .. })),
                    "case {name}: {result:?}"
                );
            }
            other => panic!("case {name}: unknown outcome tag {other:?}"),
        }
    }

    // -- The nonzero-`expiresAt` refusal: replay through the real parser.
    for case in voucher["invalid"].as_array().expect("invalid is an array") {
        let name = case["name"].as_str().expect("name");
        let claim_json = case["claim_json"].as_str().expect("claim_json");
        let expected_error = case["expected_error"].as_str().expect("expected_error");
        let err =
            parse_client_claim(claim_json).expect_err(&format!("case {name} must be refused"));
        match expected_error {
            "voucher_expires" => assert!(
                matches!(err, ClientClaimError::VoucherExpires { .. }),
                "case {name}: {err:?}"
            ),
            other => panic!("case {name}: unknown expected_error tag {other:?}"),
        }
    }
}

fn committed() -> serde_json::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vectors/wire-vectors.json");
    serde_json::from_str(&std::fs::read_to_string(&path).expect("read the committed vectors"))
        .expect("the committed vectors are valid JSON")
}

fn hex_array<const N: usize>(value: &serde_json::Value) -> [u8; N] {
    let text = value.as_str().expect("a hex string");
    hex::decode(text.trim_start_matches("0x"))
        .unwrap_or_else(|e| panic!("{text} is not hex: {e}"))
        .try_into()
        .unwrap_or_else(|v: Vec<u8>| panic!("{text} is not {N} bytes, got {}", v.len()))
}

/// ADR 0075 decision 8 (issue #1384): every committed `toon-channel` claim
/// -- no `scheme`, or `scheme: "toon-channel"` -- is refused by name by the
/// client edge's parser and the peer carriage's, replayed from the
/// committed artifact.
#[test]
fn the_committed_toon_channel_claims_are_refused_by_name() {
    use connector_domain::client_claim::{parse_client_claim, ClientClaimError};
    use connector_peer_btp::{claim_json, ClaimDecodeError};

    let committed = committed();
    let cases = committed["toon_channel_refused"]["cases"]
        .as_array()
        .expect("cases is an array");
    assert!(!cases.is_empty());
    for case in cases {
        let json = case["claim_json"].as_str().expect("claim_json");
        assert_eq!(case["client_edge_error"], "toon_channel");
        assert_eq!(
            parse_client_claim(json),
            Err(ClientClaimError::ToonChannel),
            "{json}"
        );
        assert_eq!(
            claim_json::parse(json.as_bytes()),
            Err(ClaimDecodeError::ToonChannel)
        );
        assert_eq!(case["http_status"], 400);
        let refusal = case["refusal_text"].as_str().expect("refusal_text");
        assert!(refusal.contains("toon-channel") && refusal.contains("ADR 0075"));
        assert_eq!(
            hex::decode(case["http_response_body_hex"].as_str().expect("body")).expect("hex"),
            refusal.as_bytes()
        );
    }
}

/// ADR 0075 decisions 5 and 6: the committed peer vouchers and the
/// zero-value packet's challenge replay against the real verifiers and
/// parsers -- a challenge verifies as the channel's voucher signer's and
/// never as a voucher.
#[test]
fn the_committed_peer_vouchers_and_challenge_replay() {
    use connector_domain::client_claim::{parse_client_claim, ClientClaim};
    use connector_peer_btp::challenge_json;
    use connector_signer::{
        evm_voucher_claim_state_challenge_digest, evm_voucher_digest, solana_voucher_message,
        verify_evm_voucher, verify_evm_voucher_claim_state_challenge, verify_solana_voucher,
        BatchSettlementDomain,
    };

    let committed = committed();
    let peer = &committed["peer_carriage"];

    let evm = &peer["voucher_evm"];
    let domain = BatchSettlementDomain::x402(evm["chain_id"].as_u64().expect("chain_id"));
    let channel_id: [u8; 32] = hex_array(&evm["channel_id_hex"]);
    let amount = evm["max_claimable_amount"].as_u64().expect("amount");
    assert_eq!(
        hex::encode(evm_voucher_digest(&domain, &channel_id, u128::from(amount))),
        evm["digest_hex"].as_str().expect("digest_hex")
    );
    let signer: [u8; 20] = hex_array(&evm["signer_address_hex"]);
    assert_eq!(
        signer,
        hex_array::<20>(&evm["channel_config"]["payer_hex"]),
        "a peer voucher is signed by the payer's settlement key (payerAuthorizer == payer)"
    );
    assert!(verify_evm_voucher(
        &domain,
        &channel_id,
        u128::from(amount),
        &hex_array::<65>(&evm["signature_hex"]),
        &signer
    ));
    assert!(matches!(
        parse_client_claim(evm["json"].as_str().expect("json")),
        Ok(ClientClaim::EvmVoucher(_))
    ));

    let solana = &peer["voucher_solana"];
    let account: [u8; 32] = bs58::decode(
        solana["channel_account_base58"]
            .as_str()
            .expect("channel_account_base58"),
    )
    .into_vec()
    .expect("base58")
    .try_into()
    .expect("32 bytes");
    let solana_amount = solana["max_claimable_amount"].as_u64().expect("amount");
    let signature: [u8; 64] = bs58::decode(solana["signature_base58"].as_str().expect("sig"))
        .into_vec()
        .expect("base58")
        .try_into()
        .expect("64 bytes");
    let authorized: [u8; 32] = bs58::decode(
        solana["authorized_signer_base58"]
            .as_str()
            .expect("authorized_signer_base58"),
    )
    .into_vec()
    .expect("base58")
    .try_into()
    .expect("32 bytes");
    assert_eq!(
        hex::encode(solana_voucher_message(&account, solana_amount, 0)),
        solana["signed_message_hex"].as_str().expect("message")
    );
    assert!(verify_solana_voucher(
        &account,
        solana_amount,
        0,
        &signature,
        &authorized
    ));
    assert!(matches!(
        parse_client_claim(solana["json"].as_str().expect("json")),
        Ok(ClientClaim::SolanaVoucher(_))
    ));

    let zero = &peer["zero_value_challenge"];
    assert_eq!(zero["packet"]["prepare"]["amount"], 0);
    assert!(
        zero["packet"]["claim_json"].is_null(),
        "no voucher rides it"
    );
    let challenge = challenge_json::parse(
        zero["packet"]["challenge_json"]
            .as_str()
            .expect("challenge_json")
            .as_bytes(),
    )
    .expect("the challenge parses");
    let expires = zero["expires"].as_u64().expect("expires");
    assert_eq!(challenge.expires(), expires);
    let challenge_channel: [u8; 32] = hex_array(&zero["channel_id_hex"]);
    assert_eq!(challenge_channel, channel_id, "the voucher's channel");
    assert_eq!(
        hex::encode(evm_voucher_claim_state_challenge_digest(
            &domain,
            &challenge_channel,
            expires
        )),
        zero["digest_hex"].as_str().expect("digest_hex")
    );
    let challenge_signature: [u8; 65] = hex_array(&zero["signature_hex"]);
    assert!(verify_evm_voucher_claim_state_challenge(
        &domain,
        &challenge_channel,
        expires,
        &challenge_signature,
        &signer
    ));
    assert!(
        !verify_evm_voucher(
            &domain,
            &challenge_channel,
            u128::from(expires),
            &challenge_signature,
            &signer
        ),
        "a challenge is never a voucher"
    );
}

/// ADR 0075 decision 7: each committed payout voucher, with the claim
/// envelope restored, parses as the client edge's own voucher and its
/// signature verifies.
#[test]
fn the_committed_payout_vouchers_replay() {
    use connector_domain::client_claim::{parse_client_claim, ClientClaim};
    use connector_signer::{
        evm_voucher_digest, verify_evm_voucher, verify_solana_voucher, BatchSettlementDomain,
    };

    let committed = committed();
    let payout = &committed["payout_voucher"];
    let with_envelope = |json: &str| {
        let mut claim: serde_json::Value = serde_json::from_str(json).expect("JSON");
        claim["version"] = "1.0".into();
        claim["messageId"] = "replay".into();
        claim["timestamp"] = "2030-01-01T00:00:00.000Z".into();
        claim["senderId"] = "replay".into();
        parse_client_claim(&claim.to_string()).expect("the payout parses as a voucher")
    };

    let evm = &payout["evm"];
    let ClientClaim::EvmVoucher(voucher) = with_envelope(evm["json"].as_str().expect("json"))
    else {
        panic!("an EVM payout");
    };
    assert!(voucher.channel_config.is_some(), "landing needs the config");
    let domain = BatchSettlementDomain::x402(evm["chain_id"].as_u64().expect("chain_id"));
    let channel_id: [u8; 32] = hex_array(&evm["channel_id_hex"]);
    let amount = evm["max_claimable_amount"].as_u64().expect("amount");
    assert_eq!(voucher.max_claimable_amount, amount);
    assert_eq!(
        hex::encode(evm_voucher_digest(&domain, &channel_id, u128::from(amount))),
        evm["digest_hex"].as_str().expect("digest_hex")
    );
    assert!(verify_evm_voucher(
        &domain,
        &channel_id,
        u128::from(amount),
        &hex_array::<65>(&evm["signature_hex"]),
        &hex_array::<20>(&evm["signer_address_hex"])
    ));

    let solana = &payout["solana"];
    let ClientClaim::SolanaVoucher(voucher) = with_envelope(solana["json"].as_str().expect("json"))
    else {
        panic!("a Solana payout");
    };
    let decode = |field: &str| {
        bs58::decode(solana[field].as_str().expect(field))
            .into_vec()
            .expect("base58")
    };
    let account: [u8; 32] = decode("channel_account_base58").try_into().expect("32");
    let signature: [u8; 64] = decode("signature_base58").try_into().expect("64");
    let signer: [u8; 32] = decode("authorized_signer_base58").try_into().expect("32");
    assert!(verify_solana_voucher(
        &account,
        voucher.max_claimable_amount,
        0,
        &signature,
        &signer
    ));
}
