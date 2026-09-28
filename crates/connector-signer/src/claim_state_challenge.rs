//! A read-only "prove you control this channel" signature, distinct from
//! [`crate::claim_signature`]'s balance-proof signature (issue #693).
//!
//! The claim-state endpoint (`connector-client-edge`'s `POST
//! /ilp/claim-state`) needs a caller to prove it holds a channel's
//! counterparty key without moving any value or advancing a watermark --
//! the endpoint is a read. Reusing [`crate::EvmBalanceProof`]'s digest or
//! [`crate::solana_balance_proof_message`]'s message verbatim for that
//! would make a captured challenge signature ambiguous with (and possibly
//! replayable as) a real claim, since both would recover to the same
//! bytes for the same channel/nonce/amount. This module signs a
//! **domain-separated** struct/message instead -- same signature schemes
//! (EIP-712 for EVM, Ed25519 for Solana), same "verify against the
//! channel's already-registered counterparty, never a self-declared key"
//! rule, but a different typehash (EVM) and a different tagged message
//! (Solana), so neither kind of signature ever verifies as the other.
//!
//! A challenge carries no nonce or amount -- it proves key possession, not
//! a balance -- so replay protection is `expires` alone: a caller
//! reissues a fresh challenge (a new `expires`) each time it wants to
//! prove control again, and this module treats a signature over any past
//! `expires` still in the future as valid, exactly once verified, as many
//! times as asked -- it grants a read, never a write.
//!
//! **An x402 `batch-settlement` channel** (ADR 0074, issue #1364) is proved
//! the same way, by the key that signs its vouchers -- the verified
//! `ChannelConfig`'s `payerAuthorizer` (else `payer`) on EVM,
//! `authorized_signer` on Solana -- over a challenge kept apart from that
//! key's **vouchers** exactly as the one above is kept apart from a balance
//! proof. On EVM it is the same `ClaimStateChallenge` struct, signed under
//! the channel's own recorded domain, which for a voucher channel is
//! `x402BatchSettlement`'s ([`evm_voucher_claim_state_challenge_digest`]):
//! a different typehash from `Voucher`, so neither verifies as the other,
//! and a different domain from `TokenNetwork`'s, so neither challenge
//! verifies as the other. On Solana it is a tagged message of its own
//! ([`solana_voucher_claim_state_challenge_message`]), which starts with
//! neither the voucher's `0x56 0x01` nor the `toon-channel` challenge's tag.
//!
//! **The same message proves the peer role** (ADR 0075 decision 5, issue
//! #1377): a peer's packet that moves no value carries it, signed by a bound
//! channel's voucher signer, in place of a voucher. Both uses say "I control
//! this channel's voucher signer, until `expires`" to the channel's receiver
//! and neither moves value, so one message serves both. What must stay apart
//! is the challenge and the voucher, and the separation tests below hold in
//! both directions for that use too.

use crate::claim_signature::{domain_separator, keccak256, recover_evm_signer, word_u64_be};
use crate::voucher_signature::BatchSettlementDomain;
use crate::Address;

/// The fields an EVM claim-state challenge's EIP-712 signature covers.
/// Signed under the same `EIP712Domain(name: "TokenNetwork", version: "1",
/// chainId, verifyingContract)` a real `BalanceProof` is (the channel's own
/// recorded domain, per ADR 0024 -- never a claim's/challenge's own
/// self-declared one), but a distinct struct name and field set, so its
/// digest can never collide with a `BalanceProof` digest for any input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvmClaimStateChallenge {
    pub channel_id: [u8; 32],
    /// Unix seconds past which this challenge no longer verifies.
    pub expires: u64,
    pub chain_id: u64,
    pub token_network_address: Address,
}

const CLAIM_STATE_CHALLENGE_TYPE_HASH_PREIMAGE: &[u8] =
    b"ClaimStateChallenge(bytes32 channelId,uint256 expires)";

fn struct_hash(channel_id: &[u8; 32], expires: u64) -> [u8; 32] {
    let mut buf = Vec::with_capacity(32 * 3);
    buf.extend_from_slice(&keccak256(CLAIM_STATE_CHALLENGE_TYPE_HASH_PREIMAGE));
    buf.extend_from_slice(channel_id);
    buf.extend_from_slice(&word_u64_be(expires));
    keccak256(&buf)
}

