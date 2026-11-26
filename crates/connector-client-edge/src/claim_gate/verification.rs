//! **Find the channel, then verify** (spec #1685, issue #1716): the one
//! function the claim gate, the peer voucher evidence check and the
//! claim-state reader all prove a signature on a voucher channel through.
//!
//! Finding is I/O, so it stays behind the network's batch settlement port:
//! the voucher's own `network` picks the one network asked
//! ([`VoucherChannels`](crate::batch_settlement::VoucherChannels)), and the port answers the channel's recorded signer
//! and state. Verifying is the chain's voucher scheme's, against exactly
//! what the port answered and never against anything the voucher carries.
//! EVM's is [`EvmVoucherScheme`], Solana's [`SolanaVoucherScheme`] and
//! XRPL's [`XrplVoucherScheme`].
//!
//! A channel this gate has no record of is a discovery, metered against the
//! unresolvable-lookup budget (issue #613): charged before the port is
//! asked, and given back if it found the channel.

use connector_domain::client_claim::{ClientClaim, EVM_NAMESPACE};
use connector_domain::Network;
use connector_settlement::batch::{BatchChannelState, ChannelPresentation, VoucherSigner};
use connector_signer::{
    BatchChannelConfig, BatchSettlementDomain, EvmVoucherScheme, ParsedEvmVoucher,
    ParsedSolanaVoucher, ParsedXrplVoucher, RecordedSolanaChannel, SolanaVoucherDomain,
    SolanaVoucherScheme, VoucherRefusal, VoucherScheme, XrplVoucherDomain, XrplVoucherScheme,
};

use super::ClientClaimGate;
use crate::batch_settlement::JournaledBatchChannel;
use crate::batch_settlement::{evm_channel, port_config, solana_channel, xrpl_channel};
use crate::channels::ChannelResolutionError;

/// A signature on a voucher channel, and what it claims to sign: a
/// voucher's amount, or a claim-state challenge's expiry. Each chain's
/// voucher and challenge sign different bytes, so neither verifies as the
/// other.
#[derive(Debug, Clone)]
pub(crate) enum Proof {
    /// An EVM voucher, parsed through its scheme.
    EvmVoucher(ParsedEvmVoucher),
    /// An EVM claim-state challenge. The config is the one the request
    /// presented, if any; this gate's record of the channel stands in for
    /// one it did not.
    EvmChallenge {
        channel_id: [u8; 32],
        channel_config: Option<BatchChannelConfig>,
        expires: u64,
        signature: Vec<u8>,
    },
    /// A Solana voucher, parsed through its scheme.
    SolanaVoucher(ParsedSolanaVoucher),
    /// A Solana claim-state challenge.
    SolanaChallenge {
        channel_account: [u8; 32],
        expires: u64,
        signature: Vec<u8>,
    },
    /// An XRPL voucher, parsed through its scheme.
    XrplVoucher(ParsedXrplVoucher),
    /// An XRPL claim-state challenge.
    XrplChallenge {
        channel_id: [u8; 32],
        expires: u64,
        signature: Vec<u8>,
    },
}

impl Proof {
    /// A voucher as its scheme reads it. Every field's shape was checked by
    /// the domain's parser, so a refusal here is the bytes, not the shape.
    pub(crate) fn of_voucher(voucher: &ClientClaim) -> Result<Proof, VoucherRefusal> {
        match voucher {
            ClientClaim::EvmVoucher(voucher) => {
                EvmVoucherScheme::parse(voucher).map(Proof::EvmVoucher)
            }
            ClientClaim::SolanaVoucher(voucher) => {
                SolanaVoucherScheme::parse(voucher).map(Proof::SolanaVoucher)
            }
            ClientClaim::XrplVoucher(voucher) => {
                XrplVoucherScheme::parse(voucher).map(Proof::XrplVoucher)
            }
        }
    }
}

/// A channel found and a signature checked against what its chain records.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Found {
    /// The channel, as the journal records it.
    pub(crate) channel: JournaledBatchChannel,
    /// The key the signature was checked against, as the chain records it.
    pub(crate) signer: VoucherSigner,
    /// The highest cumulative amount a voucher on it may name now (ADR 0074
    /// decision 5): read, never cached, since on EVM it can fall.
    pub(crate) max_cumulative: u128,
    /// Whether the signature is the recorded signer's.
    pub(crate) verified: bool,
}

/// Why no channel was found to verify against.
#[derive(Debug)]
pub(crate) enum NotFound {
    /// No channel by that name on that network, or none this node admits.
    Unknown,
    /// The channel's parts do not derive the id the proof names (ADR 0074
    /// decision 2): refused before the chain is asked anything.
    ChannelIdMismatch,
    /// The lookup failed, was withheld, or found a channel that takes no
    /// new voucher.
    Unresolved(ChannelResolutionError),
}

