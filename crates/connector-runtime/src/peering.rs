//! Establishing a peering from a URL: ADR 0058's one operator write, as ADR
//! 0075 amends it.
//!
//! ```text
//! POST /peers { id, url, fee, max_packet_amount, deposit? }
//! ```
//!
//! The node reads the counterparty's self-description (ADR 0050), opens
//! **its own outbound channel** toward it, and writes a durable runtime
//! peering -- with no restart and no edit to the config file.
//!
//! # A peering is two channels (ADR 0075 decision 4)
//!
//! On EVM every channel is a one-way x402 `batch-settlement` channel, so a
//! peering is two of them: A→B carries A's vouchers to B, B→A carries B's to
//! A. This write opens and funds only this node's own, and the other half
//! is **admitted, not configured**: the counterparty opens it the same way,
//! by writing `POST /peers` naming this node's URL, and its first voucher
//! is admitted by the receiving half's rules. What makes that channel the
//! counterparty's rather than a client's is its voucher signer -- EVM
//! `payerAuthorizer` -- being the key the counterparty's self-description
//! publishes (`voucherSigners`), which this write binds to the peering.
//! Neither operator pastes the other's channel id anywhere.
//!
//! On Solana, until #1379, a peering is still one `toon-channel` held in
//! both roles and derived from the two settlement keys (ADR 0059).
//!
//! # Where each part of a peering comes from
//!
//! | fact | source |
//! | --- | --- |
//! | endpoint | the self-description |
//! | carriage | the endpoint's **scheme**, and nothing else |
//! | edge identity | the self-description |
//! | receiver terms, voucher signer | the self-description (`batchSettlements`, `voucherSigners`) |
//! | the outbound channel | **this node's own**, found among the channels it opened, or opened now |
//! | `id` | **the operator.** A label in their own namespace |
//! | `fee`, `max_packet_amount`, `deposit` | **the operator.** Policy, not facts |
//!
//! `id` is never derived from the peer's ILP address -- that is
//! self-asserted, a claim and not a grant (`CONTEXT.md`, **ILP address**),
//! so deriving from it would let a stranger choose what this node's route
//! table is keyed on and what its logs say. Nor from the URL host, which
//! has a milder form of the same problem and breaks when they move hosts.
//!
//! # Three identities, and they are not interchangeable
//!
//! The **edge identity** is a secp256k1 key: what a payload is sealed to
//! (ADR 0018). The **EVM settlement address** is 20 bytes, and is both the
//! receiver this node's channel names and -- as the peer's voucher signer
//! -- what the peer's channel toward this node is bound by. The **Solana
//! settlement address** is a base58 ed25519 public key. None of them is
//! ever read in place of another.
//!
//! # Trust-on-first-use
//!
//! Whatever the URL serves is who the peering is with. The fetched identity
//! is not checked against anything the operator supplied; ADR 0058
//! considered a `settlement_address` pin and rejected it, because an
//! operator who copies the address out of the same document they are
//! pointing the node at has pinned nothing. A party who controls the URL's
//! DNS or a certificate for it chooses the counterparty -- and so the
//! receiver this node funds a channel toward. The operator's vetting of the
//! URL is the whole of the assurance (ADR 0075 decision 4: "unchanged and
//! unstrengthened").
//!
//! What that does **not** weaken: every value-bearing check downstream is
//! unchanged and remains cryptographic. A voucher's signature is verified
//! against the signer the chain records for its channel and never against
//! anything the voucher declares about itself, and a payload is sealed to
//! the edge identity.
//!
//! # The endpoint can spend gas
//!
//! Opening a channel may submit a transaction and wait for it, so this can
//! fail *after* money has moved. Two rules follow:
//!
//! * the durable row is written from a **confirmed** channel -- the id
//!   comes back off the chain, never off the submitted transaction;
//! * repeating the request against a peering already established is a
//!   **success, not a second channel**: this node looks among its own open
//!   channels toward that receiver first, and an open journaled but not yet
//!   confirmed is resumed rather than sent again (ADR 0075 decision 8).
//!
//! And the answer says which branch it took ([`ChannelBranch`]), so an
//! unintended second channel is visible in the operator's own output rather
//! than discovered later on a block explorer.

use chrono::Duration;
use connector_config::{SettlementChain, DEFAULT_MAX_PACKET_AMOUNT};
use connector_domain::x402::{
    X402BatchSettlementEvmTerms, X402BatchSettlementTerms, X402ChainSettlementTerms,
    X402SolanaSettlementTerms,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use crate::batch_channels::{receiver_terms, BatchChannelError};
use crate::connector::{hex_lower, ChannelOperationError, Connector, PeerRouteTableError};
use crate::operator_view::PeerView;
use crate::peer_route_store::{RuntimePeerChannel, RuntimePeering};
use crate::self_description::SelfDescriptionError;
use crate::voucher_binding::{VoucherBindingError, VoucherSigner};

/// The withdrawal-safety window a Solana `toon-channel` opened by
/// establishing a peering gets (#1379 moves Solana to x402).
///
/// A day, which is comfortably past the program's minimum and is a window
/// an operator can act inside without watching a clock. An EVM peering's
/// channel takes the counterparty's own published minimum `withdrawDelay`
/// instead (ADR 0075 decision 3).
pub const PEERING_SETTLEMENT_TIMEOUT_SECONDS: i64 = 24 * 60 * 60;

/// Which branch the find-or-open took.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelBranch {
    /// This node already had a live channel toward the counterparty, and
    /// this peering pays on it. What a repeat of the same request reports.
    Found,
    /// No channel existed, so one was opened and confirmed.
    Created,
}

