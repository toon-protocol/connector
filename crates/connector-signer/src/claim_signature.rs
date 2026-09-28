//! Verifying a payment claim's own cryptographic signature -- for the
//! client edge, the last, deliberately most expensive stage of the claim
//! gate (`docs/protocol/client-edge-spec.md` §1.3 step 4, issue #506). It
//! was also the peer semantics' crypto check (issue #575, ADR 0024) until
//! ADR 0075 moved peers onto vouchers (issue #1380). A claim is signed by
//! the counterparty's own key, in that chain's native scheme, over a chain-specific balance proof this
//! module reconstructs independently -- distinct from [`crate::verify`],
//! the `Signer` contract suite's own "a signature recovers to its signer's
//! own public key" check, which no claim-verification path calls.
//!
//! Both directions below verify only whether a signature is cryptographically
//! valid for the given fields, against an explicitly supplied expected
//! signer/counterparty -- comparing against a claim's *self-declared* signer
//! field would let a forger simply lie about who signed. Malformed or
//! truncated key/signature bytes are a verification failure like any other
//! forgery, never a panic (client-edge-spec.md's "a claim with a corrupted
//! or truncated signature is refused as a validation failure, never as a
//! crash").
//!
//! ## EVM: EIP-712 `BalanceProof`
//!
//! The exact typed-data domain and struct
//! `packages/contracts/src/TokenNetwork.sol` verifies on chain
//! (`BALANCE_PROOF_TYPEHASH`, `EIP712("TokenNetwork", "1")`) and the legacy
//! TypeScript connector verified off-chain
//! (`eip712-helper.ts::getDomainSeparator`/`getBalanceProofTypes`, recovered
//! from git history per issue #498's "cheapest to recover now" -- deleted by
//! #465, commit c4a4ad10):
//!
//! ```text
//! domain     = EIP712Domain(name: "TokenNetwork", version: "1", chainId, verifyingContract)
//! structHash = keccak256(abi.encode(
//!                  keccak256("BalanceProof(bytes32 channelId,uint256 nonce,uint256 transferredAmount,uint256 lockedAmount,bytes32 locksRoot)"),
//!                  channelId, nonce, transferredAmount, lockedAmount, locksRoot))
//! digest     = keccak256(0x1901 || domainSeparator(domain) || structHash)
//! ```
//!
//! `lockedAmount`/`locksRoot` are always zero on the wire (ADR 0004) but are
//! still hashed -- omitting them would compute a different digest than the
//! one the signer's wallet actually signed.
//!
//! ## Solana: Ed25519 over a 96-byte balance-proof message
//!
//! ```text
//! message[0..16)  = SOLANA_BALANCE_PROOF_DOMAIN_TAG
//! message[16..48) = settlement program id, raw bytes
//! message[48..80) = channel account pubkey, raw bytes
//! message[80..88) = nonce,                 u64 little-endian
//! message[88..96) = transferred amount,    u64 little-endian
//! ```
//!
//! It was 48 bytes -- account, nonce, amount, the exact layout the legacy
//! TypeScript connector built and verified
//! (`solana-payment-channel-sdk.ts::_buildBalanceProofMessage`, recovered
//! the same way) -- until ADR 0053/issue #1082 put the program id inside
//! it. [`solana_balance_proof_message`] carries the full reasoning.

use libsecp256k1::{Message, RecoveryId, Signature as RawSignature};
use sha3::{Digest, Keccak256};

use crate::address::derive_evm_address;
use crate::signer::PublicKeyBytes;
use crate::Address;

/// The fields an EVM claim's EIP-712 `BalanceProof` signature covers
/// (`docs/protocol/client-edge-spec.md` §1.3's `evm` claim, minus the
/// `signature`/`signerAddress` themselves, which are checked against this).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvmBalanceProof {
    pub channel_id: [u8; 32],
    pub nonce: u64,
    pub transferred_amount: u128,
    /// Always zero on the wire today (ADR 0004) but still part of the
    /// signed struct -- omitting it would not reproduce the signer's digest.
    pub locked_amount: u128,
    pub locks_root: [u8; 32],
    pub chain_id: u64,
    /// The EIP-712 domain's `verifyingContract` -- the claim's optional
    /// `tokenNetworkAddress` field (client-edge-spec.md §1.3).
    pub token_network_address: Address,
}

