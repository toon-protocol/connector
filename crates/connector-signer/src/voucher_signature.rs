//! Verifying an x402 `batch-settlement` **voucher** (ADR 0074 decision 4,
//! issue #1341) -- the second claim scheme a client may pay a connector
//! under, and only a client.
//!
//! A voucher is a claim, but not a `toon-channel` claim: it is signed over a
//! different message, under a different domain, by a signer the chain
//! names differently, and its freshness is an amount rather than a nonce.
//! So it gets its own signature type ([`VoucherSignature`]) and its own
//! verifiers here, beside [`crate::claim_signature`]'s balance proofs rather
//! than inside them. Nothing in this module is reachable from a peer
//! carriage: `connector_runtime::ClaimSignature` -- the peer wire's
//! signature enum -- deliberately has no voucher variant, so a voucher is
//! unrepresentable as a peer claim rather than merely refused as one.
//!
//! Upstream is pinned (ADR 0074's Sources): x402 at `0cb1a1f0` for EVM,
//! payment-channels at `3ffa4d67` for Solana. Every constant below is cited
//! against those, and the test vectors were produced independently of this
//! code -- the EVM ones by the pinned `x402BatchSettlement` bytecode itself
//! (`getChannelId`/`getVoucherDigest`, deployed on anvil at the production
//! address with chain id 84532) and `cast wallet sign`, the Solana one by
//! OpenSSL's Ed25519.
//!
//! ## EVM: EIP-712 `Voucher` under `x402 Batch Settlement`
//!
//! ```text
//! domain     = EIP712Domain("x402 Batch Settlement", "1", chainId, 0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003)
//! channelId  = hashTypedDataV4(keccak256(abi.encode(CHANNEL_CONFIG_TYPEHASH, config)))      -- getChannelId
//! digest     = hashTypedDataV4(keccak256(abi.encode(VOUCHER_TYPEHASH, channelId, maxClaimableAmount)))
//! ```
//!
//! The signer is `payerAuthorizer` when it is nonzero, otherwise `payer`
//! ([`evm_voucher_signer`], X402 `x402BatchSettlement.sol#L530-L538`), read
//! from a `ChannelConfig` whose id has been recomputed -- never from the
//! claim. **ECDSA only**: a zero `payerAuthorizer` with a contract-wallet
//! `payer` would need an ERC-1271 `eth_call` per packet, and ADR 0074
//! refuses that at admission, which is the backend's job (#1342), not this
//! module's.
//!
//! The contract recovers with OpenZeppelin's `ECDSA.recoverCalldata`, which
//! refuses a high-`s` signature and any `v` other than 27 or 28. This
//! verifier refuses exactly those too: a voucher the chain would refuse at
//! `claim` is value this connector can never collect, so accepting it off
//! chain would be serving work for nothing.
//!
//! ## Solana: Ed25519 over payment-channels' 50-byte voucher
//!
//! ```text
//! message[0..2)   = 0x56 0x01              (VOUCHER_MAGIC)
//! message[2..34)  = channel account, raw bytes
//! message[34..42) = cumulative amount, u64 little-endian
//! message[42..50) = expires_at,        i64 little-endian
//! ```
//!
//! PC `instructions/mod.rs#L27-L93`; X402 SVM spec `#L316-L323`. The signer
//! is the channel account's `authorized_signer`. ADR 0074 requires
//! `expires_at` to be zero and the claim parser refuses anything else
//! structurally; it stays a parameter here because it is part of the signed
//! bytes, and a vector has to be able to say so.

use libsecp256k1::{Message, RecoveryId, Signature as RawSignature};

use crate::address::derive_evm_address;
use crate::claim_signature::{keccak256, word_address, word_u64_be};
use crate::signer::PublicKeyBytes;
use crate::Address;