/// The channel a peering was established on, and how it got there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EstablishedChannel {
    /// The channel's on-chain identifier, as read back from the chain: on
    /// EVM this node's own outbound x402 channel toward the counterparty.
    pub id: String,
    pub status: ChannelBranch,
    /// Which chain it lives on -- `"evm"` or `"solana"`.
    pub chain: String,
}

/// What `POST /peers` answers: the peering, and the channel branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeeringEstablished {
    #[serde(flatten)]
    pub peer: PeerView,
    pub channel: EstablishedChannel,
}

/// Why a peering could not be established.
#[derive(Debug, Error)]
pub enum EstablishPeeringError {
    /// The URL's document could not be read. Named separately from every
    /// refusal below because it is the one failure that is about the
    /// *counterparty's* host rather than about this node's state.
    #[error(transparent)]
    SelfDescription(#[from] SelfDescriptionError),
    /// The document named no endpoint this node can dial on a carriage it
    /// speaks, so there would be no way to reach the counterparty.
    #[error(
        "{url} publishes no endpoint this connector can dial (its schemes select no carriage)"
    )]
    NoDialableEndpoint { url: String },
    /// The document published no `httpEndpoint` (issue #1217): the
    /// counterparty's `POST /ilp/claim-state` is where this node's watermark
    /// on its outbound channel is restored from (ADR 0075 decision 6), and
    /// is asked over plain HTTP whichever carriage the packets ride.
    #[error(
        "{url} publishes no httpEndpoint, so its claim-state -- where this node's outbound \
         watermark is restored from -- can never be asked"
    )]
    NoDialableClientEdge { url: String },
    /// This node and the counterparty settle on no chain in common -- on
    /// EVM, no x402 `batch-settlement` terms this node can pay on -- so no
    /// channel can exist between them and no packet could ever be paid.
    #[error(
        "this connector and {url} settle on no chain in common (on EVM, a peering needs x402 \
         batch-settlement on both nodes), so no channel can be opened"
    )]
    NoSharedChain { url: String },
    /// Both nodes settle on more than one chain in common and the request
    /// named none. Refused rather than resolved silently -- picking one for
    /// the operator is picking which asset a peering settles in.
    #[error(
        "this connector and {url} share settlement on {chains}; name one as `chain` in the request"
    )]
    AmbiguousChain { url: String, chains: String },
    /// The document's settlement address or voucher signer for the chosen
    /// chain is not one of that chain's shape. Never coerced into one.
    #[error("{url} published a {chain} settlement address this connector cannot read: {value}")]
    UnreadableSettlementAddress {
        url: String,
        chain: String,
        value: String,
    },
    /// The counterparty's x402 terms name another network than this
    /// node's: a channel opened there is one neither node's backend reads.
    #[error("{url} settles on {theirs}, and this node's x402 backend on {ours}")]
    NetworkMismatch {
        url: String,
        theirs: String,
        ours: String,
    },
    /// The counterparty publishes no voucher signer for the chain (ADR 0075
    /// decision 10), so its channel toward this node could never be bound
    /// to the peering, and its packets would never prove the peer role.
    #[error(
        "{url} publishes no voucherSigner for {network}, so its channel toward this node could \
         never be bound to the peering (ADR 0075 decision 4)"
    )]
    NoVoucherSigner { url: String, network: String },
    /// The counterparty's published terms cannot be opened on.
    #[error("{url}'s x402 terms cannot be opened on: {reason}")]
    InvalidTerms { url: String, reason: String },
    /// This node holds no open channel toward the counterparty, and the
    /// request named no `deposit` to open one with. Checked after the
    /// document is read, since only then is it known whether one exists.
    #[error(
        "this node has no channel toward {url} yet; name the opening `deposit` (in base units \
         of the shared token) to open one"
    )]
    DepositRequired { url: String },
    /// Opening, reading or restoring this node's outbound channel failed.
    #[error(transparent)]
    Outbound(#[from] BatchChannelError),
    /// The counterparty's voucher signer already proves another peering on
    /// this node: one signer, one relation (ADR 0075 decision 4).
    #[error(transparent)]
    Binding(#[from] VoucherBindingError),
    /// The chain operation itself failed -- on Solana, reading whether a
    /// channel exists, or opening one.
    #[error(transparent)]
    Channel(#[from] ChannelOperationError),
    /// The durable write was refused. Carries ADR 0034's precedence rules
    /// through unchanged.
    #[error(transparent)]
    Table(#[from] PeerRouteTableError),
}

impl Connector {
    /// Establish a peering with whoever answers `url`: ADR 0058's whole
    /// operator write, as ADR 0075 decision 4 amends it.
    ///
    /// Refuses **before any outbound request** on anything about this
    /// node's own state that would make the write unlandable -- an empty
    /// id, or one the config file owns (ADR 0034). A peering that could
    /// never be written is not worth a stranger's host being dialled for.
    ///
    /// `chain` is an optional disambiguator for the one case that has no
    /// honest default: two nodes that settle on more than one chain in
    /// common. `deposit` is what an EVM peering's outbound channel is
    /// opened with when this node has none toward the counterparty yet;
    /// with one already open it is not spent (top that channel up with
    /// `POST /channels/:id/fund` instead).
    pub async fn establish_peering(
        &self,
        id: impl Into<String>,
        url: &Url,
        fee: u64,
        max_packet_amount: u64,
        chain: Option<SettlementChain>,
        deposit: Option<u128>,
    ) -> Result<PeeringEstablished, EstablishPeeringError> {
        let id = id.into();
        self.refuse_unlandable_peering(&id)?;

        let document = self.self_description_source().fetch(url).await?;

        let endpoint = peer_endpoint(&document, self.peer_allows_plaintext()).ok_or_else(|| {
            EstablishPeeringError::NoDialableEndpoint {
                url: url.to_string(),
            }
        })?;
        // Checked before any chain operation (channel-opening gas
        // included), on a fact the document already answered: a peering
        // whose watermark can never be restored is not worth a channel.
        let client_edge_url = document.http_endpoint.clone().ok_or_else(|| {
            EstablishPeeringError::NoDialableClientEdge {
                url: url.to_string(),
            }
        })?;

        let terms = self.shared_settlement(&document, chain, url)?;
        let (binding, channel) = match terms {
            SharedSettlement::Evm {
                terms,
                voucher_signer,
            } => {
                self.open_evm_peering_channel(&id, url, &terms, voucher_signer, deposit)
                    .await?
            }
            SharedSettlement::Solana(terms) => {
                self.open_solana_peering_channel(url, &terms).await?
            }
        };

        let peering = RuntimePeering {
            fee,
            max_packet_amount,
            endpoint: Some(endpoint.to_string()),
            edge_identity: document
                .edge_identity
                .as_ref()
                .map(|identity| identity.public_key.clone()),
            client_edge_url: Some(client_edge_url.clone()),
            channels: vec![binding.clone()],
        };

        // Wire the peering before the durable write, so a row that lands is
        // a row this node can already act on; and write the row last, so
        // the durable table never names a peering the running process has
        // not wired up. On EVM that is the hop every forward to the peer is
        // covered on (ADR 0075 decision 6); on Solana, until #1379, the one
        // `toon-channel` in both roles (issue #1217).
        self.bind_runtime_peer_channel(&id, &binding);
        self.register_outbound_client_hop(&id, &binding, &client_edge_url);
        self.register_voucher_hop(&id, &binding, &client_edge_url);
        self.register_runtime_peering(&id, &peering);
        let peer = self.upsert_runtime_peer(id.clone(), peering)?;
        // After the row, because a signer is bound only to a peering that
        // exists; refused before any channel was opened if it could not be.
        self.bind_runtime_voucher_signer(&id, &binding)?;

        Ok(PeeringEstablished { peer, channel })
    }

    /// This node's own outbound x402 channel toward an EVM counterparty
    /// (ADR 0075 decisions 3 and 4): the one it already has open toward the
    /// counterparty's receiver, or a new one opened now on the terms the
    /// counterparty publishes, with `deposit`.
    async fn open_evm_peering_channel(
        &self,
        id: &str,
        url: &Url,
        terms: &X402BatchSettlementEvmTerms,
        voucher_signer: Option<String>,
        deposit: Option<u128>,
    ) -> Result<(RuntimePeerChannel, EstablishedChannel), EstablishPeeringError> {
        let url_text = url.to_string();
        let ours = self.x402_network(SettlementChain::Evm).unwrap_or_default();
        if terms.network != ours {
            return Err(EstablishPeeringError::NetworkMismatch {
                url: url_text,
                theirs: terms.network.clone(),
                ours: ours.to_string(),
            });
        }
        let voucher_signer =
            voucher_signer.ok_or_else(|| EstablishPeeringError::NoVoucherSigner {
                url: url_text.clone(),
                network: terms.network.clone(),
            })?;
        let unreadable = |value: &str| EstablishPeeringError::UnreadableSettlementAddress {
            url: url_text.clone(),
            chain: SettlementChain::Evm.to_string(),
            value: value.to_string(),
        };
        let signer =
            parse_evm_address(&voucher_signer).ok_or_else(|| unreadable(&voucher_signer))?;
        // One signer, one relation: refused before a channel is opened for
        // a peering that could then never be bound.
        if let Some(bound_to) = self.voucher_signer_peer(&VoucherSigner::Evm(signer)) {
            if bound_to != id {
                return Err(VoucherBindingError::SignerBoundElsewhere {
                    signer: voucher_signer,
                    bound_to,
                }
                .into());
            }
        }
        let receiver = parse_evm_address(&terms.pay_to).ok_or_else(|| unreadable(&terms.pay_to))?;
        let outbound = self
            .outbound_channels()
            .ok_or(BatchChannelError::NoBackend(SettlementChain::Evm))?;

        let (channel_id, status) = match outbound.live_toward(&VoucherSigner::Evm(receiver)).await?
        {
            Some(channel_id) => (channel_id, ChannelBranch::Found),
            None => {
                let deposit = deposit.ok_or_else(|| EstablishPeeringError::DepositRequired {
                    url: url_text.clone(),
                })?;
                let receiver_terms =
                    receiver_terms(&X402BatchSettlementTerms::Evm(terms.clone()), Some(url))
                        .map_err(|reason| EstablishPeeringError::InvalidTerms {
                            url: url_text.clone(),
                            reason,
                        })?;
                // The id comes back off the chain: `open` restores the
                // channel from it before answering, so a row written here is
                // a row backed by a channel the chain confirmed.
                let (state, _resumed) = outbound.open(receiver_terms, deposit).await?;
                (state.on_chain.id.0, ChannelBranch::Created)
            }
        };
        Ok((
            RuntimePeerChannel::EvmVoucher {
                outbound_channel_id: channel_id.clone(),
                voucher_signer: format!("0x{}", hex_lower(&signer)),
                network: terms.network.clone(),
            },
            EstablishedChannel {
                id: channel_id,
                status,
                chain: SettlementChain::Evm.to_string(),
            },
        ))
    }

    /// The one `toon-channel` a Solana peering holds in both roles, derived
    /// from the two settlement keys and opened if absent (ADR 0059, until
    /// #1379 moves Solana to x402).
    async fn open_solana_peering_channel(
        &self,
        url: &Url,
        terms: &X402SolanaSettlementTerms,
    ) -> Result<(RuntimePeerChannel, EstablishedChannel), EstablishPeeringError> {
        let counterparty = solana_key_bytes(&terms.settlement_address, url)?;
        let settlement = self.settlement_on_chain(SettlementChain::Solana)?;
        let (channel_id, status) = match settlement.live_channel_with(counterparty.clone()).await {
            Ok(Some(existing)) => (existing.0, ChannelBranch::Found),
            Ok(None) => {
                let opened = self
                    .open_channel(
                        Some(SettlementChain::Solana),
                        counterparty,
                        Duration::seconds(PEERING_SETTLEMENT_TIMEOUT_SECONDS),
                    )
                    .await?;
                (opened.id, ChannelBranch::Created)
            }
            Err(error) => return Err(ChannelOperationError::Settlement(error).into()),
        };
        Ok((
            RuntimePeerChannel::Solana {
                channel_account: channel_id.clone(),
                counterparty_key: terms.settlement_address.clone(),
                program_id: terms.program_id.clone(),
            },
            EstablishedChannel {
                id: channel_id,
                status,
                chain: SettlementChain::Solana.to_string(),
            },
        ))
    }
}

/// The endpoint this connector dials a counterparty on, read off its
/// self-description.
///
/// **BTP first where both are published.** A dialed BTP session is
/// symmetric once established, so either side may originate on it
/// (`peer-carriage-spec.md` §2.3); an ILP-over-HTTP peering can only ever
/// be originated on by the dialer (§6.4). Preferring the carriage that
/// leaves both directions open is the choice that forecloses least, and an
/// operator who wants the other one writes the peering in the config file.
///
/// `None` when neither published endpoint selects a carriage this node will
/// dial -- a `wss://`/`https://` endpoint always does, a plaintext one at a
/// `.onion` host always does (ADR 0070), and any other plaintext one only on
/// a node that opted in. Asked through `PeerCarriage::for_endpoint`, so a
/// peering established from a URL reads the same answer a config-file one
/// does rather than a second copy of it.
fn peer_endpoint(
    document: &connector_domain::NodeSelfDescription,
    allow_plaintext: bool,
) -> Option<Url> {
    let dialable = |published: &Option<String>| -> Option<Url> {
        let url = Url::parse(published.as_deref()?).ok()?;
        connector_config::PeerCarriage::for_endpoint(&url, allow_plaintext).map(|_| url)
    };
    dialable(&document.btp_endpoint).or_else(|| dialable(&document.http_endpoint))
}

/// One chain's published settlement facts, narrowed to the chain this
/// connector also settles on.
#[derive(Debug)]
pub(crate) enum SharedSettlement {
    /// The counterparty's x402 terms on EVM (its `batchSettlements` entry),
    /// and the voucher signer it publishes for that network, if any.
    Evm {
        terms: X402BatchSettlementEvmTerms,
        voucher_signer: Option<String>,
    },
    /// The counterparty's `toon-channel` terms on Solana, until #1379.
    Solana(X402SolanaSettlementTerms),
}

impl SharedSettlement {
    pub(crate) fn chain(&self) -> SettlementChain {
        match self {
            SharedSettlement::Evm { .. } => SettlementChain::Evm,
            SharedSettlement::Solana(_) => SettlementChain::Solana,
        }
    }
}

/// A 20-byte EVM address from its `0x`-prefixed (or bare) hex spelling.
///
/// `None` rather than a padded or truncated address for anything that is
/// not exactly 20 bytes of hex: an address coerced into shape names a
/// party no chain holds.
pub(crate) fn parse_evm_address(value: &str) -> Option<[u8; 20]> {
    let hex = value.strip_prefix("0x").unwrap_or(value);
    if hex.len() != 40 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut address = [0u8; 20];
    for (i, byte) in address.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(address)
}

/// A Solana settlement key as the 32 bytes the settlement backend takes, or
/// a named refusal for anything that is not exactly that.
fn solana_key_bytes(value: &str, url: &Url) -> Result<Vec<u8>, EstablishPeeringError> {
    let unreadable = || EstablishPeeringError::UnreadableSettlementAddress {
        url: url.to_string(),
        chain: SettlementChain::Solana.to_string(),
        value: value.to_string(),
    };
    let bytes = bs58::decode(value).into_vec().map_err(|_| unreadable())?;
    if bytes.len() != 32 {
        return Err(unreadable());
    }
    Ok(bytes)
}

/// Narrow a document's published settlements to the one chain this
/// connector will peer on: EVM from its x402 `batchSettlements` entry (ADR
/// 0075), Solana from its `toon-channel` `settlements` entry (until #1379).
pub(crate) fn shared_settlement_of(
    document: &connector_domain::NodeSelfDescription,
    settles_on: impl Fn(SettlementChain) -> bool,
    wanted: Option<SettlementChain>,
    url: &Url,
) -> Result<SharedSettlement, EstablishPeeringError> {
    let evm = document
        .batch_settlements
        .iter()
        .filter_map(|entry| match entry {
            X402BatchSettlementTerms::Evm(evm) => Some(SharedSettlement::Evm {
                voucher_signer: document
                    .voucher_signers
                    .iter()
                    .find(|signer| signer.network == evm.network)
                    .map(|signer| signer.signer.clone()),
                terms: evm.clone(),
            }),
            X402BatchSettlementTerms::Solana(_) => None,
        });
    let solana = document.settlements.iter().filter_map(|entry| match entry {
        X402ChainSettlementTerms::Solana(solana) => Some(SharedSettlement::Solana(solana.clone())),
        X402ChainSettlementTerms::Evm(_) => None,
    });
    let mut shared: Vec<SharedSettlement> = evm
        .chain(solana)
        .filter(|entry| settles_on(entry.chain()))
        .collect();
    if let Some(wanted) = wanted {
        shared.retain(|entry| entry.chain() == wanted);
    }
    match shared.len() {
        0 => Err(EstablishPeeringError::NoSharedChain {
            url: url.to_string(),
        }),
        1 => Ok(shared.remove(0)),
        _ => {
            let chains: Vec<String> = shared
                .iter()
                .map(|entry| entry.chain().to_string())
                .collect();
            Err(EstablishPeeringError::AmbiguousChain {
                url: url.to_string(),
                chains: chains.join(", "),
            })
        }
    }
}

/// The cap a peering row states, or the standing bound when it states
/// none. Shared with [`Connector`]'s own reader so the number reported to
/// an operator and the number enforced on a packet are one rule.
pub(crate) fn stated_cap(max_packet_amount: u64) -> u64 {
    if max_packet_amount > 0 {
        max_packet_amount
    } else {
        DEFAULT_MAX_PACKET_AMOUNT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use async_trait::async_trait;
    use connector_domain::x402::X402BatchSettlementEvmTerms;
    use connector_domain::{EdgeIdentity, NodeFacts, NodeSelfDescription, VoucherSignerFact};
    use connector_settlement::batch::{
        BatchSettlementPayer, InMemoryBatchChain, InMemoryBatchSettlement, PayerExit, ReceiverTerms,
    };
    use connector_settlement::InMemorySettlementBackend;

    use crate::app_client::FakeAppClient;
    use crate::batch_channels::OutboundChannels;
    use crate::clock::TestClock;
    use crate::journal::InMemoryJournal;
    use crate::peer_transport::InProcessPeerTransport;
    use crate::self_description::SelfDescriptionSource;

    const NETWORK: &str = "eip155:31337";

    fn url() -> Url {
        Url::parse("https://peer.example/ilp").expect("url")
    }

    fn evm_batch(pay_to: &str) -> X402BatchSettlementTerms {
        X402BatchSettlementTerms::Evm(X402BatchSettlementEvmTerms {
            network: NETWORK.to_string(),
            asset: "0x00000000000000000000000000000000000000dd".to_string(),
            pay_to: pay_to.to_string(),
            receiver_authorizer: pay_to.to_string(),
            min_withdraw_delay_secs: 86_400,
            name: "USDC".to_string(),
            version: "2".to_string(),
        })
    }

    fn solana() -> X402ChainSettlementTerms {
        X402ChainSettlementTerms::Solana(X402SolanaSettlementTerms {
            chain: "solana".to_string(),
            settlement_address: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
            program_id: "Toon11111111111111111111111111111111111111".to_string(),
            token_address: "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi".to_string(),
            decimals: 6,
        })
    }

    fn document(
        btp: Option<&str>,
        http: Option<&str>,
        settlements: Vec<X402ChainSettlementTerms>,
        batch_settlements: Vec<X402BatchSettlementTerms>,
        voucher_signers: Vec<VoucherSignerFact>,
    ) -> NodeSelfDescription {
        NodeSelfDescription::describe(
            &NodeFacts {
                ilp_addresses: vec!["g.example.peer".to_string()],
                http_endpoint: http.map(str::to_string),
                btp_endpoint: btp.map(str::to_string),
                peer_carriages: vec!["http".to_string()],
                settlements,
                batch_settlements,
                voucher_signers,
            },
            Some(EdgeIdentity {
                key_id: "peer-key".to_string(),
                public_key: "0x04ab".to_string(),
            }),
            Vec::new(),
            None,
        )
    }

    fn endpoints(btp: Option<&str>, http: Option<&str>) -> NodeSelfDescription {
        document(btp, http, Vec::new(), Vec::new(), Vec::new())
    }

    /// §2.3 against §6.4: a dialed BTP session is symmetric once
    /// established, so either side may originate on it, where an
    /// ILP-over-HTTP peering can only ever be originated on by the dialer.
    /// Where a node publishes both, preferring BTP forecloses least.
    #[test]
    fn btp_wins_where_a_node_publishes_both_endpoints() {
        let both = endpoints(
            Some("wss://peer.example/ilp/btp"),
            Some("https://peer.example/ilp"),
        );
        assert_eq!(
            peer_endpoint(&both, false).map(|url| url.to_string()),
            Some("wss://peer.example/ilp/btp".to_string())
        );

        let http_only = endpoints(None, Some("https://peer.example/ilp"));
        assert_eq!(
            peer_endpoint(&http_only, false).map(|url| url.to_string()),
            Some("https://peer.example/ilp".to_string())
        );
    }

    /// A plaintext endpoint selects no carriage on a node that did not opt
    /// in, so such a document publishes nothing this node can dial -- and
    /// a node with an opt-in falls back to it rather than to nothing.
    #[test]
    fn a_plaintext_endpoint_is_dialable_only_where_the_node_opted_in() {
        let plaintext = endpoints(
            Some("ws://127.0.0.1:1/ilp/btp"),
            Some("http://127.0.0.1:1/ilp"),
        );

        assert_eq!(peer_endpoint(&plaintext, false), None);
        assert_eq!(
            peer_endpoint(&plaintext, true).map(|url| url.to_string()),
            Some("ws://127.0.0.1:1/ilp/btp".to_string())
        );
    }

    /// A node publishing no endpoint at all is one this connector could
    /// only ever accept from, and a peering it cannot reach is refused
    /// rather than written.
    #[test]
    fn a_document_with_no_endpoint_is_not_dialable() {
        assert_eq!(peer_endpoint(&endpoints(None, None), true), None);
    }

    /// EVM is shared on the counterparty's x402 terms (ADR 0075), never on
    /// its `toon-channel` ones: a node that publishes only the latter on
    /// EVM shares no chain this build can peer on.
    #[test]
    fn one_shared_chain_is_the_answer_and_none_is_a_named_refusal() {
        let evm_only = document(
            None,
            Some("https://peer.example/ilp"),
            Vec::new(),
            vec![evm_batch("0x00000000000000000000000000000000000000aa")],
            Vec::new(),
        );

        let shared = shared_settlement_of(&evm_only, |_| true, None, &url()).expect("one shared");
        assert_eq!(shared.chain(), SettlementChain::Evm);

        // This node settles only on Solana; the counterparty only on EVM.
        let error = shared_settlement_of(
            &evm_only,
            |chain| chain == SettlementChain::Solana,
            None,
            &url(),
        )
        .expect_err("no chain in common");
        assert!(matches!(error, EstablishPeeringError::NoSharedChain { .. }));

        let no_x402 = document(
            None,
            Some("https://peer.example/ilp"),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        assert!(matches!(
            shared_settlement_of(&no_x402, |_| true, Some(SettlementChain::Evm), &url()),
            Err(EstablishPeeringError::NoSharedChain { .. })
        ));
    }

    /// Two nodes settling on both chains have no honest default, so the
    /// write refuses by name and says which chains it saw.
    #[test]
    fn several_shared_chains_are_refused_unless_the_request_names_one() {
        let both = document(
            None,
            Some("https://peer.example/ilp"),
            vec![solana()],
            vec![evm_batch("0x00000000000000000000000000000000000000aa")],
            Vec::new(),
        );

        let error =
            shared_settlement_of(&both, |_| true, None, &url()).expect_err("ambiguous chain");
        match error {
            EstablishPeeringError::AmbiguousChain { chains, .. } => {
                assert!(chains.contains("evm"), "{chains}");
                assert!(chains.contains("solana"), "{chains}");
            }
            other => panic!("expected an ambiguity refusal, got {other:?}"),
        }

        let named = shared_settlement_of(&both, |_| true, Some(SettlementChain::Solana), &url())
            .expect("the request named one");
        assert_eq!(named.chain(), SettlementChain::Solana);
    }

    /// The voucher signer read for an EVM peering is the one the document
    /// publishes for the same network as its x402 terms -- never another
    /// chain's key, and never the edge identity.
    #[test]
    fn the_voucher_signer_is_read_for_the_terms_own_network() {
        let published = document(
            None,
            Some("https://peer.example/ilp"),
            Vec::new(),
            vec![evm_batch("0x00000000000000000000000000000000000000aa")],
            vec![
                VoucherSignerFact {
                    network: "solana:devnet".to_string(),
                    signer: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
                },
                VoucherSignerFact {
                    network: NETWORK.to_string(),
                    signer: "0x00000000000000000000000000000000000000aa".to_string(),
                },
            ],
        );
        let SharedSettlement::Evm { voucher_signer, .. } =
            shared_settlement_of(&published, |_| true, None, &url()).expect("shared")
        else {
            panic!("EVM is the shared chain");
        };
        assert_eq!(
            voucher_signer.as_deref(),
            Some("0x00000000000000000000000000000000000000aa")
        );
    }

    /// A Solana settlement key is read as exactly 32 bytes, and an EVM
    /// address where it belongs is refused rather than coerced.
    #[test]
    fn a_solana_settlement_key_is_read_in_its_own_shape_or_refused() {
        assert_eq!(
            solana_key_bytes("9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM", &url())
                .expect("32 bytes")
                .len(),
            32
        );
        assert!(matches!(
            solana_key_bytes("0x00000000000000000000000000000000000000aa", &url()),
            Err(EstablishPeeringError::UnreadableSettlementAddress { .. })
        ));
        assert_eq!(
            parse_evm_address("9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM"),
            None
        );
    }

    /// A peering write that could never land is refused **before** any
    /// outbound request is made.
    #[tokio::test]
    async fn a_write_that_cannot_land_is_refused_before_the_fetch() {
        let connector = Connector::new(
            Vec::new(),
            Vec::new(),
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            Arc::new(TestClock::new(chrono::Utc::now())),
        )
        .with_config_peer_ids(["owned-by-config".to_string()])
        .with_settlement(
            SettlementChain::Evm,
            Arc::new(InMemorySettlementBackend::new()),
        );

        // The default source reaches no network at all, so a refusal that
        // named the host would prove the fetch happened first.
        let error = connector
            .establish_peering("owned-by-config", &url(), 0, 0, None, None)
            .await
            .expect_err("the config file owns this id");
        assert!(matches!(
            error,
            EstablishPeeringError::Table(PeerRouteTableError::OwnedByConfig(_))
        ));

        let error = connector
            .establish_peering("  ", &url(), 0, 0, None, None)
            .await
            .expect_err("an empty id is not a label");
        assert!(matches!(
            error,
            EstablishPeeringError::Table(PeerRouteTableError::InvalidPeerId)
        ));
    }

    /// **The fetch is never made on the packet path.**
    #[tokio::test]
    async fn a_node_that_can_reach_no_host_still_has_a_working_packet_path() {
        let connector = Connector::new(
            Vec::new(),
            Vec::new(),
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            Arc::new(TestClock::new(chrono::Utc::now())),
        );

        let error = connector
            .establish_peering("anyone", &url(), 0, 0, None, None)
            .await
            .expect_err("no source configured");
        assert!(matches!(
            error,
            EstablishPeeringError::SelfDescription(SelfDescriptionError::Unreachable { .. })
        ));

        let response = connector
            .handle_prepare(connector_domain::Prepare {
                amount: 100,
                expires_at: chrono::Utc::now() + chrono::Duration::seconds(30),
                greeting: false,
                destination: "g.nowhere".to_string(),
                data: Vec::new(),
            })
            .await;
        assert!(
            matches!(response, connector_domain::PacketResponse::Reject(_)),
            "an unroutable destination is rejected, not left to a fetch"
        );
    }

    // -- ADR 0075 decision 4, over the in-memory batch-settlement fake --

    struct FixedSelfDescription(NodeSelfDescription);

    #[async_trait]
    impl SelfDescriptionSource for FixedSelfDescription {
        async fn fetch(&self, _url: &Url) -> Result<NodeSelfDescription, SelfDescriptionError> {
            Ok(self.0.clone())
        }
    }

    fn hex(bytes: &[u8]) -> String {
        format!("0x{}", hex_lower(bytes))
    }

    /// Node `0x01` paying, on one fake EVM chain with node `0x02`, whose
    /// published terms and voucher signer are in the document it serves.
    async fn paying_node(
        voucher_signer: Option<[u8; 20]>,
    ) -> (Connector, Arc<InMemoryBatchSettlement>) {
        let chain = InMemoryBatchChain::new(PayerExit::Withdrawal);
        let payer = Arc::new(InMemoryBatchSettlement::on(
            Arc::clone(&chain),
            0x01,
            86_400,
        ));
        payer.fund(10_000);
        let counterparty = Arc::new(InMemoryBatchSettlement::on(chain, 0x02, 86_400));
        let ReceiverTerms::Evm(terms) = counterparty.published_terms() else {
            panic!("an EVM-shaped chain");
        };
        let published = X402BatchSettlementTerms::Evm(X402BatchSettlementEvmTerms {
            network: NETWORK.to_string(),
            asset: hex(&terms.token),
            pay_to: hex(&terms.receiver),
            receiver_authorizer: hex(&terms.receiver),
            min_withdraw_delay_secs: terms.min_withdraw_delay_secs,
            name: "USDC".to_string(),
            version: "2".to_string(),
        });
        let signers = voucher_signer
            .map(|signer| {
                vec![VoucherSignerFact {
                    network: NETWORK.to_string(),
                    signer: hex(&signer),
                }]
            })
            .unwrap_or_default();
        let served = document(
            None,
            Some("http://127.0.0.1:1/ilp"),
            Vec::new(),
            vec![published],
            signers,
        );
        let outbound = OutboundChannels::restore(
            Arc::new(InMemoryJournal::new()),
            vec![(
                SettlementChain::Evm,
                Arc::clone(&payer) as Arc<dyn BatchSettlementPayer>,
            )],
        )
        .await
        .expect("an empty journal replays");
        let connector = Connector::new(
            Vec::new(),
            Vec::new(),
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            Arc::new(TestClock::new(chrono::Utc::now())),
        )
        .with_self_description_source(Arc::new(FixedSelfDescription(served)))
        .with_peer_allow_plaintext_endpoints(true)
        .with_outbound_channels(
            Arc::new(outbound),
            vec![(SettlementChain::Evm, NETWORK.to_string())],
        );
        (connector, payer)
    }

    /// ADR 0075 decision 4: `POST /peers` opens and funds only this node's
    /// outbound channel, binds the counterparty's published voucher signer
    /// to the peering, and a repeat finds the same channel.
    #[tokio::test]
    async fn an_evm_peering_opens_this_nodes_own_channel_and_binds_the_peers_signer() {
        let signer = [0x02; 20];
        let (connector, payer) = paying_node(Some(signer)).await;

        let error = connector
            .establish_peering("node-b", &url(), 5, 0, None, None)
            .await
            .expect_err("no channel yet and no deposit named");
        assert!(
            matches!(error, EstablishPeeringError::DepositRequired { .. }),
            "{error:?}"
        );
        assert_eq!(payer.balance(), 10_000, "a refused write spends nothing");

        let established = connector
            .establish_peering("node-b", &url(), 5, 0, None, Some(1_000))
            .await
            .expect("establish");
        assert_eq!(established.channel.status, ChannelBranch::Created);
        assert_eq!(established.channel.chain, "evm");
        assert_eq!(
            payer.balance(),
            9_000,
            "the opening deposit is this node's own"
        );
        assert_eq!(
            connector.voucher_signer_peer(&VoucherSigner::Evm(signer)),
            Some("node-b".to_string()),
            "the counterparty's published signer proves this peering"
        );
        assert!(matches!(
            connector
                .runtime_peering("node-b")
                .expect("the row")
                .channels[..],
            [RuntimePeerChannel::EvmVoucher { .. }]
        ));
        connector
            .upsert_runtime_peer_route("g.example.peer", "node-b", connector_domain::Price::FREE)
            .expect("a peering paid over its own outbound channel is routable");

        let repeated = connector
            .establish_peering("node-b", &url(), 5, 0, None, Some(1_000))
            .await
            .expect("a repeat");
        assert_eq!(repeated.channel.status, ChannelBranch::Found);
        assert_eq!(repeated.channel.id, established.channel.id);
        assert_eq!(
            payer.balance(),
            9_000,
            "a repeat opens nothing and deposits nothing"
        );

        // Removing the peering stops signing on the channel and leaves it
        // open to withdraw from.
        connector
            .remove_runtime_peer_route("g.example.peer")
            .expect("route removed");
        connector
            .remove_runtime_peer("node-b")
            .expect("peering removed");
        assert_eq!(
            connector.voucher_signer_peer(&VoucherSigner::Evm(signer)),
            None
        );
        let outbound = connector.outbound_channels().expect("x402");
        assert!(outbound.knows(&established.channel.id));

        // A channel being wound down is not live: re-establishing the
        // peering after a withdrawal has started opens a fresh channel
        // rather than paying on one that backs nothing new.
        outbound
            .withdraw(&established.channel.id)
            .await
            .expect("start the withdrawal");
        let again = connector
            .establish_peering("node-b", &url(), 5, 0, None, Some(1_000))
            .await
            .expect("re-establish");
        assert_eq!(again.channel.status, ChannelBranch::Created);
        assert_ne!(again.channel.id, established.channel.id);
    }

    /// A counterparty that publishes no voucher signer could never have its
    /// channel toward this node bound, so the peering is refused before
    /// anything is spent; and a signer already proving another peering is
    /// refused the same way.
    #[tokio::test]
    async fn an_evm_peering_needs_a_published_signer_no_other_peering_holds() {
        let (connector, payer) = paying_node(None).await;
        let error = connector
            .establish_peering("node-b", &url(), 0, 0, None, Some(1_000))
            .await
            .expect_err("no voucher signer published");
        assert!(
            matches!(error, EstablishPeeringError::NoVoucherSigner { .. }),
            "{error:?}"
        );
        assert_eq!(payer.balance(), 10_000);

        let (connector, payer) = paying_node(Some([0x02; 20])).await;
        connector
            .establish_peering("node-b", &url(), 0, 0, None, Some(1_000))
            .await
            .expect("establish");
        let error = connector
            .establish_peering("node-b-again", &url(), 0, 0, None, Some(1_000))
            .await
            .expect_err("the signer already proves node-b");
        assert!(
            matches!(
                error,
                EstablishPeeringError::Binding(VoucherBindingError::SignerBoundElsewhere { .. })
            ),
            "{error:?}"
        );
        assert_eq!(payer.balance(), 9_000, "the refused write opened nothing");
    }
}