const BALANCE_PROOF_TYPE_HASH_PREIMAGE: &[u8] =
    b"BalanceProof(bytes32 channelId,uint256 nonce,uint256 transferredAmount,uint256 lockedAmount,bytes32 locksRoot)";

const EIP712_DOMAIN_TYPE_HASH_PREIMAGE: &[u8] =
    b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)";

pub(crate) fn keccak256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

/// A `u128` value as a 32-byte, big-endian ABI word (a Solidity `uint256`
/// wide enough for a real claim amount, left-zero-padded).
fn word_u128_be(value: u128) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[16..].copy_from_slice(&value.to_be_bytes());
    word
}

/// A `u64` nonce (or other small integer) as a 32-byte, big-endian ABI word
/// (a Solidity `uint256`).
pub(crate) fn word_u64_be(value: u64) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[24..].copy_from_slice(&value.to_be_bytes());
    word
}

/// A 20-byte EVM address as a 32-byte ABI word (right-aligned, per
/// Solidity's `address` encoding).
pub(crate) fn word_address(address: &Address) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[12..].copy_from_slice(address);
    word
}

/// The `TokenNetwork` EIP-712 domain separator (`EIP712("TokenNetwork",
/// "1")`), shared by every typed struct this connector signs or verifies
/// under that domain -- a real claim's `BalanceProof` and, distinctly, a
/// claim-state challenge's own struct (`crate::claim_state_challenge`).
/// Reused rather than duplicated so both compute the identical domain a
/// wallet signing either one actually sees.
pub(crate) fn domain_separator(chain_id: u64, verifying_contract: &Address) -> [u8; 32] {
    let mut buf = Vec::with_capacity(32 * 5);
    buf.extend_from_slice(&keccak256(EIP712_DOMAIN_TYPE_HASH_PREIMAGE));
    buf.extend_from_slice(&keccak256(b"TokenNetwork"));
    buf.extend_from_slice(&keccak256(b"1"));
    buf.extend_from_slice(&word_u64_be(chain_id));
    buf.extend_from_slice(&word_address(verifying_contract));
    keccak256(&buf)
}

fn struct_hash(proof: &EvmBalanceProof) -> [u8; 32] {
    let mut buf = Vec::with_capacity(32 * 6);
    buf.extend_from_slice(&keccak256(BALANCE_PROOF_TYPE_HASH_PREIMAGE));
    buf.extend_from_slice(&proof.channel_id);
    buf.extend_from_slice(&word_u64_be(proof.nonce));
    buf.extend_from_slice(&word_u128_be(proof.transferred_amount));
    buf.extend_from_slice(&word_u128_be(proof.locked_amount));
    buf.extend_from_slice(&proof.locks_root);
    keccak256(&buf)
}

/// The EIP-712 digest `proof`'s signer actually signed -- exposed so a test
/// (or a future signing path) can produce a genuine signature over exactly
/// what [`verify_evm_balance_proof`] checks.
pub fn evm_balance_proof_digest(proof: &EvmBalanceProof) -> [u8; 32] {
    let mut buf = Vec::with_capacity(2 + 32 + 32);
    buf.extend_from_slice(&[0x19, 0x01]);
    buf.extend_from_slice(&domain_separator(
        proof.chain_id,
        &proof.token_network_address,
    ));
    buf.extend_from_slice(&struct_hash(proof));
    keccak256(&buf)
}

/// Recover the EVM address that produced `signature` over `digest`, or
/// `None` for anything that isn't a well-formed 65-byte
/// `r || s || v` Ethereum signature (`v` of `0`/`1` or the conventional
/// `27`/`28`) that recovers cleanly. Never panics on attacker-controlled
/// bytes -- a truncated or corrupted signature simply fails to recover,
/// exactly like one that recovers to the wrong party.
pub(crate) fn recover_evm_signer(digest: &[u8; 32], signature: &[u8]) -> Option<Address> {
    if signature.len() != 65 {
        return None;
    }
    let mut rs = [0u8; 64];
    rs.copy_from_slice(&signature[..64]);
    let raw_signature = RawSignature::parse_standard(&rs).ok()?;
    let v = signature[64];
    let recovery_byte = if v >= 27 { v - 27 } else { v };
    let recovery_id = RecoveryId::parse(recovery_byte).ok()?;
    let message = Message::parse(digest);
    let recovered_key = libsecp256k1::recover(&message, &raw_signature, &recovery_id).ok()?;
    let public_key_bytes: PublicKeyBytes = recovered_key.serialize();
    Some(derive_evm_address(&public_key_bytes))
}