/// The EIP-712 digest `challenge`'s signer actually signs -- exposed so a
/// test (or a client implementation) can produce a genuine signature over
/// exactly what [`verify_evm_claim_state_challenge`] checks.
pub fn evm_claim_state_challenge_digest(challenge: &EvmClaimStateChallenge) -> [u8; 32] {
    let mut buf = Vec::with_capacity(2 + 32 + 32);
    buf.extend_from_slice(&[0x19, 0x01]);
    buf.extend_from_slice(&domain_separator(
        challenge.chain_id,
        &challenge.token_network_address,
    ));
    buf.extend_from_slice(&struct_hash(&challenge.channel_id, challenge.expires));
    keccak256(&buf)
}

/// Whether `signature` is a valid EIP-712 `ClaimStateChallenge` signature
/// over `challenge`, produced by `expected_counterparty` -- the channel's
/// registered counterparty, exactly as [`crate::verify_evm_balance_proof`]
/// never trusts a self-declared signer.
pub fn verify_evm_claim_state_challenge(
    challenge: &EvmClaimStateChallenge,
    signature: &[u8],
    expected_counterparty: &Address,
) -> bool {
    let digest = evm_claim_state_challenge_digest(challenge);
    recover_evm_signer(&digest, signature).as_ref() == Some(expected_counterparty)
}

/// Domain tag prefixing every Solana claim-state challenge message --
/// chosen to be neither the same length nor a prefix/suffix of
/// [`crate::solana_balance_proof_message`]'s 96 bytes, so the two
/// message layouts can never collide for any channel/nonce/amount/expires.
const SOLANA_CHALLENGE_DOMAIN_TAG: &[u8] = b"toon-claim-state-challenge-v1";

/// The message a Solana claim-state challenge's Ed25519 signature covers:
/// a fixed domain tag, the channel account, and the challenge's expiry.
pub fn solana_claim_state_challenge_message(channel_account: &[u8; 32], expires: u64) -> Vec<u8> {
    let mut message = Vec::with_capacity(SOLANA_CHALLENGE_DOMAIN_TAG.len() + 32 + 8);
    message.extend_from_slice(SOLANA_CHALLENGE_DOMAIN_TAG);
    message.extend_from_slice(channel_account);
    message.extend_from_slice(&expires.to_le_bytes());
    message
}

/// Whether `signature` is a valid Ed25519 signature, by
/// `signer_public_key`, over the claim-state challenge message for
/// `channel_account`/`expires`. `signer_public_key` must already be the
/// channel's registered counterparty key for this to mean anything -- this
/// function only checks the signature is genuine for whatever key it is
/// given.
pub fn verify_solana_claim_state_challenge(
    channel_account: &[u8; 32],
    expires: u64,
    signature: &[u8],
    signer_public_key: &[u8; 32],
) -> bool {
    let message = solana_claim_state_challenge_message(channel_account, expires);
    verify_ed25519(&message, signature, signer_public_key)
}

/// The EIP-712 digest an x402 `batch-settlement` channel's voucher signer
/// signs to prove control of EVM channel `channel_id` for a claim-state read
/// (issue #1364): `ClaimStateChallenge(bytes32 channelId,uint256 expires)`
/// under `domain`, `x402BatchSettlement`'s own EIP-712 domain -- the
/// channel's recorded domain, as a `toon-channel` challenge is signed under
/// its channel's `TokenNetwork`. `domain` is this connector's, never the
/// request's.
pub fn evm_voucher_claim_state_challenge_digest(
    domain: &BatchSettlementDomain,
    channel_id: &[u8; 32],
    expires: u64,
) -> [u8; 32] {
    domain.hash_typed_data(&struct_hash(channel_id, expires))
}

