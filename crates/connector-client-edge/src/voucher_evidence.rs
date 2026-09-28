//! **The receiving half, as the peer-role gate asks it** (ADR 0075 decision
//! 5, issue #1377): [`ClientClaimGate`] resolving the x402 channel a voucher
//! or a peer-role challenge names, reading its voucher signer from what the
//! batch-settlement backend admitted, and checking the signature -- and
//! doing nothing else.
//!
//! # Why the claim gate
//!
//! It is the one component that holds both things a voucher's signer is
//! resolved from: the batch-settlement backends (ADR 0074 decision 9's
//! seam, [`crate::BatchSettlementChannels`]) and the journaled
//! `ChannelConfig` an EVM voucher that carries none is resolved from. The
//! lookups are the very ones `POST /ilp/claim-state` makes for a
//! `batch-settlement` entry ([`ClientClaimGate::evm_voucher_channel`],
//! [`ClientClaimGate::solana_voucher_channel`]), so a channel is found for
//! the role exactly as it is found for a read, and metered the same way
//! against the unresolvable-lookup budget (issue #613).
//!
//! # What it never does
//!
//! Admit, advance or journal. Role is fixed before any watermark moves
//! (`peer-carriage-spec.md` §1.5), so a voucher that proves the peer role
//! here has not been accepted by anything; judging it is downstream of the
//! role (#1378).

use async_trait::async_trait;
use connector_domain::client_claim::ClientClaim;
use connector_peer_btp::challenge_json::PeerRoleChallenge;
use connector_peer_btp::role_gate::{VoucherCheck, VoucherEvidence};
use connector_runtime::VoucherSigner;
use connector_signer::{
    evm_voucher_signer, verify_evm_voucher, verify_evm_voucher_claim_state_challenge,
    verify_solana_voucher, verify_solana_voucher_claim_state_challenge, BatchChannelConfig,
};

use crate::channels::{decode_base58_bytes, decode_hex_bytes};
use crate::claim_gate::decode_evm_channel_config;
use crate::ClientClaimGate;

/// What an EVM channel's signature is checked over: a voucher's cumulative
/// amount, or a challenge's expiry. Two messages under two typehashes, so a
/// signature over one never verifies as the other.
enum EvmMessage {
    Voucher { max_claimable_amount: u64 },
    Challenge { expires: u64 },
}

/// As [`EvmMessage`], on Solana.
enum SolanaMessage {
    Voucher { cumulative: u64 },
    Challenge { expires: u64 },
}

impl ClientClaimGate {
    /// Resolve EVM channel `channel_id` and check `signature` over `message`
    /// against its chain-recorded voucher signer.
    async fn check_evm(
        &self,
        channel_id: [u8; 32],
        presented: Option<BatchChannelConfig>,
        message: EvmMessage,
        signature: &[u8; 65],
        requester: &str,
    ) -> VoucherCheck {
        let Ok(Some((domain, channel))) = self
            .evm_voucher_channel(&channel_id, presented, requester)
            .await
        else {
            return VoucherCheck::Unresolved;
        };
        let signer = evm_voucher_signer(&channel.config);
        let verified = match message {
            EvmMessage::Voucher {
                max_claimable_amount,
            } => verify_evm_voucher(
                &domain,
                &channel_id,
                u128::from(max_claimable_amount),
                signature,
                &signer,
            ),
            EvmMessage::Challenge { expires } => verify_evm_voucher_claim_state_challenge(
                &domain,
                &channel_id,
                expires,
                signature,
                &signer,
            ),
        };
        let signer = VoucherSigner::Evm(signer);
        if verified {
            VoucherCheck::Verified(signer)
        } else {
            VoucherCheck::SignatureInvalid(signer)
        }
    }

    /// Resolve Solana channel account `channel_account` and check
    /// `signature` over `message` against its `authorized_signer`.
    async fn check_solana(
        &self,
        channel_account: [u8; 32],
        message: SolanaMessage,
        signature: &[u8; 64],
        requester: &str,
    ) -> VoucherCheck {
        let Ok(Some(channel)) = self
            .solana_voucher_channel(&channel_account, requester)
            .await
        else {
            return VoucherCheck::Unresolved;
        };
        let signer = channel.authorized_signer;
        let verified = match message {
            // `expiresAt` is zero: the parser refused anything else (ADR
            // 0074 decision 3), and it is still part of the signed bytes.
            SolanaMessage::Voucher { cumulative } => {
                verify_solana_voucher(&channel_account, cumulative, 0, signature, &signer)
            }
            SolanaMessage::Challenge { expires } => verify_solana_voucher_claim_state_challenge(
                &channel_account,
                expires,
                signature,
                &signer,
            ),
        };
        let signer = VoucherSigner::Solana(signer);
        if verified {
            VoucherCheck::Verified(signer)
        } else {
            VoucherCheck::SignatureInvalid(signer)
        }
    }
}

#[async_trait]
impl VoucherEvidence for ClientClaimGate {
    async fn check_voucher(&self, voucher: &ClientClaim) -> VoucherCheck {
        match voucher {
            ClientClaim::EvmVoucher(voucher) => {
                let (Some(channel_id), Some(signature)) = (
                    decode_hex_bytes::<32>(&voucher.channel_id),
                    decode_hex_bytes::<65>(&voucher.signature),
                ) else {
                    return VoucherCheck::Unresolved;
                };
                let presented = match voucher
                    .channel_config
                    .as_ref()
                    .map(decode_evm_channel_config)
                    .transpose()
                {
                    Ok(presented) => presented,
                    Err(_) => return VoucherCheck::Unresolved,
                };
                self.check_evm(
                    channel_id,
                    presented,
                    EvmMessage::Voucher {
                        max_claimable_amount: voucher.max_claimable_amount,
                    },
                    &signature,
                    &format!("peer-role-voucher:{}", voucher.signature),
                )
                .await
            }
            ClientClaim::SolanaVoucher(voucher) => {
                let (Some(channel_account), Some(signature)) = (
                    decode_base58_bytes::<32>(&voucher.channel_id),
                    decode_base58_bytes::<64>(&voucher.signature),
                ) else {
                    return VoucherCheck::Unresolved;
                };
                self.check_solana(
                    channel_account,
                    SolanaMessage::Voucher {
                        cumulative: voucher.max_claimable_amount,
                    },
                    &signature,
                    &format!("peer-role-voucher:{}", voucher.signature),
                )
                .await
            }
            // A `toon-channel` claim is `ClaimBook`'s to verify, never this.
            ClientClaim::Evm(_) | ClientClaim::Solana(_) => VoucherCheck::Unresolved,
        }
    }

    async fn check_challenge(&self, challenge: &PeerRoleChallenge) -> VoucherCheck {
        match challenge {
            PeerRoleChallenge::Evm {
                channel_id,
                expires,
                signature,
                channel_config,
            } => {
                let presented = match channel_config
                    .as_ref()
                    .map(decode_evm_channel_config)
                    .transpose()
                {
                    Ok(presented) => presented,
                    Err(_) => return VoucherCheck::Unresolved,
                };
                self.check_evm(
                    *channel_id,
                    presented,
                    EvmMessage::Challenge { expires: *expires },
                    signature,
                    &format!("peer-role-challenge:0x{}", hex::encode(signature)),
                )
                .await
            }
            PeerRoleChallenge::Solana {
                channel_account,
                expires,
                signature,
            } => {
                self.check_solana(
                    *channel_account,
                    SolanaMessage::Challenge { expires: *expires },
                    signature,
                    &format!("peer-role-challenge:{}", hex::encode(signature)),
                )
                .await
            }
        }
    }
}