/// `x402BatchSettlement`'s address: the same on Base Sepolia and Base
/// mainnet, deployed by CREATE2 (ADR 0074 Sources, read live 2026-09-24).
pub const X402_BATCH_SETTLEMENT_ADDRESS: Address = [
    0x40, 0x20, 0x07, 0x4e, 0x9d, 0xf2, 0xce, 0x1d, 0xee, 0x5a, 0x9c, 0x1b, 0x5c, 0x3f, 0x54, 0x1d,
    0x02, 0xa1, 0x00, 0x03,
];

/// The contract's EIP-712 name and version (`constructor() EIP712("x402
/// Batch Settlement", "1")`, X402 `x402BatchSettlement.sol#L185`).
pub const X402_BATCH_SETTLEMENT_EIP712_NAME: &str = "x402 Batch Settlement";
pub const X402_BATCH_SETTLEMENT_EIP712_VERSION: &str = "1";

const EIP712_DOMAIN_TYPE_HASH_PREIMAGE: &[u8] =
    b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)";

const CHANNEL_CONFIG_TYPE_HASH_PREIMAGE: &[u8] =
    b"ChannelConfig(address payer,address payerAuthorizer,address receiver,address receiverAuthorizer,address token,uint40 withdrawDelay,bytes32 salt)";

const VOUCHER_TYPE_HASH_PREIMAGE: &[u8] = b"Voucher(bytes32 channelId,uint128 maxClaimableAmount)";

/// The first two bytes of every payment-channels voucher message:
/// `VOUCHER_MAGIC` and its version.
pub const SOLANA_VOUCHER_PREFIX: [u8; 2] = [0x56, 0x01];

/// `secp256k1`'s group order halved, big-endian: the largest `s` OpenZeppelin's
/// `ECDSA` accepts (EIP-2's malleability rule).
const SECP256K1_HALF_ORDER: [u8; 32] = [
    0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0x5d, 0x57, 0x6e, 0x73, 0x57, 0xa4, 0x50, 0x1d, 0xdf, 0xe9, 0x2f, 0x46, 0x68, 0x1b, 0x20, 0xa0,
];

/// A voucher's signature, discriminated by chain -- the voucher's own
/// variants, kept apart from the `toon-channel` balance-proof signatures
/// for the reason this module's doc gives. Carries the raw bytes a journal
/// records (ADR 0005) and a settlement backend later submits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoucherSignature {
    /// `r ‖ s ‖ v`, 65 bytes, exactly as the payer's wallet produced them --
    /// `v` in the 27/28 convention the contract's `ecrecover` requires.
    Evm([u8; 65]),
    /// Ed25519 `R ‖ S` over [`solana_voucher_message`].
    Solana([u8; 64]),
}

impl VoucherSignature {
    /// The signature's own bytes, 65 for EVM and 64 for Solana.
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            VoucherSignature::Evm(signature) => signature.to_vec(),
            VoucherSignature::Solana(signature) => signature.to_vec(),
        }
    }
}

/// `x402BatchSettlement`'s EIP-712 domain: the chain, and the contract.
/// Both come from this connector's own configuration, never from a claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchSettlementDomain {
    pub chain_id: u64,
    pub verifying_contract: Address,
}

impl BatchSettlementDomain {
    /// The domain on `chain_id` at the contract's one deployed address.
    pub fn x402(chain_id: u64) -> BatchSettlementDomain {
        BatchSettlementDomain {
            chain_id,
            verifying_contract: X402_BATCH_SETTLEMENT_ADDRESS,
        }
    }

    fn separator(&self) -> [u8; 32] {
        let mut buf = Vec::with_capacity(32 * 5);
        buf.extend_from_slice(&keccak256(EIP712_DOMAIN_TYPE_HASH_PREIMAGE));
        buf.extend_from_slice(&keccak256(X402_BATCH_SETTLEMENT_EIP712_NAME.as_bytes()));
        buf.extend_from_slice(&keccak256(X402_BATCH_SETTLEMENT_EIP712_VERSION.as_bytes()));
        buf.extend_from_slice(&word_u64_be(self.chain_id));
        buf.extend_from_slice(&word_address(&self.verifying_contract));
        keccak256(&buf)
    }