/// Whether `signature` is a valid EIP-712 `BalanceProof` signature over
/// `proof`, produced by `expected_counterparty` -- the channel's registered
/// counterparty, never the claim's own self-declared `signerAddress` field,
/// since a forger can declare anything (client-edge-spec.md §1.3 step 4:
/// "recovers to the channel's counterparty").
pub fn verify_evm_balance_proof(
    proof: &EvmBalanceProof,
    signature: &[u8],
    expected_counterparty: &Address,
) -> bool {
    let digest = evm_balance_proof_digest(proof);
    recover_evm_signer(&digest, signature).as_ref() == Some(expected_counterparty)
}

/// The domain tag every Solana balance-proof message begins with. Sixteen
/// bytes exactly, so the layout below is fixed-size arithmetic.
///
/// Distinct from [`SOLANA_CHALLENGE_DOMAIN_TAG`](crate::claim_state_challenge)
/// in both length and content, so a claim-state challenge and a balance proof
/// can never be mistaken for one another whatever their fields.
pub const SOLANA_BALANCE_PROOF_DOMAIN_TAG: &[u8; 16] = b"TOON-BALPROOF-V2";

/// The 96-byte message a Solana claim's Ed25519 signature covers
/// (ADR 0053, issue #1082).
///
/// | bytes | field |
/// | --- | --- |
/// | 0..16 | [`SOLANA_BALANCE_PROOF_DOMAIN_TAG`] |
/// | 16..48 | `program_id` — the settlement program this channel lives under |
/// | 48..80 | `channel_account` |
/// | 80..88 | `nonce`, u64 little-endian |
/// | 88..96 | `transferred_amount`, u64 little-endian |
///
/// # Why the program id is in here
///
/// Before ADR 0053 this message was 48 bytes -- channel account, nonce,
/// amount -- and bound **nothing about which chain the channel lives on**.
/// Its EVM counterpart binds `chain_id` and `token_network_address` through
/// the EIP-712 domain separator, which is the whole point of ADR 0024; the
/// Solana side had no equivalent. A signature was therefore valid for its
/// channel account on **any** cluster where that account existed, and the
/// separation we had came from program ids happening to differ between
/// deployments -- a deployment accident standing in for a cryptographic
/// guarantee.
///
/// The program id is the binding a program can actually enforce. On chain a
/// program knows its own id and nothing about which cluster it runs on, so a
/// cluster string here would be unverifiable: the program could not rebuild
/// the message to compare against. A claim's declared `cluster` therefore
/// stays what it always was -- **a routing hint, never a security boundary**
/// -- and is compared off chain against the cluster the node's own
/// `[settlement.solana] rpc_url` names
/// (`connector_client_edge::ClientClaimGate`, issue #975), which closes that
/// issue's honest-misconfiguration case. The forgery case is closed here, by
/// the bytes.
///
/// Note that when this record first shipped that comparison did not yet
/// exist: the sentence above described the intended end state and the check
/// landed later, with #976. Nothing in this module ever depended on it --
/// the bytes below are the security property, and the cluster comparison is
/// about the *record* a claim leaves behind.
///
/// # Why a domain tag, and not an appended field
///
/// ADR 0053 asks for a domain-separated construction rather than an
/// append. An appended field is silently truncatable by a verifier that
/// expects the old length: a 48-byte prefix of this message is not a valid
/// message under either scheme, and a verifier written against the old
/// layout rejects it outright rather than reading a shorter one as complete.
pub fn solana_balance_proof_message(
    program_id: &[u8; 32],
    channel_account: &[u8; 32],
    nonce: u64,
    transferred_amount: u64,
) -> [u8; 96] {
    let mut message = [0u8; 96];
    message[0..16].copy_from_slice(SOLANA_BALANCE_PROOF_DOMAIN_TAG);
    message[16..48].copy_from_slice(program_id);
    message[48..80].copy_from_slice(channel_account);
    message[80..88].copy_from_slice(&nonce.to_le_bytes());
    message[88..96].copy_from_slice(&transferred_amount.to_le_bytes());
    message
}