/// Whether `signature` is `expected_signer`'s claim-state challenge for
/// batch-settlement channel `channel_id` under `domain`. `expected_signer`
/// must be `crate::evm_voucher_signer` of a config that hashes to
/// `channel_id` -- the key the chain checks its vouchers against, never one
/// the request names.
pub fn verify_evm_voucher_claim_state_challenge(
    domain: &BatchSettlementDomain,
    channel_id: &[u8; 32],
    expires: u64,
    signature: &[u8],
    expected_signer: &Address,
) -> bool {
    let digest = evm_voucher_claim_state_challenge_digest(domain, channel_id, expires);
    recover_evm_signer(&digest, signature).as_ref() == Some(expected_signer)
}

/// Domain tag prefixing every Solana batch-settlement claim-state challenge.
/// Neither a prefix of [`SOLANA_CHALLENGE_DOMAIN_TAG`] nor prefixed by it,
/// and not starting with a voucher's `0x56 0x01`
/// ([`crate::SOLANA_VOUCHER_PREFIX`]).
const SOLANA_VOUCHER_CHALLENGE_DOMAIN_TAG: &[u8] = b"toon-voucher-claim-state-challenge-v1";

/// The message a Solana batch-settlement channel's `authorized_signer`
/// signs to prove control of `channel_account` for a claim-state read
/// (issue #1364): a fixed domain tag, the channel account, and the
/// challenge's expiry, `u64` little-endian.
pub fn solana_voucher_claim_state_challenge_message(
    channel_account: &[u8; 32],
    expires: u64,
) -> Vec<u8> {
    let mut message = Vec::with_capacity(SOLANA_VOUCHER_CHALLENGE_DOMAIN_TAG.len() + 32 + 8);
    message.extend_from_slice(SOLANA_VOUCHER_CHALLENGE_DOMAIN_TAG);
    message.extend_from_slice(channel_account);
    message.extend_from_slice(&expires.to_le_bytes());
    message
}

/// Whether `signature` is `authorized_signer`'s claim-state challenge for
/// batch-settlement channel `channel_account`. `authorized_signer` must be
/// the channel account's own field, read from the chain.
pub fn verify_solana_voucher_claim_state_challenge(
    channel_account: &[u8; 32],
    expires: u64,
    signature: &[u8],
    authorized_signer: &[u8; 32],
) -> bool {
    let message = solana_voucher_claim_state_challenge_message(channel_account, expires);
    verify_ed25519(&message, signature, authorized_signer)
}

