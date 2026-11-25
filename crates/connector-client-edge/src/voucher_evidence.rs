//! **The receiving half, as the peer-role gate asks it** (ADR 0075 decision
//! 5, issue #1377): [`ClientClaimGate`] resolving the x402 channel a voucher
//! or a peer-role challenge names, reading its voucher signer from what the
//! batch-settlement backend admitted, and checking the signature -- and
//! doing nothing else.
//!
//! # Why the claim gate
//!
//! It is the one component that holds both things a voucher's signer is
//! resolved from: the node's settlement networks (spec #1684, one
//! collection keyed by CAIP-2 network) and the journaled
//! `ChannelConfig` an EVM voucher that carries none is resolved from. A
//! channel is found and its signature verified by the gate's one "find the
//! channel, then verify" function (spec #1685, issue #1716) -- the very one
//! `POST /ilp/claim-state` and voucher admission call -- so a channel is
//! found for the role exactly as it is found for a read, and metered the
//! same way against the unresolvable-lookup budget (issue #613).
//!
//! # What the role check never does
//!
//! Admit, advance or journal. Role is fixed before any watermark moves
//! (`peer-carriage-spec.md` §1.5), so a voucher that proves the peer role
//! here has not been accepted by anything.
//!
//! # Judging a peer's voucher, downstream of the role (#1378)
//!
//! [`VoucherEvidence::judge_peer_voucher`] is the one place that does admit
//! one: through this gate's own voucher admission, with no price of its own
//! (the peer carriages measure coverage themselves, `price_gate`), so a
//! peer's voucher is held to exactly the rules, the watermark and the
//! journal a client's is. That is `peer-carriage-spec.md` §1.8's
//! requirement: a voucher channel keeps **one** watermark, whichever role
//! its vouchers arrive under -- a peer bound after it paid as a client
//! continues from where the channel stands, never from zero.

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use connector_domain::client_claim::ClientClaim;
use connector_domain::Network;
use connector_peer_btp::challenge_json::PeerRoleChallenge;
use connector_peer_btp::role_gate::{PeerVoucherVerdict, VoucherCheck, VoucherEvidence};
use connector_runtime::{ClaimAckOutcome, ClaimRejectReason};
use connector_signer::EvmVoucherScheme;

use crate::claim_gate::Proof;
use crate::{ClaimIngestRejection, ClientClaimGate};

/// A refused peer voucher's verdict, in the `claim-ack`'s one refusal
/// taxonomy (`peer-carriage-spec.md` §6.1). A refusal that is this node's
/// own and temporary -- the journal could not be written, the chain could
/// not be asked -- is **not acknowledged** rather than refused, so the
/// payer's voucher stays pending and its resend is judged afresh (§6.3).
fn voucher_ack(rejection: &ClaimIngestRejection) -> ClaimAckOutcome {
    match rejection {
        ClaimIngestRejection::NotDurable
        | ClaimIngestRejection::ChannelLookupFailed(_)
        | ClaimIngestRejection::LookupBudgetExhausted { .. } => ClaimAckOutcome::NotSent,
        ClaimIngestRejection::SignatureInvalid => {
            ClaimAckOutcome::Rejected(ClaimRejectReason::SignatureInvalid)
        }
        ClaimIngestRejection::AmountNotAdvancing { .. }
        | ClaimIngestRejection::Underpayment { .. }
        | ClaimIngestRejection::Undercollateralized { .. } => {
            ClaimAckOutcome::Rejected(ClaimRejectReason::AmountNotAdvancing)
        }
        // Every other refusal is about the channel or the voucher's shape:
        // there is no channel this node will take it on. Listed rather than
        // caught by a wildcard, so a new refusal has to be placed here.
        ClaimIngestRejection::Malformed(_)
        | ClaimIngestRejection::Mina
        | ClaimIngestRejection::ToonChannel
        | ClaimIngestRejection::UnknownChannel
        | ClaimIngestRejection::ChannelTerminal(_)
        | ClaimIngestRejection::WrapUnsupported
        | ClaimIngestRejection::WrapFailed(_)
        | ClaimIngestRejection::BatchSettlementNotAccepted
        | ClaimIngestRejection::VoucherChannelConfigMismatch { .. }
        | ClaimIngestRejection::NetworkMissing
        | ClaimIngestRejection::NetworkNotSettled { .. }
        | ClaimIngestRejection::UnknownAllowance
        | ClaimIngestRejection::AllowanceExpired
        | ClaimIngestRejection::AllowanceClosed
        | ClaimIngestRejection::OverBudget { .. } => {
            ClaimAckOutcome::Rejected(ClaimRejectReason::UnknownChannel)
        }
    }
}

