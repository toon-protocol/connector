//! Where a node's x402 `batch-settlement` channels meet its client edge's
//! journal (ADR 0074, epic #1349): the boot step that restores every channel
//! the client edge's journal holds vouchers on, and the view of the claim
//! gate the watchers and sweeps (#1344) read the latest voucher on each
//! channel from ([`ClaimGateVouchers`]). The claim gate itself finds a
//! voucher's channel through the settlement networks (spec #1684), with no
//! adapter here between them.
//!
//! Here rather than in either crate it joins: `connector-client-edge` does
//! not depend on the settlement crates and should not, and ADR 0001 puts
//! construction in `connector-cli`.

use std::sync::Arc;

use connector_client_edge::{ClientClaimGate, JournaledBatchChannel};
use connector_domain::Network;
use connector_runtime::network_of_presentation;
use connector_settlement::batch::{
    BatchSettlementBackend, ChannelPresentation, EvmChannelConfig, HeldVoucher, HeldVouchers,
    Voucher,
};
use connector_settlement::ChannelId;
use connector_signer::BatchChannelConfig;

/// An XRPL channel id as the port spells it: 64 lowercase hex, no prefix.
fn xrpl_channel(channel_id: &[u8; 32]) -> ChannelId {
    ChannelId(
        channel_id
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

/// An EVM channel id as the port spells it: `0x` and 64 lowercase hex.
fn evm_channel(channel_id: &[u8; 32]) -> ChannelId {
    ChannelId(format!(
        "0x{}",
        channel_id
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}

/// A Solana channel account as the port spells it: base58.
fn solana_channel(channel_account: &[u8; 32]) -> ChannelId {
    ChannelId(solana_sdk::pubkey::Pubkey::new_from_array(*channel_account).to_string())
}

/// The gate's config as the port carries it: the same seven fields.
fn port_config(config: &BatchChannelConfig) -> EvmChannelConfig {
    EvmChannelConfig {
        payer: config.payer,
        payer_authorizer: config.payer_authorizer,
        receiver: config.receiver,
        receiver_authorizer: config.receiver_authorizer,
        token: config.token,
        withdraw_delay: config.withdraw_delay,
        salt: config.salt,
    }
}

/// The claim gate as the watchers and sweeps read it (ADR 0074 decision 5):
/// every batch-settlement channel it has accepted a voucher on, with that
/// channel's latest voucher. The gate stays the one place a voucher is
/// accepted; this only reads what it holds.
pub(crate) struct ClaimGateVouchers(pub(crate) Arc<ClientClaimGate>);

impl HeldVouchers for ClaimGateVouchers {
    fn held_vouchers(&self) -> Vec<HeldVoucher> {
        self.0
            .batch_channels()
            .into_iter()
            .filter_map(|channel| {
                let (amount, signature) = self.0.latest_inbound_claim(&channel.channel_key())?;
                Some(held_voucher(channel, amount, signature))
            })
            .collect()
    }

    /// An XRPL channel id binds no network, so the gate's record of the
    /// network the channel's voucher named answers where the presentation
    /// cannot (ADR 0087 decision 2).
    fn network_of(&self, channel: &ChannelId) -> Option<String> {
        self.0
            .batch_channels()
            .into_iter()
            .find_map(|held| match held {
                JournaledBatchChannel::Xrpl {
                    channel_id,
                    network,
                } if xrpl_channel(&channel_id) == *channel => Some(network.caip2().to_string()),
                _ => None,
            })
    }
}

/// [`ClaimGateVouchers`] narrowed to the channels on one network (ADR
/// 0086): what one network's watcher lands, so a backend is never handed a
/// channel on another network. Which network a channel is on is proved from
/// its presentation against `networks`, every network this node settles on.
pub(crate) struct NetworkVouchers {
    pub(crate) held: ClaimGateVouchers,
    pub(crate) network: Network,
    pub(crate) networks: Vec<Network>,
}

impl HeldVouchers for NetworkVouchers {
    fn held_vouchers(&self) -> Vec<HeldVoucher> {
        self.held
            .held_vouchers()
            .into_iter()
            .filter(|held| {
                // The gate's record first: an XRPL id says no network.
                let recorded = self
                    .held
                    .network_of(held.presentation.channel())
                    .and_then(|network| network.parse::<Network>().ok());
                recorded
                    .or_else(|| network_of_presentation(&held.presentation, &self.networks))
                    .as_ref()
                    == Some(&self.network)
            })
            .collect()
    }
}

/// The claim gate as the connector asks which network a payment's channel
/// is on (ADR 0086 decision 14): the network whose domain reproduces the
/// channel the gate journaled, among every network this node settles on.
pub(crate) struct GateChannelNetworks {
    pub(crate) gate: Arc<ClientClaimGate>,
    pub(crate) networks: Vec<Network>,
}

impl connector_runtime::ChannelNetworks for GateChannelNetworks {
    fn network_of(&self, channel_key: &str) -> Option<Network> {
        let channel = self
            .gate
            .batch_channels()
            .into_iter()
            .find(|channel| channel.channel_key() == channel_key)?;
        match channel {
            // Recorded with the channel: its id says none (ADR 0087).
            JournaledBatchChannel::Xrpl { network, .. } => Some(Network::Xrpl(network)),
            channel => network_of_presentation(&presentation(channel), &self.networks),
        }
    }
}

/// A journaled channel and its latest voucher, as the port names them.
fn held_voucher(channel: JournaledBatchChannel, amount: u128, signature: Vec<u8>) -> HeldVoucher {
    HeldVoucher {
        presentation: presentation(channel),
        voucher: Voucher {
            cumulative_amount: amount,
            signature,
        },
    }
}

/// A journaled channel as it is presented to the port.
fn presentation(channel: JournaledBatchChannel) -> ChannelPresentation {
    match channel {
        JournaledBatchChannel::Evm { channel_id, config } => ChannelPresentation::Evm {
            channel: evm_channel(&channel_id),
            config: port_config(&config),
        },
        JournaledBatchChannel::Solana { channel_account } => ChannelPresentation::Solana {
            channel: solana_channel(&channel_account),
        },
        JournaledBatchChannel::Xrpl { channel_id, .. } => ChannelPresentation::Xrpl {
            channel: xrpl_channel(&channel_id),
        },
    }
}

/// Restore every batch-settlement channel `channels` names to its chain's
/// backend, so the port's `channel_state` and `land` work on it from the
/// moment this node serves -- the journal is the only place an EVM
/// channel's config survives a restart (ADR 0074 decision 2).
///
/// **Restored, not re-admitted.** These channels hold vouchers this node
/// already accepted, and landing them is what protects that value (ADR 0074
/// decision 5). Re-running admission here would strand them whenever a rule
/// had tightened since -- a raised minimum delay -- while the payer walks
/// off with what it paid. So only what landing needs is checked: the
/// channel still exists on chain under the id its presentation derives. The
/// rules still gate **new** vouchers: the claim gate admits each channel
/// afresh, under this node's current terms, before it accepts one.
///
/// Best effort, channel by channel, and never a refusal to start: a channel
/// that cannot be restored now is logged and left, unadmitted. Whether
/// anything tries again is the backend's to say (XRPL's: ADR 0087, the
/// #1641 update). A channel whose chain has since been opted
/// out of is logged too: its vouchers can no longer be accepted here, and
/// whatever it still holds unlanded is the operator's to collect.
///
/// An EVM channel goes to the network whose domain reproduces its id (ADR
/// 0086): its journal key names no network, and its id binds one.
pub(crate) async fn restore_journaled_channels(
    channels: &[JournaledBatchChannel],
    backends: &[(Network, Arc<dyn BatchSettlementBackend>)],
) {
    for channel in channels {
        let presentation = presentation(*channel);
        let network = match channel {
            // The network the voucher named, journaled with the channel.
            JournaledBatchChannel::Xrpl { network, .. } => Some(Network::Xrpl(*network)),
            _ => {
                network_of_presentation(&presentation, backends.iter().map(|(network, _)| network))
            }
        };
        let backend = backends
            .iter()
            .find(|(held, _)| Some(held) == network.as_ref())
            .map(|(_, backend)| backend);
        let channel = presentation.channel().clone();
        let Some(backend) = backend else {
            tracing::warn!(
                %channel,
                chain = presentation.chain(),
                "the client-edge journal holds vouchers on a batch-settlement channel on a \
                 network this node no longer opts in to; they will not be landed from here"
            );
            continue;
        };
        if let Err(error) = backend.restore(presentation).await {
            tracing::warn!(
                %channel,
                %error,
                "could not restore a batch-settlement channel the client-edge journal records; \
                 it is not admitted in this process, and only a backend that watches it \
                 will try again"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A watcher lands what the gate journaled: an EVM channel with the
    /// config `claim` needs, a Solana one by its account, each carrying its
    /// latest voucher's amount and signature unchanged.
    #[test]
    fn a_journaled_channel_is_held_under_the_ports_presentation() {
        let config = BatchChannelConfig {
            payer: [1; 20],
            payer_authorizer: [2; 20],
            receiver: [3; 20],
            receiver_authorizer: [4; 20],
            token: [5; 20],
            withdraw_delay: 86_400,
            salt: [6; 32],
        };
        let evm = held_voucher(
            JournaledBatchChannel::Evm {
                channel_id: [0xab; 32],
                config,
            },
            300,
            vec![9; 65],
        );
        assert_eq!(
            evm.presentation,
            ChannelPresentation::Evm {
                channel: evm_channel(&[0xab; 32]),
                config: port_config(&config),
            }
        );
        assert_eq!(evm.voucher.cumulative_amount, 300);
        assert_eq!(evm.voucher.signature, vec![9; 65]);

        let solana = held_voucher(
            JournaledBatchChannel::Solana {
                channel_account: [0x01; 32],
            },
            7,
            vec![8; 64],
        );
        assert_eq!(
            solana.presentation,
            ChannelPresentation::Solana {
                channel: solana_channel(&[0x01; 32]),
            }
        );
        assert_eq!(solana.voucher.cumulative_amount, 7);
    }

    #[test]
    fn channel_ids_are_spelled_as_the_port_spells_them() {
        assert_eq!(evm_channel(&[0xab; 32]).0, format!("0x{}", "ab".repeat(32)));
        assert_eq!(
            solana_channel(&[0x01; 32]).0,
            solana_sdk::pubkey::Pubkey::new_from_array([0x01; 32]).to_string()
        );
    }
}