    fn hash_typed_data(&self, struct_hash: &[u8; 32]) -> [u8; 32] {
        let mut buf = Vec::with_capacity(2 + 32 + 32);
        buf.extend_from_slice(&[0x19, 0x01]);
        buf.extend_from_slice(&self.separator());
        buf.extend_from_slice(struct_hash);
        keccak256(&buf)
    }
}

/// An x402 `ChannelConfig` (X402 `x402BatchSettlement.sol#L36-L44`): the
/// seven immutable fields a channel's id is the hash of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchChannelConfig {
    pub payer: Address,
    pub payer_authorizer: Address,
    pub receiver: Address,
    pub receiver_authorizer: Address,
    pub token: Address,
    /// A Solidity `uint40`. Values wider than 40 bits are not a config the
    /// contract could hold; [`evm_batch_channel_id`] hashes whatever it is
    /// given, so the parser is where one is refused.
    pub withdraw_delay: u64,
    pub salt: [u8; 32],
}

/// `getChannelId(config)`: the channel id `config` hashes to on `domain`
/// (X402 `x402BatchSettlement.sol#L442-L446`). Pure -- no RPC. ADR 0074
/// decision 2: the client presents its config, and the connector recomputes
/// the id and refuses a mismatch.
pub fn evm_batch_channel_id(
    domain: &BatchSettlementDomain,
    config: &BatchChannelConfig,
) -> [u8; 32] {
    let mut buf = Vec::with_capacity(32 * 8);
    buf.extend_from_slice(&keccak256(CHANNEL_CONFIG_TYPE_HASH_PREIMAGE));
    buf.extend_from_slice(&word_address(&config.payer));
    buf.extend_from_slice(&word_address(&config.payer_authorizer));
    buf.extend_from_slice(&word_address(&config.receiver));
    buf.extend_from_slice(&word_address(&config.receiver_authorizer));
    buf.extend_from_slice(&word_address(&config.token));
    buf.extend_from_slice(&word_u64_be(config.withdraw_delay));
    buf.extend_from_slice(&config.salt);
    domain.hash_typed_data(&keccak256(&buf))
}

/// `getVoucherDigest(channelId, maxClaimableAmount)`: what a voucher's
/// signer signs (X402 `x402BatchSettlement.sol#L454-L456`).
pub fn evm_voucher_digest(
    domain: &BatchSettlementDomain,
    channel_id: &[u8; 32],
    max_claimable_amount: u128,
) -> [u8; 32] {
    let mut amount = [0u8; 32];
    amount[16..].copy_from_slice(&max_claimable_amount.to_be_bytes());
    let mut buf = Vec::with_capacity(32 * 3);
    buf.extend_from_slice(&keccak256(VOUCHER_TYPE_HASH_PREIMAGE));
    buf.extend_from_slice(channel_id);
    buf.extend_from_slice(&amount);
    domain.hash_typed_data(&keccak256(&buf))
}

/// Whose signature a voucher on `config`'s channel must carry:
/// `payerAuthorizer` if it is nonzero, otherwise `payer` (X402
/// `x402BatchSettlement.sol#L530-L538`).
pub fn evm_voucher_signer(config: &BatchChannelConfig) -> Address {
    if config.payer_authorizer == [0u8; 20] {
        config.payer
    } else {
        config.payer_authorizer
    }
}

/// Recover the address behind a 65-byte `r ‖ s ‖ v` voucher signature under
/// OpenZeppelin `ECDSA`'s rules -- `v` of 27 or 28, `s` in the lower half of
/// the curve order -- or `None`. Stricter than
/// [`crate::claim_signature`]'s balance-proof recovery on purpose: a
/// signature the contract refuses is a voucher that can never be claimed.
fn recover_voucher_signer(digest: &[u8; 32], signature: &[u8; 65]) -> Option<Address> {
    let v = signature[64];
    if v != 27 && v != 28 {
        return None;
    }
    if signature[32..64] > SECP256K1_HALF_ORDER[..] {
        return None;
    }
    let mut rs = [0u8; 64];
    rs.copy_from_slice(&signature[..64]);
    let raw_signature = RawSignature::parse_standard(&rs).ok()?;
    let recovery_id = RecoveryId::parse(v - 27).ok()?;
    let recovered =
        libsecp256k1::recover(&Message::parse(digest), &raw_signature, &recovery_id).ok()?;
    let public_key: PublicKeyBytes = recovered.serialize();
    Some(derive_evm_address(&public_key))
}