impl ClientClaimGate {
    /// Find the channel `proof` names on `network`, then verify its
    /// signature against the signer the chain records for it. `requester`
    /// is whom a discovery is metered as.
    pub(crate) async fn find_then_verify(
        &self,
        network: &Network,
        proof: &Proof,
        requester: &str,
    ) -> Result<Found, NotFound> {
        match proof {
            Proof::EvmVoucher(voucher) => {
                self.find_then_verify_evm(
                    network,
                    &voucher.channel_id,
                    voucher.channel_config,
                    requester,
                    |domain, config| EvmVoucherScheme::verify(domain, voucher, config),
                )
                .await
            }
            Proof::EvmChallenge {
                channel_id,
                channel_config,
                expires,
                signature,
            } => {
                self.find_then_verify_evm(
                    network,
                    channel_id,
                    *channel_config,
                    requester,
                    |domain, config| {
                        EvmVoucherScheme::verify_challenge(
                            domain, channel_id, *expires, signature, config,
                        )
                    },
                )
                .await
            }
            Proof::SolanaVoucher(voucher) => {
                self.find_then_verify_solana(
                    network,
                    &voucher.channel_account,
                    requester,
                    |domain, recorded| SolanaVoucherScheme::verify(domain, voucher, recorded),
                )
                .await
            }
            Proof::SolanaChallenge {
                channel_account,
                expires,
                signature,
            } => {
                self.find_then_verify_solana(
                    network,
                    channel_account,
                    requester,
                    |domain, recorded| {
                        SolanaVoucherScheme::verify_challenge(
                            domain,
                            channel_account,
                            *expires,
                            signature,
                            recorded,
                        )
                    },
                )
                .await
            }
            Proof::XrplVoucher(voucher) => {
                self.find_then_verify_xrpl(
                    network,
                    &voucher.channel_id,
                    requester,
                    |domain, key| XrplVoucherScheme::verify(domain, voucher, key),
                )
                .await
            }
            Proof::XrplChallenge {
                channel_id,
                expires,
                signature,
            } => {
                self.find_then_verify_xrpl(network, channel_id, requester, |domain, key| {
                    XrplVoucherScheme::verify_challenge(
                        domain, channel_id, *expires, signature, key,
                    )
                })
                .await
            }
        }
    }

    /// An EVM channel is found only from its config, since the contract
    /// stores a hash: the presented one, else this gate's record. Either is
    /// re-hashed to the channel id under the network's own domain (ADR 0086)
    /// before the chain is asked, and the scheme verifies against exactly
    /// the config the network admitted.
    async fn find_then_verify_evm(
        &self,
        network: &Network,
        channel_id: &[u8; 32],
        presented: Option<BatchChannelConfig>,
        requester: &str,
        verify: impl FnOnce(&BatchSettlementDomain, &BatchChannelConfig) -> Result<(), VoucherRefusal>,
    ) -> Result<Found, NotFound> {
        let domain = EvmVoucherScheme::domain_on(network)
            .filter(|_| self.voucher_channels.settles_on(network))
            .ok_or(NotFound::Unknown)?;
        let known =
            self.known_batch_channel(&format!("{EVM_NAMESPACE}:0x{}", hex::encode(channel_id)));
        let recorded = match known {
            Some(JournaledBatchChannel::Evm { config, .. }) => Some(config),
            _ => None,
        };
        let config = presented.or(recorded).ok_or(NotFound::Unknown)?;
        EvmVoucherScheme::check_channel_id(&domain, channel_id, &config)
            .map_err(|_| NotFound::ChannelIdMismatch)?;
        let presentation = ChannelPresentation::Evm {
            channel: evm_channel(channel_id),
            config: port_config(&config),
        };
        let state = self
            .metered_find(known.is_none(), requester, network, presentation)
            .await?;
        let verified = match verify(&domain, &config) {
            Ok(()) => true,
            Err(VoucherRefusal::ChannelIdMismatch) => return Err(NotFound::ChannelIdMismatch),
            Err(_) => false,
        };
        Ok(Found {
            channel: JournaledBatchChannel::Evm {
                channel_id: *channel_id,
                config,
            },
            signer: VoucherSigner::Evm(EvmVoucherScheme::signer(&config)),
            max_cumulative: state.voucher_ceiling(),
            verified,
        })
    }