fn verify_ed25519(message: &[u8], signature: &[u8], public_key: &[u8; 32]) -> bool {
    let Ok(public_key) = ed25519_dalek::PublicKey::from_bytes(public_key) else {
        return false;
    };
    let Ok(signature) = ed25519_dalek::Signature::from_bytes(signature) else {
        return false;
    };
    use ed25519_dalek::Verifier;
    public_key.verify(message, &signature).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::derive_evm_address;
    use ed25519_dalek::Signer as Ed25519Signer;
    use libsecp256k1::{Message, PublicKey, SecretKey};
    use rand::rngs::OsRng;

    fn sample_challenge() -> EvmClaimStateChallenge {
        EvmClaimStateChallenge {
            channel_id: [1u8; 32],
            expires: 1_800_000_000,
            chain_id: 8453,
            token_network_address: [0x42; 20],
        }
    }

    fn sign_as_a_wallet_would(secret: &SecretKey, digest: &[u8; 32]) -> Vec<u8> {
        let message = Message::parse(digest);
        let (signature, recovery_id) = libsecp256k1::sign(&message, secret);
        let serialized = signature.serialize();
        let mut bytes = Vec::with_capacity(65);
        bytes.extend_from_slice(&serialized);
        let recovery_byte: u8 = recovery_id.into();
        bytes.push(recovery_byte + 27);
        bytes
    }

    fn generate_evm_keypair() -> (SecretKey, Address) {
        let secret = SecretKey::random(&mut OsRng);
        let public = PublicKey::from_secret_key(&secret);
        let address = derive_evm_address(&public.serialize());
        (secret, address)
    }

    #[test]
    fn a_genuine_evm_challenge_signature_verifies_against_its_signers_address() {
        let (secret, address) = generate_evm_keypair();
        let challenge = sample_challenge();
        let signature =
            sign_as_a_wallet_would(&secret, &evm_claim_state_challenge_digest(&challenge));

        assert!(verify_evm_claim_state_challenge(
            &challenge, &signature, &address
        ));
    }

    #[test]
    fn an_evm_challenge_signature_does_not_verify_against_a_different_partys_address() {
        let (secret, _address) = generate_evm_keypair();
        let (_other_secret, other_address) = generate_evm_keypair();
        let challenge = sample_challenge();
        let signature =
            sign_as_a_wallet_would(&secret, &evm_claim_state_challenge_digest(&challenge));

        assert!(!verify_evm_claim_state_challenge(
            &challenge,
            &signature,
            &other_address
        ));
    }

    #[test]
    fn a_genuine_balance_proof_signature_does_not_verify_as_a_claim_state_challenge() {
        // The domain-separation property this module exists for: a
        // signature captured off a real payment claim must never double
        // as proof of control for a read.
        let (secret, address) = generate_evm_keypair();
        let proof = crate::EvmBalanceProof {
            channel_id: [1u8; 32],
            nonce: 5,
            transferred_amount: 1_000,
            locked_amount: 0,
            locks_root: [0u8; 32],
            chain_id: 8453,
            token_network_address: [0x42; 20],
        };
        let balance_proof_signature =
            sign_as_a_wallet_would(&secret, &crate::evm_balance_proof_digest(&proof));

        let challenge = EvmClaimStateChallenge {
            channel_id: proof.channel_id,
            expires: proof.nonce, // even if a forger tries to line the fields up
            chain_id: proof.chain_id,
            token_network_address: proof.token_network_address,
        };

        assert!(!verify_evm_claim_state_challenge(
            &challenge,
            &balance_proof_signature,
            &address
        ));
    }

    #[test]
    fn changing_expires_invalidates_a_prior_evm_signature() {
        let (secret, address) = generate_evm_keypair();
        let challenge = sample_challenge();
        let signature =
            sign_as_a_wallet_would(&secret, &evm_claim_state_challenge_digest(&challenge));

        let tampered = EvmClaimStateChallenge {
            expires: challenge.expires + 1,
            ..challenge
        };

        assert!(!verify_evm_claim_state_challenge(
            &tampered, &signature, &address
        ));
    }

    #[test]
    fn a_truncated_evm_signature_fails_to_verify_rather_than_panicking() {
        let (secret, address) = generate_evm_keypair();
        let challenge = sample_challenge();
        let mut signature =
            sign_as_a_wallet_would(&secret, &evm_claim_state_challenge_digest(&challenge));
        signature.truncate(10);

        assert!(!verify_evm_claim_state_challenge(
            &challenge, &signature, &address
        ));
    }

    fn generate_solana_keypair() -> ed25519_dalek::Keypair {
        ed25519_dalek::Keypair::generate(&mut OsRng)
    }

    #[test]
    fn a_genuine_solana_challenge_signature_verifies_against_its_signers_key() {
        let keypair = generate_solana_keypair();
        let channel_account = [3u8; 32];
        let message = solana_claim_state_challenge_message(&channel_account, 1_800_000_000);
        let signature = keypair.sign(&message);

        assert!(verify_solana_claim_state_challenge(
            &channel_account,
            1_800_000_000,
            &signature.to_bytes(),
            &keypair.public.to_bytes(),
        ));
    }

    #[test]
    fn a_solana_challenge_signature_does_not_verify_against_a_different_partys_key() {
        let keypair = generate_solana_keypair();
        let other_keypair = generate_solana_keypair();
        let channel_account = [3u8; 32];
        let message = solana_claim_state_challenge_message(&channel_account, 1_800_000_000);
        let signature = keypair.sign(&message);

        assert!(!verify_solana_claim_state_challenge(
            &channel_account,
            1_800_000_000,
            &signature.to_bytes(),
            &other_keypair.public.to_bytes(),
        ));
    }

    #[test]
    fn a_genuine_solana_balance_proof_signature_does_not_verify_as_a_claim_state_challenge() {
        let keypair = generate_solana_keypair();
        let channel_account = [3u8; 32];
        let balance_proof_message =
            crate::solana_balance_proof_message(&[9u8; 32], &channel_account, 7, 500);
        let signature = keypair.sign(&balance_proof_message);

        assert!(!verify_solana_claim_state_challenge(
            &channel_account,
            7,
            &signature.to_bytes(),
            &keypair.public.to_bytes(),
        ));
    }

    #[test]
    fn a_malformed_solana_public_key_fails_to_verify_rather_than_panicking() {
        let keypair = generate_solana_keypair();
        let channel_account = [3u8; 32];
        let message = solana_claim_state_challenge_message(&channel_account, 1_800_000_000);
        let signature = keypair.sign(&message);

        let malformed_key = [0u8; 32];
        assert!(!verify_solana_claim_state_challenge(
            &channel_account,
            1_800_000_000,
            &signature.to_bytes(),
            &malformed_key,
        ));
    }

    // -- x402 batch-settlement channels (issue #1364) --

    fn voucher_domain() -> BatchSettlementDomain {
        BatchSettlementDomain::x402(84_532)
    }

    #[test]
    fn a_genuine_voucher_channel_challenge_verifies_against_its_signer() {
        let (secret, address) = generate_evm_keypair();
        let digest =
            evm_voucher_claim_state_challenge_digest(&voucher_domain(), &[1u8; 32], 1_800_000_000);
        let signature = sign_as_a_wallet_would(&secret, &digest);

        assert!(verify_evm_voucher_claim_state_challenge(
            &voucher_domain(),
            &[1u8; 32],
            1_800_000_000,
            &signature,
            &address
        ));
        let (_other, other_address) = generate_evm_keypair();
        assert!(!verify_evm_voucher_claim_state_challenge(
            &voucher_domain(),
            &[1u8; 32],
            1_800_000_000,
            &signature,
            &other_address
        ));
        assert!(!verify_evm_voucher_claim_state_challenge(
            &voucher_domain(),
            &[1u8; 32],
            1_800_000_001,
            &signature,
            &address
        ));
    }

    /// The property the challenge exists for: a voucher captured off the wire
    /// never doubles as proof of control, even with its amount lined up
    /// against `expires`.
    #[test]
    fn a_voucher_signature_does_not_verify_as_a_voucher_channel_challenge() {
        let (secret, address) = generate_evm_keypair();
        let voucher = crate::evm_voucher_digest(&voucher_domain(), &[1u8; 32], 1_800_000_000);
        let signature = sign_as_a_wallet_would(&secret, &voucher);

        assert!(!verify_evm_voucher_claim_state_challenge(
            &voucher_domain(),
            &[1u8; 32],
            1_800_000_000,
            &signature,
            &address
        ));
    }

    /// Same struct, different domain: a `toon-channel` challenge and a
    /// voucher-channel challenge over the same id and expiry never verify as
    /// each other, nor does one chain's as another's.
    #[test]
    fn a_voucher_channel_challenge_is_bound_to_x402s_domain_and_chain() {
        let (secret, address) = generate_evm_keypair();
        let toon = EvmClaimStateChallenge {
            channel_id: [1u8; 32],
            expires: 1_800_000_000,
            chain_id: 84_532,
            token_network_address: crate::X402_BATCH_SETTLEMENT_ADDRESS,
        };
        let toon_signature =
            sign_as_a_wallet_would(&secret, &evm_claim_state_challenge_digest(&toon));
        assert!(!verify_evm_voucher_claim_state_challenge(
            &voucher_domain(),
            &[1u8; 32],
            1_800_000_000,
            &toon_signature,
            &address
        ));

        let digest =
            evm_voucher_claim_state_challenge_digest(&voucher_domain(), &[1u8; 32], 1_800_000_000);
        let signature = sign_as_a_wallet_would(&secret, &digest);
        assert!(!verify_evm_claim_state_challenge(
            &toon, &signature, &address
        ));
        assert!(!verify_evm_voucher_claim_state_challenge(
            &BatchSettlementDomain::x402(8453),
            &[1u8; 32],
            1_800_000_000,
            &signature,
            &address
        ));
    }

    #[test]
    fn a_genuine_solana_voucher_channel_challenge_verifies_against_its_signer_only() {
        let keypair = generate_solana_keypair();
        let other = generate_solana_keypair();
        let channel_account = [3u8; 32];
        let message = solana_voucher_claim_state_challenge_message(&channel_account, 1_800_000_000);
        let signature = keypair.sign(&message).to_bytes();

        assert!(verify_solana_voucher_claim_state_challenge(
            &channel_account,
            1_800_000_000,
            &signature,
            &keypair.public.to_bytes(),
        ));
        assert!(!verify_solana_voucher_claim_state_challenge(
            &channel_account,
            1_800_000_000,
            &signature,
            &other.public.to_bytes(),
        ));
        assert!(!verify_solana_voucher_claim_state_challenge(
            &channel_account,
            1_800_000_001,
            &signature,
            &keypair.public.to_bytes(),
        ));
    }

    /// ADR 0075 decision 5 makes the voucher-channel challenge the peer
    /// role's proof for a packet that moves no value, beside a voucher for
    /// one that moves some. The two must stay apart in the other direction
    /// too: a challenge captured off the peer wire never pays, on either
    /// chain, even with its `expires` lined up against the amount.
    #[test]
    fn a_peer_role_challenge_never_verifies_as_a_voucher_on_either_chain() {
        let (secret, address) = generate_evm_keypair();
        let digest =
            evm_voucher_claim_state_challenge_digest(&voucher_domain(), &[1u8; 32], 1_800_000_000);
        let challenge: [u8; 65] = sign_as_a_wallet_would(&secret, &digest)
            .try_into()
            .expect("65 bytes");
        let voucher: [u8; 65] = sign_as_a_wallet_would(
            &secret,
            &crate::evm_voucher_digest(&voucher_domain(), &[1u8; 32], 1_800_000_000),
        )
        .try_into()
        .expect("65 bytes");
        let verifies_as_a_voucher = |signature: &[u8; 65]| {
            crate::verify_evm_voucher(
                &voucher_domain(),
                &[1u8; 32],
                1_800_000_000,
                signature,
                &address,
            )
        };
        assert!(verifies_as_a_voucher(&voucher), "the control");
        assert!(!verifies_as_a_voucher(&challenge));

        let keypair = generate_solana_keypair();
        let channel_account = [3u8; 32];
        let challenge = keypair
            .sign(&solana_voucher_claim_state_challenge_message(
                &channel_account,
                1_800_000_000,
            ))
            .to_bytes();
        let voucher = keypair
            .sign(&crate::solana_voucher_message(
                &channel_account,
                1_800_000_000,
                0,
            ))
            .to_bytes();
        let verifies_as_a_voucher = |signature: &[u8; 64]| {
            crate::verify_solana_voucher(
                &channel_account,
                1_800_000_000,
                0,
                signature,
                &keypair.public.to_bytes(),
            )
        };
        assert!(verifies_as_a_voucher(&voucher), "the control");
        assert!(!verifies_as_a_voucher(&challenge));
    }

    /// Neither a voucher nor a `toon-channel` challenge verifies as a
    /// voucher-channel challenge, and a voucher-channel challenge is neither.
    #[test]
    fn a_solana_voucher_channel_challenge_is_neither_a_voucher_nor_a_toon_challenge() {
        let keypair = generate_solana_keypair();
        let public = keypair.public.to_bytes();
        let channel_account = [3u8; 32];
        let voucher = crate::solana_voucher_message(&channel_account, 1_800_000_000, 0);
        let voucher_signature = keypair.sign(&voucher).to_bytes();
        let toon = solana_claim_state_challenge_message(&channel_account, 1_800_000_000);
        let toon_signature = keypair.sign(&toon).to_bytes();

        for signature in [voucher_signature, toon_signature] {
            assert!(!verify_solana_voucher_claim_state_challenge(
                &channel_account,
                1_800_000_000,
                &signature,
                &public,
            ));
        }
        let message = solana_voucher_claim_state_challenge_message(&channel_account, 1_800_000_000);
        assert!(!verify_solana_claim_state_challenge(
            &channel_account,
            1_800_000_000,
            &keypair.sign(&message).to_bytes(),
            &public,
        ));
        assert!(!message.starts_with(&crate::SOLANA_VOUCHER_PREFIX));
        assert!(!message.starts_with(SOLANA_CHALLENGE_DOMAIN_TAG));
        assert!(!toon.starts_with(SOLANA_VOUCHER_CHALLENGE_DOMAIN_TAG));
    }
}