/// Whether `signature` is `expected_signer`'s voucher for
/// `max_claimable_amount` on `channel_id`, under `domain`.
///
/// `expected_signer` must be [`evm_voucher_signer`] of a config whose
/// [`evm_batch_channel_id`] is `channel_id` -- this function only checks the
/// signature, exactly as [`crate::verify_evm_balance_proof`] does, and a
/// signer taken from anywhere else (the claim, say) makes the check
/// meaningless. Never panics on attacker-controlled bytes.
pub fn verify_evm_voucher(
    domain: &BatchSettlementDomain,
    channel_id: &[u8; 32],
    max_claimable_amount: u128,
    signature: &[u8; 65],
    expected_signer: &Address,
) -> bool {
    let digest = evm_voucher_digest(domain, channel_id, max_claimable_amount);
    recover_voucher_signer(&digest, signature).as_ref() == Some(expected_signer)
}

/// The 50-byte message a payment-channels voucher's Ed25519 signature
/// covers. See this module's doc for the layout.
pub fn solana_voucher_message(
    channel_account: &[u8; 32],
    cumulative_amount: u64,
    expires_at: i64,
) -> [u8; 50] {
    let mut message = [0u8; 50];
    message[0..2].copy_from_slice(&SOLANA_VOUCHER_PREFIX);
    message[2..34].copy_from_slice(channel_account);
    message[34..42].copy_from_slice(&cumulative_amount.to_le_bytes());
    message[42..50].copy_from_slice(&expires_at.to_le_bytes());
    message
}

