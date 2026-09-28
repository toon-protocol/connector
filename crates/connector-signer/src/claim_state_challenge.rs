//! A read-only "prove you control this channel" signature, distinct from a
//! voucher (issues #693, #1364).
//!
//! `POST /ilp/claim-state` needs a caller to prove it holds an x402
//! `batch-settlement` channel's **voucher signer** -- the verified
//! `ChannelConfig`'s `payerAuthorizer` (else `payer`) on EVM,
//! `authorized_signer` on Solana -- without moving any value or advancing a
//! watermark: the endpoint is a read. Reusing the voucher's own digest or
//! message would make a captured challenge ambiguous with (and possibly
//! replayable as) a real payment, so this module signs a
//! **domain-separated** struct/message instead. On EVM it is
//! `ClaimStateChallenge(bytes32 channelId,uint256 expires)` under
//! `x402BatchSettlement`'s EIP-712 domain ([`evm_voucher_claim_state_challenge_digest`]):
//! a different typehash from `Voucher`, so neither verifies as the other. On
//! Solana it is a tagged message of its own
//! ([`solana_voucher_claim_state_challenge_message`]), which does not start
//! with the voucher's `0x56 0x01`.
//!
//! A challenge carries no amount -- it proves key possession, not a balance
//! -- so replay protection is `expires` alone: a caller reissues a fresh
//! challenge each time it wants to prove control again.
//!
//! **The same message proves the peer role** (ADR 0075 decision 5, issue
//! #1377), and **declares a client's channel at BTP auth** (issue #1384,
//! replacing the retired `TokenNetwork`-domain `auth_channel_proof`): both say
//! "I control this channel's voucher signer, until `expires`" to the channel's
//! receiver, and neither moves value, so one message serves all three. What
//! must stay apart is the challenge and the voucher, and the separation tests
//! below hold in both directions.
//!
//! The `toon-channel` claim-state challenge -- the same struct under a
//! `TokenNetwork`'s domain, and the `toon-claim-state-challenge-v1` Solana
//! message -- is deleted with that claim scheme (ADR 0075, issue #1384).

use crate::eip712::{keccak256, recover_evm_signer, word_u64_be};
use crate::voucher_signature::BatchSettlementDomain;
use crate::Address;

const CLAIM_STATE_CHALLENGE_TYPE_HASH_PREIMAGE: &[u8] =
    b"ClaimStateChallenge(bytes32 channelId,uint256 expires)";

fn struct_hash(channel_id: &[u8; 32], expires: u64) -> [u8; 32] {
    let mut buf = Vec::with_capacity(32 * 3);
    buf.extend_from_slice(&keccak256(CLAIM_STATE_CHALLENGE_TYPE_HASH_PREIMAGE));
    buf.extend_from_slice(channel_id);
    buf.extend_from_slice(&word_u64_be(expires));
    keccak256(&buf)
}

/// The EIP-712 digest an x402 `batch-settlement` channel's voucher signer
/// signs to prove control of EVM channel `channel_id` for a claim-state read
/// (issue #1364): `ClaimStateChallenge(bytes32 channelId,uint256 expires)`
/// under `domain`, `x402BatchSettlement`'s own EIP-712 domain -- the
/// channel's recorded domain. `domain` is this connector's, never the
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

/// Domain tag prefixing every Solana batch-settlement claim-state challenge:
/// not starting with a voucher's `0x56 0x01` ([`crate::SOLANA_VOUCHER_PREFIX`]).
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

    fn generate_solana_keypair() -> ed25519_dalek::Keypair {
        ed25519_dalek::Keypair::generate(&mut OsRng)
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

    /// A voucher-channel challenge is bound to x402's domain on one chain:
    /// signed for one chain id, it does not verify for another.
    #[test]
    fn a_voucher_channel_challenge_is_bound_to_its_chain() {
        let (secret, address) = generate_evm_keypair();
        let digest =
            evm_voucher_claim_state_challenge_digest(&voucher_domain(), &[1u8; 32], 1_800_000_000);
        let signature = sign_as_a_wallet_would(&secret, &digest);
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

    /// A voucher does not verify as a voucher-channel challenge, and a
    /// voucher-channel challenge's message is not a voucher's.
    #[test]
    fn a_solana_voucher_channel_challenge_is_not_a_voucher() {
        let keypair = generate_solana_keypair();
        let public = keypair.public.to_bytes();
        let channel_account = [3u8; 32];
        let voucher = crate::solana_voucher_message(&channel_account, 1_800_000_000, 0);
        let voucher_signature = keypair.sign(&voucher).to_bytes();
        assert!(!verify_solana_voucher_claim_state_challenge(
            &channel_account,
            1_800_000_000,
            &voucher_signature,
            &public,
        ));
        let message = solana_voucher_claim_state_challenge_message(&channel_account, 1_800_000_000);
        assert!(!message.starts_with(&crate::SOLANA_VOUCHER_PREFIX));
    }
}