    /// Every field of a Solana channel is on chain: the signer is its
    /// `authorized_signer` (ADR 0074 decision 4), and the channel is the
    /// account the network answered at. The scheme verifies against exactly
    /// that.
    async fn find_then_verify_solana(
        &self,
        network: &Network,
        channel_account: &[u8; 32],
        requester: &str,
        verify: impl FnOnce(&SolanaVoucherDomain, &RecordedSolanaChannel) -> Result<(), VoucherRefusal>,
    ) -> Result<Found, NotFound> {
        if !self.voucher_channels.settles_on(network) {
            return Err(NotFound::Unknown);
        }
        let channel = JournaledBatchChannel::Solana {
            channel_account: *channel_account,
        };
        let unseen = self.known_batch_channel(&channel.channel_key()).is_none();
        let presentation = ChannelPresentation::Solana {
            channel: solana_channel(channel_account),
        };
        let state = self
            .metered_find(unseen, requester, network, presentation)
            .await?;
        // A Solana network reporting another chain's signer is not a channel
        // this gate can verify against.
        let VoucherSigner::Solana(authorized_signer) = state.voucher_signer else {
            return Err(NotFound::Unknown);
        };
        let recorded = RecordedSolanaChannel {
            channel_account: *channel_account,
            authorized_signer,
        };
        let verified = match verify(&SolanaVoucherDomain::payment_channels(), &recorded) {
            Ok(()) => true,
            Err(VoucherRefusal::ChannelIdMismatch) => return Err(NotFound::ChannelIdMismatch),
            Err(_) => false,
        };
        Ok(Found {
            channel,
            signer: state.voucher_signer,
            max_cumulative: u128::from(saturating_u64(&state)),
            verified,
        })
    }

    /// Every field of an XRPL channel is on the ledger: the signer is its
    /// `PublicKey`, and the scheme verifies against it. A channel id binds
    /// no network (ADR 0087 decision 2), so the network asked is the one the
    /// proof names.
    async fn find_then_verify_xrpl(
        &self,
        network: &Network,
        channel_id: &[u8; 32],
        requester: &str,
        verify: impl FnOnce(&XrplVoucherDomain, &[u8; 33]) -> Result<(), VoucherRefusal>,
    ) -> Result<Found, NotFound> {
        let Network::Xrpl(xrpl) = network else {
            return Err(NotFound::Unknown);
        };
        let domain = XrplVoucherScheme::domain_on(network)
            .filter(|_| self.voucher_channels.settles_on(network))
            .ok_or(NotFound::Unknown)?;
        let channel = JournaledBatchChannel::Xrpl {
            channel_id: *channel_id,
            network: *xrpl,
        };
        let unseen = self.known_batch_channel(&channel.channel_key()).is_none();
        let presentation = ChannelPresentation::Xrpl {
            channel: xrpl_channel(channel_id),
        };
        let state = self
            .metered_find(unseen, requester, network, presentation)
            .await?;
        // An XRPL network reporting another chain's signer is not a channel
        // this gate can verify against.
        let VoucherSigner::Xrpl(key) = state.voucher_signer else {
            return Err(NotFound::Unknown);
        };
        Ok(Found {
            channel,
            signer: state.voucher_signer,
            max_cumulative: u128::from(saturating_u64(&state)),
            verified: verify(&domain, &key).is_ok(),
        })
    }

    /// Ask `network` for `presentation`'s channel, metered as a discovery
    /// when `unseen` (issue #613).
    async fn metered_find(
        &self,
        unseen: bool,
        requester: &str,
        network: &Network,
        presentation: ChannelPresentation,
    ) -> Result<BatchChannelState, NotFound> {
        let budget = self.lookup_budget();
        let reservation = if unseen {
            match budget.reserve(requester).await {
                Ok(reservation) => Some(reservation),
                Err(exhausted) => {
                    tracing::warn!(
                        bound = exhausted.bound.as_str(),
                        allowance = exhausted.allowance,
                        signer = %requester,
                        "declining to look up an unknown batch-settlement channel: this node's \
                         discovery drain is saturated and its queue is full"
                    );
                    return Err(NotFound::Unresolved(ChannelResolutionError::Budgeted(
                        exhausted,
                    )));
                }
            }
        } else {
            None
        };
        let found = self
            .voucher_channels
            .find(network, presentation)
            .await
            .map_err(NotFound::Unresolved)?;
        if let (Some(reservation), true) = (reservation, found.is_some()) {
            budget.refund(reservation);
        }
        found.ok_or(NotFound::Unknown)
    }
}

/// The port's voucher ceiling in a `u64` chain's amounts: Solana's signed
/// message and SPL amounts (ADR 0074 decision 3, amended 2026-09-30 #1429),
/// XRPL's drops (ADR 0087 decision 2). A ceiling wider than any `u64` bounds
/// nothing a voucher there can name, so it saturates: clamping a bound down
/// loses no value, where a voucher amount narrowed must never be truncated.
/// EVM's ceiling is `u128` and never narrows.
fn saturating_u64(state: &BatchChannelState) -> u64 {
    u64::try_from(state.voucher_ceiling()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use connector_settlement::batch::BatchChannelStatus;
    use connector_settlement::ChannelId;

    /// Solana's and XRPL's amounts are `u64`: a wider ceiling saturates
    /// rather than wraps.
    #[test]
    fn a_u64_chains_ceiling_wider_than_u64_saturates_rather_than_wraps() {
        let wide = BatchChannelState {
            id: ChannelId("c".to_string()),
            status: BatchChannelStatus::Open,
            voucher_signer: VoucherSigner::Solana([7; 32]),
            landed: u128::from(u64::MAX),
            collateral: 5,
        };
        assert_eq!(saturating_u64(&wide), u64::MAX);
    }
}
