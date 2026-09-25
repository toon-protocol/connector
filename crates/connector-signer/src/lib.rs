//! Signing (ADR 0001; ADR 0012, as superseded by issue #556 -- see that
//! ADR's own superseding note).
//!
//! This crate is the connector's sole owner of key handling: the [`Signer`]
//! port with its [`LocalSigner`] and [`KmsSigner`] implementations. No other
//! crate in this workspace holds key material or performs a signing
//! operation directly -- anything that needs to sign a claim or a
//! settlement transaction takes a `&dyn Signer`. It is also where a claim's
//! signature is checked -- both a peer claim (issue #575, ADR 0024)
//! and a client edge claim's chain-native wallet signature (issue #506) go
//! through [`verify_evm_balance_proof`]/[`verify_solana_balance_proof`],
//! neither needing key material of its own, only the public key or address
//! a channel's counterparty is already known by. An x402 `batch-settlement`
//! voucher (ADR 0074, issue #1341) is verified here too, through its own
//! [`verify_evm_voucher`]/[`verify_solana_voucher`] and [`VoucherSignature`]
//! -- a second claim scheme, never a variant of the first. [`verify`] is unrelated to
//! either: it is the `Signer` contract suite's own "a signature recovers to
//! its signer's own public key" check (`src/contract.rs`).
//!
//! Deliberately absent, per ADR 0012: mnemonic recovery, seed management,
//! human wallet authentication, a wallet database, and any fraud or
//! anomaly rule engine. Those invariants are enforced elsewhere, by
//! watermarks and signature verification on the packet plane.
//!
//! Also deliberately absent as of ADR 0046 (issue #1074): NIP-01 event
//! signing. This crate held one function for it, `sign_ilp_peer_info`, and one
//! event kind, `ILP_PEER_INFO_KIND` (10032) -- the connector's own announce.
//! A connector answers when asked and never announces, so there is no event
//! for it to author, and the BIP-340 Schnorr machinery that signed one is gone
//! with it. `nip59` remains: wrapping a claim to a receiver is not announcing.
//!
//! ADR 0012 also named a treasury component (`Treasury`/`ChainClient`) that
//! spends and reports a balance through a `Signer`. It never had a caller
//! outside its own `#[cfg(test)]` module on any running node -- the real,
//! wired collateral path is `connector-settlement`'s `SettlementBackend`
//! (`fund`/`redeem`/`channel_state`, constructed in `connector-cli::runtime`
//! and integration-tested against a real chain). Issue #556 removed it, on
//! ADR 0033's own precedent that a component whose job is already done
//! elsewhere is removed rather than restated.

mod address;
mod claim_signature;
mod claim_state_challenge;
mod crypto;
mod ed25519_signer;
mod error;
pub mod giftwrap;
mod kms;
mod local;
pub mod nip59;
mod signer;
mod voucher_signature;

pub use address::{derive_evm_address, to_hex, Address};
pub use claim_signature::{
    evm_balance_proof_digest, solana_balance_proof_message, verify_evm_balance_proof,
    verify_solana_balance_proof, EvmBalanceProof,
};
pub use claim_state_challenge::{
    evm_claim_state_challenge_digest, solana_claim_state_challenge_message,
    verify_evm_claim_state_challenge, verify_solana_claim_state_challenge, EvmClaimStateChallenge,
};
pub use ed25519_signer::{Ed25519Signer, LocalEd25519Signer};
pub use error::SignerError;
pub use giftwrap::GiftWrapError;
pub use kms::{InMemoryKmsBackend, KmsBackend, KmsSigner};
pub use local::LocalSigner;
pub use nip59::{unwrap_claim, wrap_claim, Nip59Error, WrappedClaim};
pub use signer::{verify, PublicKeyBytes, Signature, Signer};
pub use voucher_signature::{
    evm_batch_channel_id, evm_voucher_digest, evm_voucher_signer, solana_voucher_message,
    verify_evm_voucher, verify_solana_voucher, BatchChannelConfig, BatchSettlementDomain,
    VoucherSignature, SOLANA_VOUCHER_PREFIX, X402_BATCH_SETTLEMENT_ADDRESS,
    X402_BATCH_SETTLEMENT_EIP712_NAME, X402_BATCH_SETTLEMENT_EIP712_VERSION,
};

#[cfg(test)]
mod contract;
