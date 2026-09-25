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

/// The committed contract, read as a payer reads it: a Solana claim's
/// `programId` names the settlement program its `channelAccount` lives under
/// (`docs/protocol/client-edge-spec.md` §1.3), which is the same 32 bytes
/// ADR 0053 puts at offset 16 of the signed balance proof.
///
/// This asserts on the **committed artifact**, not on the generator, because
/// the artifact is what `toon-client`, `rig` and `swap` replay (ADR 0021).
/// Until issue #1127 the fixture declared the system program while the
/// connector verified against the channel's own program, so the one
/// cross-repo statement of this field taught every payer reading it that any
/// base58 32-byte value would do -- and that is exactly why the connector
/// still only warns on a disagreement (§1.3) instead of refusing.
///
/// The system-program exclusion is spelled out rather than implied: it is the
/// specific wrong value this vector shipped, and re-introducing it would be
/// silent under an equality check alone.
#[test]
fn the_solana_claim_vector_declares_the_program_its_signature_is_bound_to() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vectors/wire-vectors.json");
    let committed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read the committed vectors"))
            .expect("the committed vectors are valid JSON");

    let case = &committed["peer_carriage"]["claim_solana"];
    let claim: serde_json::Value = serde_json::from_str(
        case["json"]
            .as_str()
            .expect("claim_solana carries its claim as a JSON string"),
    )
    .expect("the claim string is itself valid JSON");

    let declared = claim["programId"]
        .as_str()
        .expect("a Solana claim declares a programId");
    let declared_bytes = bs58::decode(declared)
        .into_vec()
        .expect("programId is base58");
    assert_eq!(
        declared_bytes.len(),
        32,
        "a programId is a 32-byte Solana address"
    );

    let signed_message = hex::decode(
        case["signed_message_hex"]
            .as_str()
            .expect("claim_solana carries the message its signature covers"),
    )
    .expect("signed_message_hex is hex");
    assert_eq!(
        signed_message.len(),
        96,
        "ADR 0053's balance proof is 96 bytes"
    );

    assert_eq!(
        &signed_message[16..48],
        declared_bytes.as_slice(),
        "the declared programId must be the program the signature is bound to -- a fixture that \
         declares one program and signs under another is not a contract anyone can conform to"
    );
    assert_ne!(
        declared, "11111111111111111111111111111111",
        "the system program is not a settlement program: no channel lives under it, so a claim \
         declaring it names nothing (issue #1127)"
    );
}

/// ADR 0074 decision 7 (issue #1347): the committed voucher vectors replay
/// against the real verification and parsing code, not merely against the
/// generator that produced them -- the same discipline
/// `the_solana_claim_vector_declares_the_program_its_signature_is_bound_to`
/// already applies to `claim_solana` above, extended to both voucher
/// shapes, the amount-only watermark rule and the nonzero-`expiresAt`
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