/// Whose lookup a peer-role check is metered as, against the
/// unresolvable-lookup budget (issue #613): the signature as it rode the
/// wire, under one prefix, as `POST /ilp/claim-state` keys its own
/// challenge lookups.
fn requester(wire_signature: &str) -> String {
    format!("peer-role:{wire_signature}")
}

impl ClientClaimGate {
    /// Find the channel `proof` names on `network` and check its signature
    /// against the chain-recorded voucher signer, as the role gate reads it.
    async fn check(&self, network: &Network, proof: &Proof, requester: &str) -> VoucherCheck {
        match self.find_then_verify(network, proof, requester).await {
            Ok(found) if found.verified => VoucherCheck::Verified(found.signer),
            Ok(found) => VoucherCheck::SignatureInvalid(found.signer),
            Err(_) => VoucherCheck::Unresolved,
        }
    }
}

#[async_trait]
impl VoucherEvidence for ClientClaimGate {
    async fn check_voucher(&self, voucher: &ClientClaim) -> VoucherCheck {
        // ADR 0086 decision 1: a voucher on a network this node has no
        // table for names no channel it could resolve.
        if !self.settles_on(voucher.network()) {
            return VoucherCheck::Unresolved;
        }
        let Ok(proof) = Proof::of_voucher(voucher) else {
            return VoucherCheck::Unresolved;
        };
        let wire_signature = match voucher {
            ClientClaim::EvmVoucher(voucher) => &voucher.signature,
            ClientClaim::SolanaVoucher(voucher) => &voucher.signature,
            ClientClaim::XrplVoucher(voucher) => &voucher.signature,
        };
        self.check(voucher.network(), &proof, &requester(wire_signature))
            .await
    }

    async fn judge_peer_voucher(&self, voucher: &ClientClaim) -> PeerVoucherVerdict {
        let prior = self
            .watermark(&voucher.channel_key())
            .map_or(0, |watermark| watermark.cumulative_amount);
        // No price: coverage is the peer carriage's to measure, from `prior`
        // (`connector_peer_btp::price_gate`). The watermark, the collateral
        // check and the journal are this gate's, exactly as for a client.
        let ack = match self.admit_voucher(voucher.clone(), 0).await {
            Ok((_, ticket)) => match ticket.durable().await {
                Ok(()) => ClaimAckOutcome::Accepted,
                Err(rejection) => voucher_ack(&rejection),
            },
            Err(rejection) => {
                tracing::info!(
                    channel = %voucher.channel_key(),
                    reason = %rejection.message(),
                    "a peer's voucher was not accepted"
                );
                voucher_ack(&rejection)
            }
        };
        PeerVoucherVerdict { ack, prior }
    }

    async fn check_challenge(&self, challenge: &PeerRoleChallenge) -> VoucherCheck {
        if !self.settles_on(challenge.network()) {
            return VoucherCheck::Unresolved;
        }
        let (proof, wire_signature) = match challenge {
            PeerRoleChallenge::Evm {
                channel_id,
                expires,
                signature,
                channel_config,
                ..
            } => {
                let Ok(channel_config) = channel_config
                    .as_ref()
                    .map(EvmVoucherScheme::decode_config)
                    .transpose()
                else {
                    return VoucherCheck::Unresolved;
                };
                (
                    Proof::EvmChallenge {
                        channel_id: *channel_id,
                        channel_config,
                        expires: *expires,
                        signature: signature.to_vec(),
                    },
                    format!("0x{}", hex::encode(signature)),
                )
            }
            PeerRoleChallenge::Solana {
                channel_account,
                expires,
                signature,
                ..
            } => (
                Proof::SolanaChallenge {
                    channel_account: *channel_account,
                    expires: *expires,
                    signature: signature.to_vec(),
                },
                BASE64.encode(signature),
            ),
            PeerRoleChallenge::Xrpl {
                channel_id,
                expires,
                signature,
                ..
            } => (
                Proof::XrplChallenge {
                    channel_id: *channel_id,
                    expires: *expires,
                    signature: signature.clone(),
                },
                hex::encode(signature),
            ),
        };
        self.check(challenge.network(), &proof, &requester(&wire_signature))
            .await
    }
}