/// Whether `signature` is a valid Ed25519 signature, by
/// `signer_public_key`, over the balance-proof message for
/// `channel_account`/`nonce`/`transferred_amount`
/// (client-edge-spec.md §1.3 step 4: "A Solana claim's signature is
/// verified against the declared signer key"). `signer_public_key` must
/// already be the channel's registered counterparty key for this to mean
/// anything -- this function only checks the signature is genuine for
/// whatever key it is given.
pub fn verify_solana_balance_proof(
    program_id: &[u8; 32],
    channel_account: &[u8; 32],
    nonce: u64,
    transferred_amount: u64,
    signature: &[u8],
    signer_public_key: &[u8; 32],
) -> bool {
    let Ok(public_key) = ed25519_dalek::PublicKey::from_bytes(signer_public_key) else {
        return false;
    };
    let Ok(signature) = ed25519_dalek::Signature::from_bytes(signature) else {
        return false;
    };
    let message =
        solana_balance_proof_message(program_id, channel_account, nonce, transferred_amount);
    use ed25519_dalek::Verifier;
    public_key.verify(&message, &signature).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Signer as Ed25519Signer;
    use libsecp256k1::{PublicKey, SecretKey};
    use rand::rngs::OsRng;

    fn sample_proof() -> EvmBalanceProof {
        EvmBalanceProof {
            channel_id: [1u8; 32],
            nonce: 5,
            transferred_amount: 1_000,
            locked_amount: 0,
            locks_root: [0u8; 32],
            chain_id: 8453,
            token_network_address: [0x42; 20],
        }
    }

    /// Sign `digest` exactly the way a real EVM wallet would (a 65-byte
    /// `r || s || v` signature, `v` in the conventional `{27, 28}` range) --
    /// deliberately not reusing `crate::Signature`'s own encoding, which is
    /// this connector's internal representation for a different claim
    /// scheme entirely.
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
    fn a_genuine_evm_signature_verifies_against_its_signers_address() {
        let (secret, address) = generate_evm_keypair();
        let proof = sample_proof();
        let signature = sign_as_a_wallet_would(&secret, &evm_balance_proof_digest(&proof));

        assert!(verify_evm_balance_proof(&proof, &signature, &address));
    }

    #[test]
    fn an_evm_signature_does_not_verify_against_a_different_partys_address() {
        let (secret, _address) = generate_evm_keypair();
        let (_other_secret, other_address) = generate_evm_keypair();
        let proof = sample_proof();
        let signature = sign_as_a_wallet_would(&secret, &evm_balance_proof_digest(&proof));

        assert!(!verify_evm_balance_proof(
            &proof,
            &signature,
            &other_address
        ));
    }

    #[test]
    fn a_truncated_evm_signature_fails_to_verify_rather_than_panicking() {
        let (secret, address) = generate_evm_keypair();
        let proof = sample_proof();
        let mut signature = sign_as_a_wallet_would(&secret, &evm_balance_proof_digest(&proof));
        signature.truncate(10);

        assert!(!verify_evm_balance_proof(&proof, &signature, &address));
    }

    #[test]
    fn a_corrupted_evm_signature_fails_to_verify_rather_than_panicking() {
        let (secret, address) = generate_evm_keypair();
        let proof = sample_proof();
        let mut signature = sign_as_a_wallet_would(&secret, &evm_balance_proof_digest(&proof));
        signature[0] ^= 0xff;
        signature[32] ^= 0xff;

        assert!(!verify_evm_balance_proof(&proof, &signature, &address));
    }

    #[test]
    fn an_empty_evm_signature_fails_to_verify_rather_than_panicking() {
        let (_secret, address) = generate_evm_keypair();
        let proof = sample_proof();

        assert!(!verify_evm_balance_proof(&proof, &[], &address));
    }

    #[test]
    fn changing_any_evm_proof_field_invalidates_a_prior_signature() {
        let (secret, address) = generate_evm_keypair();
        let proof = sample_proof();
        let signature = sign_as_a_wallet_would(&secret, &evm_balance_proof_digest(&proof));

        let tampered_channel = EvmBalanceProof {
            channel_id: [2u8; 32],
            ..proof
        };
        let tampered_nonce = EvmBalanceProof {
            nonce: proof.nonce + 1,
            ..proof
        };
        let tampered_amount = EvmBalanceProof {
            transferred_amount: proof.transferred_amount + 1,
            ..proof
        };
        let tampered_chain = EvmBalanceProof {
            chain_id: proof.chain_id + 1,
            ..proof
        };
        let tampered_contract = EvmBalanceProof {
            token_network_address: [0x99; 20],
            ..proof
        };

        for tampered in [
            tampered_channel,
            tampered_nonce,
            tampered_amount,
            tampered_chain,
            tampered_contract,
        ] {
            assert!(!verify_evm_balance_proof(&tampered, &signature, &address));
        }
    }

    #[test]
    fn the_evm_digest_is_deterministic() {
        let proof = sample_proof();
        assert_eq!(
            evm_balance_proof_digest(&proof),
            evm_balance_proof_digest(&proof)
        );
    }

    fn generate_solana_keypair() -> ed25519_dalek::Keypair {
        ed25519_dalek::Keypair::generate(&mut OsRng)
    }

    /// A fixed settlement-program id for the Solana cases below. Its value
    /// carries no meaning; what matters is that it is part of the signed
    /// message, so a signature made under it does not verify under another
    /// (ADR 0053, issue #1082).
    const TEST_PROGRAM_ID: [u8; 32] = [9u8; 32];

    /// ADR 0053, issue #1082: the property the whole change exists for.
    ///
    /// Before it, a Solana claim's signature covered the channel account, the
    /// nonce and the amount, and **nothing about which chain the channel
    /// lived on** -- so a signature was valid for its account on any cluster
    /// where that account existed. The separation we had came from program
    /// ids happening to differ between deployments, which is a deployment
    /// accident rather than a cryptographic guarantee.
    #[test]
    fn a_solana_signature_does_not_verify_under_a_different_program() {
        let keypair = generate_solana_keypair();
        let channel_account = [3u8; 32];
        let other_program = [0xAAu8; 32];
        assert_ne!(TEST_PROGRAM_ID, other_program);

        let message = solana_balance_proof_message(&TEST_PROGRAM_ID, &channel_account, 7, 500);
        let signature = keypair.sign(&message);
        let public_key = keypair.public.to_bytes();

        assert!(verify_solana_balance_proof(
            &TEST_PROGRAM_ID,
            &channel_account,
            7,
            500,
            &signature.to_bytes(),
            &public_key,
        ));
        assert!(
            !verify_solana_balance_proof(
                &other_program,
                &channel_account,
                7,
                500,
                &signature.to_bytes(),
                &public_key,
            ),
            "a claim signed against one settlement program must not verify \
             against another -- that is the cross-cluster replay ADR 0053 closes"
        );
    }

    /// The EVM counterpart of this property is
    /// `changing_any_evm_proof_field_invalidates_a_prior_signature`. This is
    /// the Solana one, and it now covers the program id as well.
    #[test]
    fn changing_any_solana_proof_field_invalidates_a_prior_signature() {
        let keypair = generate_solana_keypair();
        let public_key = keypair.public.to_bytes();
        let account = [3u8; 32];
        let signature = keypair
            .sign(&solana_balance_proof_message(
                &TEST_PROGRAM_ID,
                &account,
                7,
                500,
            ))
            .to_bytes();

        for (label, program, acct, nonce, amount) in [
            ("program id", [0xAAu8; 32], account, 7u64, 500u64),
            ("channel account", TEST_PROGRAM_ID, [4u8; 32], 7, 500),
            ("nonce", TEST_PROGRAM_ID, account, 8, 500),
            ("transferred amount", TEST_PROGRAM_ID, account, 7, 501),
        ] {
            assert!(
                !verify_solana_balance_proof(
                    &program,
                    &acct,
                    nonce,
                    amount,
                    &signature,
                    &public_key
                ),
                "changing the {label} must invalidate a prior signature"
            );
        }
    }

    /// The domain tag is what makes the old 48-byte layout unreadable as a
    /// prefix of this one: a verifier written against it sees a length
    /// mismatch rather than a truncated message (ADR 0053's "domain-separated
    /// construction, not an append").
    #[test]
    fn the_message_is_96_bytes_and_begins_with_its_domain_tag() {
        let message = solana_balance_proof_message(&TEST_PROGRAM_ID, &[3u8; 32], 7, 500);
        assert_eq!(message.len(), 96);
        assert_eq!(&message[0..16], SOLANA_BALANCE_PROOF_DOMAIN_TAG);
        assert_eq!(&message[16..48], &TEST_PROGRAM_ID);
    }

    #[test]
    fn a_genuine_solana_signature_verifies_against_its_signers_key() {
        let keypair = generate_solana_keypair();
        let channel_account = [3u8; 32];
        let message = solana_balance_proof_message(&TEST_PROGRAM_ID, &channel_account, 7, 500);
        let signature = keypair.sign(&message);

        assert!(verify_solana_balance_proof(
            &TEST_PROGRAM_ID,
            &channel_account,
            7,
            500,
            &signature.to_bytes(),
            &keypair.public.to_bytes(),
        ));
    }

    #[test]
    fn a_solana_signature_does_not_verify_against_a_different_partys_key() {
        let keypair = generate_solana_keypair();
        let other_keypair = generate_solana_keypair();
        let channel_account = [3u8; 32];
        let message = solana_balance_proof_message(&TEST_PROGRAM_ID, &channel_account, 7, 500);
        let signature = keypair.sign(&message);

        assert!(!verify_solana_balance_proof(
            &TEST_PROGRAM_ID,
            &channel_account,
            7,
            500,
            &signature.to_bytes(),
            &other_keypair.public.to_bytes(),
        ));
    }

    #[test]
    fn a_truncated_solana_signature_fails_to_verify_rather_than_panicking() {
        let keypair = generate_solana_keypair();
        let channel_account = [3u8; 32];
        let message = solana_balance_proof_message(&TEST_PROGRAM_ID, &channel_account, 7, 500);
        let signature = keypair.sign(&message);
        let truncated = &signature.to_bytes()[..10];

        assert!(!verify_solana_balance_proof(
            &TEST_PROGRAM_ID,
            &channel_account,
            7,
            500,
            truncated,
            &keypair.public.to_bytes(),
        ));
    }

    #[test]
    fn a_corrupted_solana_signature_fails_to_verify_rather_than_panicking() {
        let keypair = generate_solana_keypair();
        let channel_account = [3u8; 32];
        let message = solana_balance_proof_message(&TEST_PROGRAM_ID, &channel_account, 7, 500);
        let signature = keypair.sign(&message);
        let mut corrupted = signature.to_bytes();
        corrupted[0] ^= 0xff;

        assert!(!verify_solana_balance_proof(
            &TEST_PROGRAM_ID,
            &channel_account,
            7,
            500,
            &corrupted,
            &keypair.public.to_bytes(),
        ));
    }

    #[test]
    fn a_malformed_solana_public_key_fails_to_verify_rather_than_panicking() {
        let keypair = generate_solana_keypair();
        let channel_account = [3u8; 32];
        let message = solana_balance_proof_message(&TEST_PROGRAM_ID, &channel_account, 7, 500);
        let signature = keypair.sign(&message);

        // An all-zero "public key" is not a valid Ed25519 point.
        let malformed_key = [0u8; 32];
        assert!(!verify_solana_balance_proof(
            &TEST_PROGRAM_ID,
            &channel_account,
            7,
            500,
            &signature.to_bytes(),
            &malformed_key,
        ));
    }

    #[test]
    fn changing_any_solana_field_invalidates_a_prior_signature() {
        let keypair = generate_solana_keypair();
        let channel_account = [3u8; 32];
        let message = solana_balance_proof_message(&TEST_PROGRAM_ID, &channel_account, 7, 500);
        let signature = keypair.sign(&message);
        let public_key = keypair.public.to_bytes();

        assert!(!verify_solana_balance_proof(
            &TEST_PROGRAM_ID,
            &[9u8; 32],
            7,
            500,
            &signature.to_bytes(),
            &public_key,
        ));
        assert!(!verify_solana_balance_proof(
            &TEST_PROGRAM_ID,
            &channel_account,
            8,
            500,
            &signature.to_bytes(),
            &public_key,
        ));
        assert!(!verify_solana_balance_proof(
            &TEST_PROGRAM_ID,
            &channel_account,
            7,
            501,
            &signature.to_bytes(),
            &public_key,
        ));
    }
}