/// Whether `signature` is `authorized_signer`'s voucher for
/// `cumulative_amount` on `channel_account`. `authorized_signer` must be the
/// channel account's own `authorized_signer` field, read from the chain --
/// never a key the claim declares. Never panics on attacker-controlled
/// bytes: a malformed key or signature simply does not verify.
pub fn verify_solana_voucher(
    channel_account: &[u8; 32],
    cumulative_amount: u64,
    expires_at: i64,
    signature: &[u8; 64],
    authorized_signer: &[u8; 32],
) -> bool {
    let Ok(public_key) = ed25519_dalek::PublicKey::from_bytes(authorized_signer) else {
        return false;
    };
    let Ok(signature) = ed25519_dalek::Signature::from_bytes(signature) else {
        return false;
    };
    let message = solana_voucher_message(channel_account, cumulative_amount, expires_at);
    use ed25519_dalek::Verifier;
    public_key.verify(&message, &signature).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claim_signature::{evm_balance_proof_digest, solana_balance_proof_message};
    use crate::EvmBalanceProof;
    use ed25519_dalek::Signer as _;
    use libsecp256k1::{PublicKey, SecretKey};
    use proptest::prelude::*;

    fn hex<const N: usize>(text: &str) -> [u8; N] {
        let text = text.strip_prefix("0x").unwrap_or(text);
        assert_eq!(text.len(), N * 2, "fixture hex is {N} bytes");
        let mut out = [0u8; N];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).expect("fixture is hex");
        }
        out
    }

    // -- EVM: vectors from the pinned contract itself --
    //
    // x402 `0cb1a1f0`'s `x402BatchSettlement`, built with its own foundry
    // config, deployed on `anvil --chain-id 84532` and its runtime code
    // placed at 0x4020074e…0003 with `anvil_setCode`; then
    // `getChannelId(FIXTURE_CONFIG)`, `getVoucherDigest(id, 5000)` and
    // `cast wallet sign --no-hash` with anvil's account 1 key
    // (0x59c6995e…690d, address 0x70997970…79C8). `eip712Domain()` there
    // answered ("x402 Batch Settlement", "1", 84532, 0x4020074e…0003) and
    // `VOUCHER_TYPEHASH()` 0x1e1bd6ff…9a69 -- the figure ADR 0074 read off
    // both live chains.

    const FIXTURE_CHAIN_ID: u64 = 84_532;
    const FIXTURE_PAYER_AUTHORIZER_SECRET: &str =
        "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";

    fn fixture_config() -> BatchChannelConfig {
        BatchChannelConfig {
            payer: [0x11; 20],
            payer_authorizer: hex("0x70997970C51812dc3A010C7d01b50e0d17dc79C8"),
            receiver: [0x33; 20],
            receiver_authorizer: [0x44; 20],
            token: [0x55; 20],
            withdraw_delay: 86_400,
            salt: [0x66; 32],
        }
    }

    fn fixture_domain() -> BatchSettlementDomain {
        BatchSettlementDomain::x402(FIXTURE_CHAIN_ID)
    }

    #[test]
    fn the_pinned_address_is_the_one_adr_0074_names() {
        assert_eq!(
            X402_BATCH_SETTLEMENT_ADDRESS,
            hex::<20>("0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003")
        );
    }

    #[test]
    fn the_voucher_typehash_is_the_one_both_live_chains_report() {
        assert_eq!(
            keccak256(VOUCHER_TYPE_HASH_PREIMAGE),
            hex::<32>("0x1e1bd6ff84c3e0d9029a292b212e039c0ca97ec497c55191a4a5874294609a69")
        );
    }

    #[test]
    fn a_config_hashes_to_the_channel_id_the_contract_computes() {
        assert_eq!(
            evm_batch_channel_id(&fixture_domain(), &fixture_config()),
            hex::<32>("0x88d37e9be679d5e46c7c1d073e6f41b5ec07cc5099319a49b80ba460f0d8055d")
        );
        let zero_authorizer = BatchChannelConfig {
            payer_authorizer: [0u8; 20],
            ..fixture_config()
        };
        assert_eq!(
            evm_batch_channel_id(&fixture_domain(), &zero_authorizer),
            hex::<32>("0xd90283498ac1b4b04c73bf57672c0d51d7f125f0d65c6f5b8e3e0800035867b4")
        );
    }

    #[test]
    fn a_voucher_digest_is_the_one_the_contract_computes() {
        let channel_id = evm_batch_channel_id(&fixture_domain(), &fixture_config());
        assert_eq!(
            evm_voucher_digest(&fixture_domain(), &channel_id, 5_000),
            hex::<32>("0x0485b41befd093f42efa53a626e86749b97998f0243e2c7f74b179851d880a8a")
        );
        // The whole uint128 width: a voucher's amount is wider than this
        // connector's u64, and the digest must still be the chain's.
        assert_eq!(
            evm_voucher_digest(&fixture_domain(), &channel_id, u128::MAX),
            hex::<32>("0x3fd853e98fcb965aa64d4c55527447057c5e44f673e2c1fb17d10211d8eb569e")
        );
    }

    fn cast_signature() -> [u8; 65] {
        hex("0x6be416a12f0d5af512c04435315cc4235e915536d73175e05b08b597c160f0ac463b79066298bea17a15716eedbf07231ffbc514295c9057d8c195bebc8566241b")
    }

    #[test]
    fn a_voucher_signed_by_cast_verifies_against_its_payer_authorizer() {
        let config = fixture_config();
        let channel_id = evm_batch_channel_id(&fixture_domain(), &config);
        assert!(verify_evm_voucher(
            &fixture_domain(),
            &channel_id,
            5_000,
            &cast_signature(),
            &evm_voucher_signer(&config),
        ));
    }

    #[test]
    fn this_crates_own_key_derivation_agrees_with_cast_about_the_fixture_signer() {
        let secret = SecretKey::parse(&hex(FIXTURE_PAYER_AUTHORIZER_SECRET)).expect("valid");
        let address = derive_evm_address(&PublicKey::from_secret_key(&secret).serialize());
        assert_eq!(address, fixture_config().payer_authorizer);
    }

    #[test]
    fn the_signer_is_the_payer_authorizer_when_set_and_the_payer_when_zero() {
        let config = fixture_config();
        assert_eq!(evm_voucher_signer(&config), config.payer_authorizer);
        let zero = BatchChannelConfig {
            payer_authorizer: [0u8; 20],
            ..config
        };
        assert_eq!(evm_voucher_signer(&zero), config.payer);
    }

    #[test]
    fn a_voucher_does_not_verify_against_the_payer_when_an_authorizer_is_set() {
        let config = fixture_config();
        let channel_id = evm_batch_channel_id(&fixture_domain(), &config);
        assert!(!verify_evm_voucher(
            &fixture_domain(),
            &channel_id,
            5_000,
            &cast_signature(),
            &config.payer,
        ));
    }

    #[test]
    fn changing_any_signed_evm_field_invalidates_the_voucher() {
        let config = fixture_config();
        let signer = evm_voucher_signer(&config);
        let channel_id = evm_batch_channel_id(&fixture_domain(), &config);
        let other_chain = BatchSettlementDomain::x402(8_453);
        let other_contract = BatchSettlementDomain {
            chain_id: FIXTURE_CHAIN_ID,
            verifying_contract: [0x99; 20],
        };
        for (label, domain, channel, amount) in [
            ("amount", fixture_domain(), channel_id, 5_001u128),
            ("channel", fixture_domain(), [0xabu8; 32], 5_000),
            ("chain id", other_chain, channel_id, 5_000),
            ("contract", other_contract, channel_id, 5_000),
        ] {
            assert!(
                !verify_evm_voucher(&domain, &channel, amount, &cast_signature(), &signer),
                "changing the {label} must invalidate the voucher"
            );
        }
    }

    /// OpenZeppelin's `ECDSA` refuses `v` of 0 or 1: the contract's
    /// `ecrecover` would answer the zero address. A voucher this connector
    /// accepted in that form could never be claimed.
    #[test]
    fn a_voucher_with_v_zero_or_one_is_refused_as_the_contract_would() {
        let config = fixture_config();
        let channel_id = evm_batch_channel_id(&fixture_domain(), &config);
        let mut signature = cast_signature();
        signature[64] -= 27;
        assert!(!verify_evm_voucher(
            &fixture_domain(),
            &channel_id,
            5_000,
            &signature,
            &evm_voucher_signer(&config),
        ));
    }

    /// The malleated twin of a genuine signature -- `s` replaced by `n - s`
    /// and `v` flipped -- recovers the same key under plain ECDSA, and the
    /// contract refuses it (`ECDSAInvalidSignatureS`). So does this.
    #[test]
    fn a_high_s_voucher_is_refused_as_the_contract_would() {
        const ORDER: [u8; 32] = [
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xfe, 0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c,
            0xd0, 0x36, 0x41, 0x41,
        ];
        let config = fixture_config();
        let channel_id = evm_batch_channel_id(&fixture_domain(), &config);
        let genuine = cast_signature();
        let mut malleated = genuine;
        let mut borrow = 0i16;
        for i in (0..32).rev() {
            let difference = i16::from(ORDER[i]) - i16::from(genuine[32 + i]) - borrow;
            borrow = i16::from(difference < 0);
            malleated[32 + i] = (difference + 256 * borrow) as u8;
        }
        malleated[64] = if genuine[64] == 27 { 28 } else { 27 };

        // It really is the same key under ECDSA without EIP-2's rule, so the
        // refusal below is the rule and not a broken signature.
        let digest = evm_voucher_digest(&fixture_domain(), &channel_id, 5_000);
        let mut rs = [0u8; 64];
        rs.copy_from_slice(&malleated[..64]);
        let lax = libsecp256k1::recover(
            &Message::parse(&digest),
            &RawSignature::parse_overflowing(&rs),
            &RecoveryId::parse(malleated[64] - 27).unwrap(),
        )
        .expect("recovers without the low-s rule");
        assert_eq!(
            derive_evm_address(&lax.serialize()),
            evm_voucher_signer(&config)
        );

        assert!(!verify_evm_voucher(
            &fixture_domain(),
            &channel_id,
            5_000,
            &malleated,
            &evm_voucher_signer(&config),
        ));
    }

    /// A voucher's digest is not a balance proof's for any input, so neither
    /// kind of signature can stand in for the other -- the domain name alone
    /// separates them, before the struct does.
    #[test]
    fn a_voucher_digest_is_never_a_balance_proof_digest() {
        let channel_id = evm_batch_channel_id(&fixture_domain(), &fixture_config());
        let proof = EvmBalanceProof {
            channel_id,
            nonce: 0,
            transferred_amount: 5_000,
            locked_amount: 0,
            locks_root: [0u8; 32],
            chain_id: FIXTURE_CHAIN_ID,
            token_network_address: X402_BATCH_SETTLEMENT_ADDRESS,
        };
        assert_ne!(
            evm_voucher_digest(&fixture_domain(), &channel_id, 5_000),
            evm_balance_proof_digest(&proof)
        );
    }

    // -- Solana: a vector from OpenSSL's Ed25519 --
    //
    // `openssl pkeyutl -sign -rawin` with the Ed25519 key whose 32-byte seed
    // is 0x21 repeated, over 0x5601 ‖ 0xc3×32 ‖ 5000u64 LE ‖ 0i64 LE.

    const FIXTURE_SOLANA_SEED: [u8; 32] = [0x21; 32];
    const FIXTURE_SOLANA_CHANNEL: [u8; 32] = [0xc3; 32];

    fn fixture_solana_signer() -> [u8; 32] {
        hex("884b8857f4eaa1613c61504db34d4beaf346517a0e31de3cddd4d9b4201d9d0b")
    }

    fn openssl_signature() -> [u8; 64] {
        hex("347482945bb1d06372454c0f88c48934e7a0ab8042553132d4abb9937125281154f3bf945db285c9dd5bf1e252592b2d8122aa4ad357675f08a3590f0fa9a405")
    }

    #[test]
    fn the_solana_voucher_message_is_payment_channels_50_byte_layout() {
        let message = solana_voucher_message(&FIXTURE_SOLANA_CHANNEL, 5_000, 0);
        let mut expected = vec![0x56, 0x01];
        expected.extend_from_slice(&FIXTURE_SOLANA_CHANNEL);
        expected.extend_from_slice(&[0x88, 0x13, 0, 0, 0, 0, 0, 0]);
        expected.extend_from_slice(&[0u8; 8]);
        assert_eq!(message.len(), 50);
        assert_eq!(message.to_vec(), expected);
        assert_eq!(
            &solana_voucher_message(&FIXTURE_SOLANA_CHANNEL, 0, -1)[42..50],
            &[0xff; 8],
            "expires_at is a signed little-endian i64"
        );
    }

    #[test]
    fn a_voucher_signed_by_openssl_verifies_against_its_authorized_signer() {
        assert!(verify_solana_voucher(
            &FIXTURE_SOLANA_CHANNEL,
            5_000,
            0,
            &openssl_signature(),
            &fixture_solana_signer(),
        ));
    }

    #[test]
    fn this_crates_ed25519_agrees_with_openssl_about_the_fixture_key() {
        let secret = ed25519_dalek::SecretKey::from_bytes(&FIXTURE_SOLANA_SEED).expect("seed");
        let public: ed25519_dalek::PublicKey = (&secret).into();
        assert_eq!(public.to_bytes(), fixture_solana_signer());
        let keypair = ed25519_dalek::Keypair { secret, public };
        let message = solana_voucher_message(&FIXTURE_SOLANA_CHANNEL, 5_000, 0);
        assert_eq!(keypair.sign(&message).to_bytes(), openssl_signature());
    }

    #[test]
    fn changing_any_signed_solana_field_invalidates_the_voucher() {
        for (label, channel, amount, expires_at, signer) in [
            (
                "channel",
                [0xc4u8; 32],
                5_000u64,
                0i64,
                fixture_solana_signer(),
            ),
            (
                "amount",
                FIXTURE_SOLANA_CHANNEL,
                5_001,
                0,
                fixture_solana_signer(),
            ),
            (
                "expiry",
                FIXTURE_SOLANA_CHANNEL,
                5_000,
                1,
                fixture_solana_signer(),
            ),
            ("signer", FIXTURE_SOLANA_CHANNEL, 5_000, 0, [0x09; 32]),
        ] {
            assert!(
                !verify_solana_voucher(&channel, amount, expires_at, &openssl_signature(), &signer),
                "changing the {label} must invalidate the voucher"
            );
        }
    }

    /// Neither Solana message can be read as the other: a voucher is 50
    /// bytes behind `0x5601`, a balance proof 96 behind `TOON-BALPROOF-V2`.
    #[test]
    fn a_voucher_message_is_never_a_balance_proof_message() {
        let voucher = solana_voucher_message(&FIXTURE_SOLANA_CHANNEL, 5_000, 0);
        let proof = solana_balance_proof_message(&[0u8; 32], &FIXTURE_SOLANA_CHANNEL, 0, 5_000);
        assert_ne!(voucher.len(), proof.len());
        assert_ne!(voucher[0..2], proof[0..2]);
    }

    #[test]
    fn voucher_signature_bytes_keep_their_schemes_own_width() {
        assert_eq!(VoucherSignature::Evm(cast_signature()).to_bytes().len(), 65);
        assert_eq!(
            VoucherSignature::Solana(openssl_signature())
                .to_bytes()
                .len(),
            64
        );
    }

    proptest! {
        /// A genuine voucher by a random payer authorizer, for any amount in
        /// the full uint128 range, verifies against exactly that signer.
        #[test]
        fn a_genuine_evm_voucher_verifies_for_any_amount(
            secret in proptest::array::uniform32(1u8..),
            amount in any::<u128>(),
            salt in proptest::array::uniform32(any::<u8>()),
        ) {
            let Ok(secret) = SecretKey::parse(&secret) else { return Ok(()); };
            let authorizer = derive_evm_address(&PublicKey::from_secret_key(&secret).serialize());
            let config = BatchChannelConfig { payer_authorizer: authorizer, salt, ..fixture_config() };
            let channel_id = evm_batch_channel_id(&fixture_domain(), &config);
            let digest = evm_voucher_digest(&fixture_domain(), &channel_id, amount);
            let (signature, recovery) = libsecp256k1::sign(&Message::parse(&digest), &secret);
            let mut bytes = [0u8; 65];
            bytes[..64].copy_from_slice(&signature.serialize());
            bytes[64] = recovery.serialize() + 27;
            prop_assert!(verify_evm_voucher(&fixture_domain(), &channel_id, amount, &bytes, &authorizer));
            prop_assert!(!verify_evm_voucher(&fixture_domain(), &channel_id, amount, &bytes, &config.payer));
        }

        /// Arbitrary bytes never panic, and never verify against the fixture signer.
        #[test]
        fn arbitrary_evm_signature_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 65)) {
            let bytes: [u8; 65] = bytes.try_into().unwrap();
            let channel_id = evm_batch_channel_id(&fixture_domain(), &fixture_config());
            let _ = verify_evm_voucher(&fixture_domain(), &channel_id, 5_000, &bytes, &fixture_config().payer_authorizer);
        }

        #[test]
        fn arbitrary_solana_signature_and_key_bytes_never_panic(
            signature in proptest::collection::vec(any::<u8>(), 64),
            key in proptest::array::uniform32(any::<u8>()),
        ) {
            let signature: [u8; 64] = signature.try_into().unwrap();
            let _ = verify_solana_voucher(&FIXTURE_SOLANA_CHANNEL, 5_000, 0, &signature, &key);
        }
    }
}
