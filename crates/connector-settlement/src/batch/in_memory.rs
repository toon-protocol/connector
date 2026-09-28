use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;

use super::port::{
    AdmissionRefusal, BatchChannelState, BatchChannelStatus, BatchSettlementBackend,
    BatchSettlementError, BatchSettlementPayer, ChannelPresentation, EvmChannelConfig,
    EvmReceiverTerms, OpenedChannel, OutboundChannelRecord, OutboundChannelState, ReceiverTerms,
    SolanaReceiverTerms, Voucher, VoucherSigner,
};
use crate::ChannelId;

/// How the payer of a channel on this fake leaves it, which is the one place
/// the two chains' lifecycles differ in a way the port can see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayerExit {
    /// EVM-shaped: `initiateWithdraw`. The channel goes `Withdrawing`, keeps
    /// accepting vouchers up to what the withdrawal leaves, and never seals;
    /// `finalizeWithdraw` leaves it `Open` again.
    /// Presentations are [`ChannelPresentation::Evm`], carrying a config.
    Withdrawal,
    /// Solana-shaped: `request_close`. The channel goes `Closing`, accepts
    /// no new voucher, and landing one seals it; `distribute` ends it.
    /// Presentations are [`ChannelPresentation::Solana`].
    Close,
}

/// The terms a client opens a channel on, in the vocabulary both chains
/// share. A fixture translates them into its chain's own fields: an EVM
/// `ChannelConfig`, a Solana `open`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelTerms {
    /// The opening deposit, in base units.
    pub deposit: u128,
    /// EVM `withdrawDelay`, Solana `grace_period`, in seconds.
    pub delay_secs: u64,
    /// Whether every seat that must be this node's is this node's: EVM
    /// `receiver` and `receiverAuthorizer`; Solana `payee`, `rent_payer`
    /// and the single-recipient distribution. `false` gives `receiver` /
    /// `payee` to someone else.
    pub pays_this_node: bool,
    /// Whether the channel holds the token this node settles in.
    pub in_settled_token: bool,
}

/// Every party on the fake chain is one byte, repeated to the width its
/// chain's keys have: an EVM address is `[party; 20]`, a Solana key
/// `[party; 32]`.
type Party = u8;

/// The node [`InMemoryBatchSettlement::new`] builds.
const THIS_NODE: Party = 0xaa;
const SOMEONE_ELSE: Party = 0xbb;
/// The token every node on the fake settles in, and one none does.
const SETTLED_TOKEN: Party = 0x70;
const OTHER_TOKEN: Party = 0x71;
/// The client the stand-in methods open channels as.
const CLIENT: Party = 0xcc;
/// The client's session key: ADR 0074 decision 6 makes a `payerAuthorizer`
/// the ordinary case, so the client's channels all name one.
const SESSION_KEY: Party = 0xdd;

struct FakeChannel {
    /// Who a refund goes to.
    payer: Party,
    /// EVM `receiver` and `receiverAuthorizer`; Solana `payee`, `rent_payer`
    /// and the one distribution recipient.
    receiver: Party,
    in_settled_token: bool,
    /// EVM `withdrawDelay`, Solana `grace_period`.
    delay_secs: u64,
    /// The config the channel was opened with, on an EVM-shaped chain.
    config: Option<EvmChannelConfig>,
    voucher_signer: VoucherSigner,
    deposit: u128,
    landed: u128,
    /// EVM-shaped: the amount a pending withdrawal will take.
    pending_withdrawal: u128,
    status: BatchChannelStatus,
    /// While the payer's withdrawal or close is pending: the chain time at
    /// which it may finish.
    exit_due_at: Option<u64>,
}

/// Everything a channel is opened with, whoever opens it.
struct Opening {
    payer: Party,
    /// Who signs the channel's vouchers: EVM `payerAuthorizer`, Solana
    /// `authorized_signer`.
    payer_authorizer: Party,
    receiver: Party,
    in_settled_token: bool,
    delay_secs: u64,
    deposit: u128,
}

struct Ledger {
    channels: HashMap<ChannelId, FakeChannel>,
    /// The index the next channel id is minted from: every channel opened,
    /// and every open prepared, takes one.
    next_index: usize,
    /// Each party's balance of the settled token, outside any channel.
    accounts: HashMap<Party, u128>,
    /// Seconds, moved only by [`InMemoryBatchChain::advance_time`].
    now: u64,
}

/// "The chain" several [`InMemoryBatchSettlement`] nodes share: channels,
/// the token balances of the accounts that pay into them, and a clock. It
/// is what lets one fake node's outbound channel be another's inbound one,
/// as a peering is (ADR 0075 decision 4).
pub struct InMemoryBatchChain {
    exit: PayerExit,
    ledger: Mutex<Ledger>,
}

impl InMemoryBatchChain {
    /// An empty chain whose payers leave by `exit`.
    pub fn new(exit: PayerExit) -> Arc<Self> {
        Arc::new(InMemoryBatchChain {
            exit,
            ledger: Mutex::new(Ledger {
                channels: HashMap::new(),
                next_index: 0,
                accounts: HashMap::new(),
                now: 0,
            }),
        })
    }

    /// Move the chain's clock forward, so a withdrawal's delay can run.
    pub fn advance_time(&self, secs: u64) {
        self.ledger().now += secs;
    }

    fn ledger(&self) -> MutexGuard<'_, Ledger> {
        self.ledger
            .lock()
            .expect("InMemoryBatchChain lock poisoned")
    }

    fn chain_name(&self) -> &'static str {
        match self.exit {
            PayerExit::Withdrawal => "evm",
            PayerExit::Close => "solana",
        }
    }

    /// Refuse anything for a chain other than this one.
    fn require(&self, chain: &'static str) -> Result<(), BatchSettlementError> {
        if chain == self.chain_name() {
            Ok(())
        } else {
            Err(BatchSettlementError::WrongChain {
                presented: chain,
                backend: self.chain_name(),
            })
        }
    }

    fn channel_id(&self, index: usize) -> ChannelId {
        match self.exit {
            PayerExit::Withdrawal => ChannelId(format!("0x{index:064x}")),
            PayerExit::Close => ChannelId(format!("batch-channel-{index}")),
        }
    }

    fn voucher_signer(&self, party: Party) -> VoucherSigner {
        match self.exit {
            PayerExit::Withdrawal => VoucherSigner::Evm([party; 20]),
            PayerExit::Close => VoucherSigner::Solana([party; 32]),
        }
    }

    /// Reserve the next channel id, and on an EVM-shaped chain the config
    /// it derives from, without putting anything on the chain: what a
    /// prepared open names before it is sent.
    fn reserve(
        &self,
        ledger: &mut Ledger,
        opening: &Opening,
    ) -> (ChannelId, Option<EvmChannelConfig>) {
        let index = ledger.next_index;
        ledger.next_index += 1;
        let id = self.channel_id(index);
        let config = match self.exit {
            PayerExit::Withdrawal => {
                let mut salt = [0u8; 32];
                salt[..8].copy_from_slice(&(index as u64).to_be_bytes());
                Some(EvmChannelConfig {
                    payer: [opening.payer; 20],
                    payer_authorizer: [opening.payer_authorizer; 20],
                    receiver: [opening.receiver; 20],
                    receiver_authorizer: [opening.receiver; 20],
                    token: [if opening.in_settled_token {
                        SETTLED_TOKEN
                    } else {
                        OTHER_TOKEN
                    }; 20],
                    withdraw_delay: opening.delay_secs,
                    salt,
                })
            }
            PayerExit::Close => None,
        };
        (id, config)
    }

    /// Put a channel on the chain and say how it is presented.
    fn create(&self, ledger: &mut Ledger, opening: Opening) -> OpenedChannel {
        let (id, config) = self.reserve(ledger, &opening);
        self.insert(ledger, id, config, opening)
    }

    /// Put the channel reserved as `id` on the chain.
    fn insert(
        &self,
        ledger: &mut Ledger,
        id: ChannelId,
        config: Option<EvmChannelConfig>,
        opening: Opening,
    ) -> OpenedChannel {
        let Opening {
            payer,
            payer_authorizer,
            receiver,
            in_settled_token,
            delay_secs,
            deposit,
        } = opening;
        let voucher_signer = self.voucher_signer(payer_authorizer);
        ledger.channels.insert(
            id.clone(),
            FakeChannel {
                payer,
                receiver,
                in_settled_token,
                delay_secs,
                config: config.clone(),
                voucher_signer,
                deposit,
                landed: 0,
                pending_withdrawal: 0,
                status: BatchChannelStatus::Open,
                exit_due_at: None,
            },
        );
        let presentation = match config {
            Some(config) => ChannelPresentation::Evm {
                channel: id,
                config,
            },
            None => ChannelPresentation::Solana { channel: id },
        };
        OpenedChannel {
            presentation,
            voucher_signer,
        }
    }

    /// The payer begins to leave with everything not yet landed: EVM
    /// `initiateWithdraw(balance − totalClaimed)`, Solana `request_close`.
    fn begin_exit(&self, stored: &mut FakeChannel, now: u64) {
        match self.exit {
            PayerExit::Withdrawal => {
                stored.pending_withdrawal = stored.deposit - stored.landed;
                stored.status = BatchChannelStatus::Withdrawing;
            }
            PayerExit::Close => stored.status = BatchChannelStatus::Closing,
        }
        stored.exit_due_at = Some(now + stored.delay_secs);
    }
}

/// Take `amount` out of `party`'s account, as a transaction paying into a
/// channel would, or refuse as the chain would.
fn debit(ledger: &mut Ledger, party: Party, amount: u128) -> Result<(), BatchSettlementError> {
    let balance = ledger.accounts.entry(party).or_default();
    if *balance < amount {
        return Err(BatchSettlementError::Backend(format!(
            "the settlement account holds {balance}, short of {amount}"
        )));
    }
    *balance -= amount;
    Ok(())
}

/// How long, in the fake chain's seconds, a prepared Solana-shaped open
/// stays sendable: its stand-in for a blockhash's lifetime.
const OPEN_LIFETIME_SECS: u64 = 90;

/// The fake's stand-in for a payer-signed `open`: the delay the payer built
/// it with and the sponsor's published minimum deposit.
fn fake_open_transaction(delay_secs: u64, min_deposit: u128) -> Vec<u8> {
    let mut bytes = delay_secs.to_be_bytes().to_vec();
    bytes.extend_from_slice(&min_deposit.to_be_bytes());
    bytes
}

fn parse_fake_open_transaction(bytes: &[u8]) -> Option<(u64, u128)> {
    let delay = u64::from_be_bytes(bytes.get(..8)?.try_into().ok()?);
    let min_deposit = u128::from_be_bytes(bytes.get(8..24)?.try_into().ok()?);
    Some((delay, min_deposit))
}

fn credit(ledger: &mut Ledger, party: Party, amount: u128) {
    *ledger.accounts.entry(party).or_default() += amount;
}

/// The in-memory batch-settlement backend: one node on an
/// [`InMemoryBatchChain`], implementing both halves of the port. As
/// receiver ([`BatchSettlementBackend`]) it admits and lands on channels
/// that pay it; as payer ([`BatchSettlementPayer`]) it opens, funds, signs
/// on and withdraws from channels toward another node on the same chain.
/// It is the fake this workspace's tests use, and the first implementation
/// to pass both of [`super::contract`]'s suites (ADR 0007), in both
/// [`PayerExit`] shapes.
///
/// A client paying this node is stood in for by the inherent methods
/// [`client_open`](Self::client_open), [`client_deposit`](Self::client_deposit)
/// and [`client_begin_exit`](Self::client_begin_exit). That client brings
/// its own money, which the fake does not count; only a node's paying half
/// moves a balance.
///
/// It verifies no signature, and the vouchers it signs carry none worth
/// verifying: the port lands vouchers already verified.
pub struct InMemoryBatchSettlement {
    chain: Arc<InMemoryBatchChain>,
    node: Party,
    minimum_delay_secs: u64,
    min_sponsored_deposit: u128,
    admitted: Mutex<HashSet<ChannelId>>,
    /// Every channel this node opened, with the highest amount it has
    /// signed on it.
    outbound: Mutex<HashMap<ChannelId, u128>>,
}

impl InMemoryBatchSettlement {
    /// A node alone on a chain of its own, whose payers leave by `exit`,
    /// admitting channels whose delay is at least `minimum_delay_secs`.
    pub fn new(exit: PayerExit, minimum_delay_secs: u64) -> Self {
        Self::on(InMemoryBatchChain::new(exit), THIS_NODE, minimum_delay_secs)
    }

    /// Node `party` on `chain`, admitting channels whose delay is at least
    /// `minimum_delay_secs`. `party` is the byte its keys repeat; two nodes
    /// on one chain need two, and neither may be one the fake reserves
    /// (`0xbb`, `0xcc`, `0xdd`, `0x70`, `0x71`).
    pub fn on(chain: Arc<InMemoryBatchChain>, party: u8, minimum_delay_secs: u64) -> Self {
        InMemoryBatchSettlement {
            chain,
            node: party,
            minimum_delay_secs,
            min_sponsored_deposit: 0,
            admitted: Mutex::new(HashSet::new()),
            outbound: Mutex::new(HashMap::new()),
        }
    }

    /// Publish, and have this node's sponsor enforce, a minimum opening
    /// deposit (Solana's `min_sponsored_deposit`). Meaningless on an
    /// EVM-shaped chain, which has no sponsor.
    pub fn with_min_sponsored_deposit(mut self, minimum: u128) -> Self {
        self.min_sponsored_deposit = minimum;
        self
    }

    /// Credit this node's settlement account with `amount` of the settled
    /// token, as a faucet or a mint would.
    pub fn fund(&self, amount: u128) {
        credit(&mut self.chain.ledger(), self.node, amount);
    }

    /// This node's settlement account's balance of the settled token.
    pub fn balance(&self) -> u128 {
        self.chain
            .ledger()
            .accounts
            .get(&self.node)
            .copied()
            .unwrap_or(0)
    }

    /// What this node publishes about the channels it receives on: the
    /// terms another node opens toward it under (ADR 0075 decision 3).
    pub fn published_terms(&self) -> ReceiverTerms {
        match self.chain.exit {
            PayerExit::Withdrawal => ReceiverTerms::Evm(EvmReceiverTerms {
                receiver: [self.node; 20],
                token: [SETTLED_TOKEN; 20],
                min_withdraw_delay_secs: self.minimum_delay_secs,
            }),
            PayerExit::Close => ReceiverTerms::Solana(SolanaReceiverTerms {
                sponsor: [self.node; 32],
                receiver: [self.node; 32],
                mint: [SETTLED_TOKEN; 32],
                min_grace_period_secs: self.minimum_delay_secs,
                min_deposit: self.min_sponsored_deposit,
                sponsor_endpoint: format!(
                    "https://node-{:02x}.example/ilp/batch-settlement/solana/open",
                    self.node
                ),
            }),
        }
    }

    /// Stand in for a client opening and funding a channel toward this
    /// node on `terms`, always as the same payer. Not on the port: on a
    /// real chain this is the client's own transaction.
    pub fn client_open(&self, terms: ChannelTerms) -> OpenedChannel {
        let chain = &self.chain;
        let mut ledger = chain.ledger();
        let receiver = if terms.pays_this_node {
            self.node
        } else {
            SOMEONE_ELSE
        };
        chain.create(
            &mut ledger,
            Opening {
                payer: CLIENT,
                payer_authorizer: SESSION_KEY,
                receiver,
                in_settled_token: terms.in_settled_token,
                delay_secs: terms.delay_secs,
                deposit: terms.deposit,
            },
        )
    }

    /// A presentation, for this fake's chain, of a channel nobody opened.
    pub fn unopened_presentation(&self) -> ChannelPresentation {
        let channel = ChannelId(format!("unopened-{}", self.chain.chain_name()));
        match self.chain.exit {
            PayerExit::Withdrawal => ChannelPresentation::Evm {
                channel,
                config: EvmChannelConfig {
                    payer: [CLIENT; 20],
                    payer_authorizer: [SESSION_KEY; 20],
                    receiver: [self.node; 20],
                    receiver_authorizer: [self.node; 20],
                    token: [SETTLED_TOKEN; 20],
                    withdraw_delay: self.minimum_delay_secs,
                    salt: [0xff; 32],
                },
            },
            PayerExit::Close => ChannelPresentation::Solana { channel },
        }
    }

    /// Stand in for the client adding `amount` to the deposit (EVM
    /// `deposit`, Solana `top_up`).
    pub fn client_deposit(&self, channel: &ChannelId, amount: u128) {
        let mut ledger = self.chain.ledger();
        let stored = ledger
            .channels
            .get_mut(channel)
            .expect("deposit into an opened channel");
        stored.deposit += amount;
    }

    /// Stand in for the client beginning to leave with everything not yet
    /// landed: EVM `initiateWithdraw(balance − totalClaimed)`, Solana
    /// `request_close`.
    pub fn client_begin_exit(&self, channel: &ChannelId) {
        let mut ledger = self.chain.ledger();
        let now = ledger.now;
        let stored = ledger
            .channels
            .get_mut(channel)
            .expect("exit an opened channel");
        self.chain.begin_exit(stored, now);
    }

    fn state(&self, id: &ChannelId, stored: &FakeChannel) -> BatchChannelState {
        let collateral = if stored.status.accepts_vouchers() {
            stored
                .deposit
                .saturating_sub(stored.landed)
                .saturating_sub(stored.pending_withdrawal)
        } else {
            0
        };
        BatchChannelState {
            id: id.clone(),
            status: stored.status,
            voucher_signer: stored.voucher_signer,
            landed: stored.landed,
            collateral,
        }
    }

    fn judge(&self, stored: &FakeChannel) -> Option<AdmissionRefusal> {
        if stored.receiver != self.node {
            return Some(AdmissionRefusal::NotPayableToThisNode {
                field: match self.chain.exit {
                    PayerExit::Withdrawal => "receiver",
                    PayerExit::Close => "payee",
                },
            });
        }
        if !stored.in_settled_token {
            return Some(AdmissionRefusal::TokenNotSettled);
        }
        if stored.delay_secs < self.minimum_delay_secs {
            return Some(AdmissionRefusal::DelayBelowMinimum {
                delay_secs: stored.delay_secs,
                minimum_secs: self.minimum_delay_secs,
            });
        }
        if self.chain.exit == PayerExit::Close && stored.status != BatchChannelStatus::Open {
            return Some(AdmissionRefusal::NotOpen);
        }
        None
    }

    fn admitted(&self) -> MutexGuard<'_, HashSet<ChannelId>> {
        self.admitted
            .lock()
            .expect("InMemoryBatchSettlement lock poisoned")
    }

    fn outbound(&self) -> MutexGuard<'_, HashMap<ChannelId, u128>> {
        self.outbound
            .lock()
            .expect("InMemoryBatchSettlement lock poisoned")
    }

    fn require_admitted(&self, channel: &ChannelId) -> Result<(), BatchSettlementError> {
        if self.admitted().contains(channel) {
            Ok(())
        } else {
            Err(BatchSettlementError::ChannelNotAdmitted(channel.clone()))
        }
    }

    /// The highest amount this node has signed on `channel`, if it opened
    /// it.
    fn signed(&self, channel: &ChannelId) -> Result<u128, BatchSettlementError> {
        self.outbound()
            .get(channel)
            .copied()
            .ok_or_else(|| BatchSettlementError::NotOutbound(channel.clone()))
    }

    fn outbound_state_of(
        &self,
        ledger: &Ledger,
        channel: &ChannelId,
        signed: u128,
    ) -> Result<OutboundChannelState, BatchSettlementError> {
        let stored = ledger
            .channels
            .get(channel)
            .ok_or_else(|| BatchSettlementError::ChannelNotFound(channel.clone()))?;
        Ok(OutboundChannelState {
            on_chain: self.state(channel, stored),
            signed,
        })
    }

    /// The channel `presentation` names, once it is shown to be for this
    /// chain and to name itself consistently: what both
    /// [`admit`](BatchSettlementBackend::admit) and
    /// [`restore`](BatchSettlementBackend::restore) check before anything
    /// else.
    fn locate(
        &self,
        presentation: &ChannelPresentation,
    ) -> Result<ChannelId, BatchSettlementError> {
        self.chain.require(presentation.chain())?;
        let id = presentation.channel().clone();
        let ledger = self.chain.ledger();
        if let ChannelPresentation::Evm { config, .. } = presentation {
            // The real backend hashes the config and compares. The fake has
            // no hash, so a config derives the id of the channel opened
            // under it, and a config nothing was opened under derives an id
            // that is not the presented one whenever the presented one
            // exists (it was opened under a different config).
            let derived = ledger
                .channels
                .iter()
                .find(|(_, stored)| stored.config.as_ref() == Some(config))
                .map(|(derived, _)| derived.clone());
            let mismatch = match &derived {
                Some(derived) => *derived != id,
                None => ledger.channels.contains_key(&id),
            };
            if mismatch {
                return Err(BatchSettlementError::ChannelIdMismatch {
                    presented: id,
                    derived: derived
                        .unwrap_or_else(|| ChannelId("an unopened channel".to_string())),
                });
            }
        }
        Ok(id)
    }
}

#[async_trait]
impl BatchSettlementBackend for InMemoryBatchSettlement {
    async fn admit(
        &self,
        presentation: ChannelPresentation,
    ) -> Result<BatchChannelState, BatchSettlementError> {
        let id = self.locate(&presentation)?;
        let ledger = self.chain.ledger();
        let stored = ledger
            .channels
            .get(&id)
            .ok_or_else(|| BatchSettlementError::ChannelNotFound(id.clone()))?;
        if let Some(refusal) = self.judge(stored) {
            return Err(BatchSettlementError::NotAdmissible {
                channel: id,
                refusal,
            });
        }
        let state = self.state(&id, stored);
        drop(ledger);
        self.admitted().insert(id);
        Ok(state)
    }

    async fn restore(
        &self,
        presentation: ChannelPresentation,
    ) -> Result<BatchChannelState, BatchSettlementError> {
        let id = self.locate(&presentation)?;
        let ledger = self.chain.ledger();
        let stored = ledger
            .channels
            .get(&id)
            .ok_or_else(|| BatchSettlementError::ChannelNotFound(id.clone()))?;
        let state = self.state(&id, stored);
        drop(ledger);
        self.admitted().insert(id);
        Ok(state)
    }

    async fn channel_state(
        &self,
        channel: &ChannelId,
    ) -> Result<BatchChannelState, BatchSettlementError> {
        self.require_admitted(channel)?;
        let ledger = self.chain.ledger();
        let stored = ledger
            .channels
            .get(channel)
            .ok_or_else(|| BatchSettlementError::ChannelNotFound(channel.clone()))?;
        Ok(self.state(channel, stored))
    }

    async fn land(
        &self,
        channel: &ChannelId,
        voucher: Voucher,
    ) -> Result<BatchChannelState, BatchSettlementError> {
        self.require_admitted(channel)?;
        let mut ledger = self.chain.ledger();
        let stored = ledger
            .channels
            .get_mut(channel)
            .ok_or_else(|| BatchSettlementError::ChannelNotFound(channel.clone()))?;
        if stored.status == BatchChannelStatus::Sealed {
            return Err(BatchSettlementError::ChannelSealed(channel.clone()));
        }
        if voucher.cumulative_amount <= stored.landed {
            return Err(BatchSettlementError::StaleVoucher {
                amount: voucher.cumulative_amount,
                landed: stored.landed,
            });
        }
        if voucher.cumulative_amount > stored.deposit {
            return Err(BatchSettlementError::VoucherExceedsDeposit {
                amount: voucher.cumulative_amount,
                deposited: stored.deposit,
            });
        }
        stored.landed = voucher.cumulative_amount;
        if stored.status == BatchChannelStatus::Closing {
            // `settle_and_seal`: the only way to land on a closing Solana
            // channel, and terminal.
            stored.status = BatchChannelStatus::Sealed;
        }
        let stored = &*stored;
        Ok(self.state(channel, stored))
    }
}

#[async_trait]
impl BatchSettlementPayer for InMemoryBatchSettlement {
    async fn prepare_open(
        &self,
        terms: ReceiverTerms,
        deposit: u128,
    ) -> Result<OutboundChannelRecord, BatchSettlementError> {
        let chain = &self.chain;
        chain.require(terms.chain())?;
        let (token, receiver) = match &terms {
            ReceiverTerms::Evm(evm) => (evm.token[0], evm.receiver[0]),
            ReceiverTerms::Solana(solana) => (solana.mint[0], solana.receiver[0]),
        };
        if token != SETTLED_TOKEN {
            return Err(BatchSettlementError::TokenNotShared);
        }
        let opening = Opening {
            payer: self.node,
            // The settlement key signs the vouchers too: on EVM
            // `payerAuthorizer == payer` (ADR 0075 decision 3).
            payer_authorizer: self.node,
            receiver,
            in_settled_token: true,
            delay_secs: terms.min_delay_secs(),
            deposit,
        };
        let mut ledger = chain.ledger();
        let (channel, config) = chain.reserve(&mut ledger, &opening);
        Ok(match (terms, config) {
            // The config names the receiver's whole address, as a real one
            // does: the fake keys its party by the first byte, and a caller
            // that looks the channel up by the receiver it opened toward
            // must find it by that address.
            (ReceiverTerms::Evm(evm), Some(config)) => OutboundChannelRecord::Evm {
                channel,
                config: EvmChannelConfig {
                    receiver: evm.receiver,
                    receiver_authorizer: evm.receiver,
                    ..config
                },
                deposit,
            },
            (ReceiverTerms::Solana(solana), _) => OutboundChannelRecord::Solana {
                channel,
                receiver: solana.receiver,
                sponsor_endpoint: solana.sponsor_endpoint,
                deposit,
                // The fake's own "signed open": what the chain needs to
                // create the channel, since the fake parses no transaction.
                transaction: fake_open_transaction(opening.delay_secs, solana.min_deposit),
                last_valid_block_height: ledger.now + OPEN_LIFETIME_SECS,
            },
            (ReceiverTerms::Evm(_), None) => unreachable!("an EVM-shaped chain reserves a config"),
        })
    }

    async fn open_prepared(
        &self,
        record: &OutboundChannelRecord,
    ) -> Result<OpenedChannel, BatchSettlementError> {
        let chain = &self.chain;
        chain.require(record.chain())?;
        let channel = record.channel().clone();
        let deposit = record.deposit();
        let mut ledger = chain.ledger();
        // Already on chain: adopted as it stands, nothing sent.
        if let Some(stored) = ledger.channels.get(&channel) {
            let presentation = record.presentation();
            let voucher_signer = stored.voucher_signer;
            drop(ledger);
            self.outbound().entry(channel).or_insert(0);
            return Ok(OpenedChannel {
                presentation,
                voucher_signer,
            });
        }
        let (config, receiver, delay_secs) = match record {
            OutboundChannelRecord::Evm { config, .. } => (
                Some(config.clone()),
                config.receiver[0],
                config.withdraw_delay,
            ),
            OutboundChannelRecord::Solana {
                receiver,
                transaction,
                last_valid_block_height,
                ..
            } => {
                if ledger.now > *last_valid_block_height {
                    return Err(BatchSettlementError::OpenLapsed(channel));
                }
                let (delay_secs, min_deposit) = parse_fake_open_transaction(transaction)
                    .ok_or_else(|| {
                        BatchSettlementError::Backend("not a fake open transaction".to_string())
                    })?;
                // The sponsor co-signs only at or above its published
                // minimum, and says so by name (ADR 0074 decision 5).
                if deposit < min_deposit {
                    return Err(BatchSettlementError::OpenRefused(format!(
                        "deposit_below_minimum: deposit is {deposit}; the sponsor co-signs an \
                         open only at or above {min_deposit}"
                    )));
                }
                (None, receiver[0], delay_secs)
            }
        };
        debit(&mut ledger, self.node, deposit)?;
        let opened = chain.insert(
            &mut ledger,
            channel.clone(),
            config,
            Opening {
                payer: self.node,
                payer_authorizer: self.node,
                receiver,
                in_settled_token: true,
                delay_secs,
                deposit,
            },
        );
        drop(ledger);
        self.outbound().insert(channel, 0);
        Ok(opened)
    }

    async fn restore_outbound(
        &self,
        record: &OutboundChannelRecord,
        signed: u128,
    ) -> Result<OutboundChannelState, BatchSettlementError> {
        self.chain.require(record.chain())?;
        let channel = record.channel().clone();
        let ledger = self.chain.ledger();
        let stored = ledger
            .channels
            .get(&channel)
            .ok_or_else(|| BatchSettlementError::ChannelNotFound(channel.clone()))?;
        if stored.payer != self.node {
            return Err(BatchSettlementError::NotOutbound(channel));
        }
        let landed = stored.landed;
        let mut outbound = self.outbound();
        let watermark = outbound.entry(channel.clone()).or_insert(0);
        *watermark = (*watermark).max(signed).max(landed);
        let watermark = *watermark;
        drop(outbound);
        self.outbound_state_of(&ledger, &channel, watermark)
    }

    async fn top_up(
        &self,
        channel: &ChannelId,
        increment: u128,
    ) -> Result<OutboundChannelState, BatchSettlementError> {
        let signed = self.signed(channel)?;
        let mut ledger = self.chain.ledger();
        let sealed = ledger
            .channels
            .get(channel)
            .is_some_and(|stored| stored.status == BatchChannelStatus::Sealed);
        if sealed {
            return Err(BatchSettlementError::ChannelSealed(channel.clone()));
        }
        debit(&mut ledger, self.node, increment)?;
        if let Some(stored) = ledger.channels.get_mut(channel) {
            stored.deposit += increment;
        }
        self.outbound_state_of(&ledger, channel, signed)
    }

    async fn sign_voucher(
        &self,
        channel: &ChannelId,
        cumulative_amount: u128,
    ) -> Result<Voucher, BatchSettlementError> {
        let mut outbound = self.outbound();
        let signed = outbound
            .get_mut(channel)
            .ok_or_else(|| BatchSettlementError::NotOutbound(channel.clone()))?;
        if cumulative_amount <= *signed {
            return Err(BatchSettlementError::VoucherNotAdvancing {
                amount: cumulative_amount,
                signed: *signed,
            });
        }
        let backed = self
            .outbound_state_of(&self.chain.ledger(), channel, *signed)?
            .on_chain
            .voucher_ceiling();
        if cumulative_amount > backed {
            return Err(BatchSettlementError::VoucherUnbacked {
                amount: cumulative_amount,
                backed,
            });
        }
        *signed = cumulative_amount;
        Ok(Voucher {
            cumulative_amount,
            signature: match self.chain.exit {
                PayerExit::Withdrawal => vec![self.node; 65],
                PayerExit::Close => vec![self.node; 64],
            },
        })
    }

    async fn start_withdrawal(
        &self,
        channel: &ChannelId,
    ) -> Result<OutboundChannelState, BatchSettlementError> {
        let signed = self.signed(channel)?;
        let mut ledger = self.chain.ledger();
        let now = ledger.now;
        let stored = ledger
            .channels
            .get_mut(channel)
            .ok_or_else(|| BatchSettlementError::ChannelNotFound(channel.clone()))?;
        if stored.exit_due_at.is_none() {
            if stored.status == BatchChannelStatus::Sealed {
                return Err(BatchSettlementError::ChannelSealed(channel.clone()));
            }
            self.chain.begin_exit(stored, now);
        }
        self.outbound_state_of(&ledger, channel, signed)
    }

    async fn finish_withdrawal(
        &self,
        channel: &ChannelId,
    ) -> Result<OutboundChannelState, BatchSettlementError> {
        let signed = self.signed(channel)?;
        let mut ledger = self.chain.ledger();
        let now = ledger.now;
        let stored = ledger
            .channels
            .get_mut(channel)
            .ok_or_else(|| BatchSettlementError::ChannelNotFound(channel.clone()))?;
        let due_at = stored
            .exit_due_at
            .ok_or_else(|| BatchSettlementError::NoWithdrawalPending(channel.clone()))?;
        // A Solana channel the receiver has sealed may be distributed at
        // once; otherwise the delay must have run.
        let sealed = stored.status == BatchChannelStatus::Sealed;
        if now < due_at && !sealed {
            return Err(BatchSettlementError::WithdrawalNotDue {
                channel: channel.clone(),
                remaining_secs: due_at - now,
            });
        }
        let unlanded = stored.deposit - stored.landed;
        let (payer, receiver, landed) = (stored.payer, stored.receiver, stored.landed);
        let (refund, paid_out) = match self.chain.exit {
            // `finalizeWithdraw`: whatever the withdrawal asked for, it
            // takes nothing the receiver landed inside the delay.
            PayerExit::Withdrawal => {
                let refund = stored.pending_withdrawal.min(unlanded);
                stored.deposit -= refund;
                stored.pending_withdrawal = 0;
                stored.status = BatchChannelStatus::Open;
                (refund, 0)
            }
            // `distribute`: the receiver's landed share to it, the rest back
            // to the payer, and the channel is over.
            PayerExit::Close => {
                stored.status = BatchChannelStatus::Sealed;
                (unlanded, landed)
            }
        };
        stored.exit_due_at = None;
        credit(&mut ledger, payer, refund);
        credit(&mut ledger, receiver, paid_out);
        self.outbound_state_of(&ledger, channel, signed)
    }

    async fn outbound_state(
        &self,
        channel: &ChannelId,
    ) -> Result<OutboundChannelState, BatchSettlementError> {
        let signed = self.signed(channel)?;
        self.outbound_state_of(&self.chain.ledger(), channel, signed)
    }

    /// A stand-in signature, as [`Self::sign_voucher`]'s is: this node's
    /// byte, at the chain's signature length, with the expiry's low byte in
    /// the last position so two expiries never sign alike. Nothing verifies
    /// it; the fake holds only which channels this node opened.
    async fn sign_claim_state_challenge(
        &self,
        channel: &ChannelId,
        expires: u64,
    ) -> Result<Vec<u8>, BatchSettlementError> {
        self.signed(channel)?;
        let mut signature = match self.chain.exit {
            PayerExit::Withdrawal => vec![self.node; 65],
            PayerExit::Close => vec![self.node; 64],
        };
        if let Some(last) = signature.last_mut() {
            *last = expires.to_le_bytes()[0];
        }
        Ok(signature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE_DAY: u64 = 86_400;

    fn admissible() -> ChannelTerms {
        ChannelTerms {
            deposit: 1_000,
            delay_secs: ONE_DAY,
            pays_this_node: true,
            in_settled_token: true,
        }
    }

    fn voucher(cumulative_amount: u128) -> Voucher {
        Voucher {
            cumulative_amount,
            signature: vec![0u8; 64],
        }
    }

    /// Solana: once the payer asks to close, only `settle_and_seal` can
    /// land, and it is terminal -- nothing lands after it.
    #[tokio::test]
    async fn landing_on_a_closing_channel_seals_it() {
        let fake = InMemoryBatchSettlement::new(PayerExit::Close, ONE_DAY);
        let opened = fake.client_open(admissible());
        let channel = opened.presentation.channel().clone();
        fake.admit(opened.presentation).await.expect("admit");

        fake.client_begin_exit(&channel);
        let state = fake.channel_state(&channel).await.expect("state");
        assert_eq!(state.status, BatchChannelStatus::Closing);
        assert!(!state.status.accepts_vouchers());

        let state = fake.land(&channel, voucher(400)).await.expect("seal");
        assert_eq!(state.status, BatchChannelStatus::Sealed);
        assert_eq!(state.landed, 400);
        assert_eq!(state.collateral, 0);

        let err = fake.land(&channel, voucher(500)).await.unwrap_err();
        assert_eq!(err, BatchSettlementError::ChannelSealed(channel));
    }

    /// Solana: a channel already closing is refused at admission. It can
    /// back no new voucher, so there is nothing to admit it for.
    #[tokio::test]
    async fn a_closing_channel_is_not_admitted() {
        let fake = InMemoryBatchSettlement::new(PayerExit::Close, ONE_DAY);
        let opened = fake.client_open(admissible());
        fake.client_begin_exit(opened.presentation.channel());

        let err = fake.admit(opened.presentation.clone()).await.unwrap_err();
        assert_eq!(
            err,
            BatchSettlementError::NotAdmissible {
                channel: opened.presentation.channel().clone(),
                refusal: AdmissionRefusal::NotOpen,
            }
        );
    }

    /// EVM: a withdrawal is not an end. The channel still accepts vouchers
    /// up to what the withdrawal leaves, and landing does not seal it.
    #[tokio::test]
    async fn a_withdrawing_channel_still_accepts_and_never_seals() {
        let fake = InMemoryBatchSettlement::new(PayerExit::Withdrawal, ONE_DAY);
        let opened = fake.client_open(admissible());
        let channel = opened.presentation.channel().clone();
        fake.admit(opened.presentation).await.expect("admit");

        fake.client_begin_exit(&channel);
        let state = fake.land(&channel, voucher(400)).await.expect("claim");
        assert_eq!(state.status, BatchChannelStatus::Withdrawing);
        assert!(state.status.accepts_vouchers());
        assert_eq!(state.landed, 400);
        // The withdrawal took everything unlanded, which was all 1000; the
        // claim that landed first leaves collateral saturated at zero.
        assert_eq!(state.collateral, 0);

        // A fresh deposit during the withdrawal is collateral again.
        fake.client_deposit(&channel, 250);
        let state = fake.channel_state(&channel).await.expect("state");
        assert_eq!(
            state.collateral, 0,
            "1250 - 400 landed - 1000 pending saturates"
        );
        fake.client_deposit(&channel, 500);
        let state = fake.channel_state(&channel).await.expect("state");
        assert_eq!(state.collateral, 350);
        assert_eq!(state.voucher_ceiling(), 750);
    }

    /// EVM: the connector recomputes the id from the presented config and
    /// refuses a config that does not derive the id it came with.
    #[tokio::test]
    async fn an_evm_config_that_derives_another_id_is_refused() {
        let fake = InMemoryBatchSettlement::new(PayerExit::Withdrawal, ONE_DAY);
        let first = fake.client_open(admissible());
        let second = fake.client_open(admissible());
        let (ChannelPresentation::Evm { channel, .. }, ChannelPresentation::Evm { config, .. }) =
            (first.presentation, second.presentation.clone())
        else {
            panic!("an EVM-shaped fake presents EVM channels");
        };

        let err = fake
            .admit(ChannelPresentation::Evm {
                channel: channel.clone(),
                config,
            })
            .await
            .unwrap_err();
        assert_eq!(
            err,
            BatchSettlementError::ChannelIdMismatch {
                presented: channel.clone(),
                derived: second.presentation.channel().clone(),
            }
        );
        let err = fake.channel_state(&channel).await.unwrap_err();
        assert_eq!(err, BatchSettlementError::ChannelNotAdmitted(channel));
    }

    #[tokio::test]
    async fn a_presentation_for_the_other_chain_is_refused() {
        let evm = InMemoryBatchSettlement::new(PayerExit::Withdrawal, ONE_DAY);
        let solana = InMemoryBatchSettlement::new(PayerExit::Close, ONE_DAY);

        let err = evm.admit(solana.unopened_presentation()).await.unwrap_err();
        assert_eq!(
            err,
            BatchSettlementError::WrongChain {
                presented: "solana",
                backend: "evm",
            }
        );
        let err = solana.admit(evm.unopened_presentation()).await.unwrap_err();
        assert_eq!(
            err,
            BatchSettlementError::WrongChain {
                presented: "evm",
                backend: "solana",
            }
        );
    }

    /// Two nodes on one chain, each funded.
    fn peers(exit: PayerExit) -> (InMemoryBatchSettlement, InMemoryBatchSettlement) {
        let chain = InMemoryBatchChain::new(exit);
        let a = InMemoryBatchSettlement::on(Arc::clone(&chain), 0x01, ONE_DAY);
        let b = InMemoryBatchSettlement::on(chain, 0x02, ONE_DAY).with_min_sponsored_deposit(500);
        a.fund(10_000);
        b.fund(10_000);
        (a, b)
    }

    /// EVM: the channel a node opens names its settlement key twice, as
    /// payer and as voucher signer (ADR 0075 decision 3), and the
    /// counterparty in both receiving seats.
    #[tokio::test]
    async fn an_evm_channel_this_node_opens_names_its_settlement_key_as_payer_authorizer() {
        let (a, b) = peers(PayerExit::Withdrawal);
        let opened = a.open(b.published_terms(), 1_000).await.expect("open");
        let ChannelPresentation::Evm { config, .. } = opened.presentation else {
            panic!("an EVM-shaped fake presents EVM channels");
        };
        assert_eq!(config.payer, [0x01; 20]);
        assert_eq!(config.payer_authorizer, config.payer);
        assert_eq!(config.receiver, [0x02; 20]);
        assert_eq!(config.receiver_authorizer, config.receiver);
        assert_eq!(config.withdraw_delay, ONE_DAY);
        assert_eq!(opened.voucher_signer, VoucherSigner::Evm([0x01; 20]));
    }

    /// Solana: the sponsor co-signs no open below its published minimum
    /// deposit, and the payer surfaces its refusal by name (ADR 0074
    /// decision 5). Nothing is opened, and nothing spent.
    #[tokio::test]
    async fn an_open_below_the_sponsors_minimum_is_refused_by_name() {
        let (a, b) = peers(PayerExit::Close);
        let err = a.open(b.published_terms(), 499).await.unwrap_err();
        assert!(
            matches!(&err, BatchSettlementError::OpenRefused(reason) if reason.starts_with("deposit_below_minimum")),
            "{err:?}"
        );
        assert_eq!(a.balance(), 10_000);
        a.open(b.published_terms(), 500)
            .await
            .expect("an open at the minimum is sponsored");
    }

    /// Solana: a close the receiver has sealed by landing may be
    /// distributed at once, without waiting out the grace period, and pays
    /// each side its share.
    #[tokio::test]
    async fn a_sealed_channel_is_distributed_without_waiting_for_the_grace_period() {
        let (a, b) = peers(PayerExit::Close);
        let opened = a.open(b.published_terms(), 1_000).await.expect("open");
        let channel = opened.presentation.channel().clone();
        b.admit(opened.presentation).await.expect("admit");
        let voucher = a.sign_voucher(&channel, 250).await.expect("sign");

        a.start_withdrawal(&channel).await.expect("request_close");
        b.land(&channel, voucher).await.expect("settle_and_seal");
        let state = a.finish_withdrawal(&channel).await.expect("distribute");
        assert_eq!(state.on_chain.status, BatchChannelStatus::Sealed);
        assert_eq!(a.balance(), 10_000 - 250);
        assert_eq!(b.balance(), 10_000 + 250);
    }

    /// Solana: a close the receiver never seals is distributed once the
    /// grace period has run, and not a second before; the payer gets back
    /// everything the receiver did not land.
    #[tokio::test]
    async fn an_unsealed_close_is_distributed_once_the_grace_period_runs() {
        let chain = InMemoryBatchChain::new(PayerExit::Close);
        let a = InMemoryBatchSettlement::on(Arc::clone(&chain), 0x01, ONE_DAY);
        let b = InMemoryBatchSettlement::on(Arc::clone(&chain), 0x02, ONE_DAY);
        a.fund(10_000);
        let opened = a.open(b.published_terms(), 1_000).await.expect("open");
        let channel = opened.presentation.channel().clone();
        b.admit(opened.presentation).await.expect("admit");
        let voucher = a.sign_voucher(&channel, 300).await.expect("sign");
        b.land(&channel, voucher).await.expect("settle while open");

        a.start_withdrawal(&channel).await.expect("request_close");
        chain.advance_time(ONE_DAY - 1);
        assert_eq!(
            a.finish_withdrawal(&channel).await.unwrap_err(),
            BatchSettlementError::WithdrawalNotDue {
                channel: channel.clone(),
                remaining_secs: 1,
            }
        );
        chain.advance_time(1);
        let state = a.finish_withdrawal(&channel).await.expect("distribute");
        assert_eq!(state.on_chain.status, BatchChannelStatus::Sealed);
        assert_eq!(state.on_chain.landed, 300);
        assert_eq!(a.balance(), 10_000 - 300);
        assert_eq!(b.balance(), 300);
    }

    /// Solana: a prepared open the chain never took, once its blockhash has
    /// expired, never will. The record opens nothing and spends nothing, and
    /// says so by name, so the caller can abandon it and open afresh.
    #[tokio::test]
    async fn a_prepared_solana_open_lapses_once_its_blockhash_expires() {
        let chain = InMemoryBatchChain::new(PayerExit::Close);
        let a = InMemoryBatchSettlement::on(Arc::clone(&chain), 0x01, ONE_DAY);
        let b = InMemoryBatchSettlement::on(Arc::clone(&chain), 0x02, ONE_DAY);
        a.fund(10_000);
        let record = a
            .prepare_open(b.published_terms(), 1_000)
            .await
            .expect("prepare");
        chain.advance_time(OPEN_LIFETIME_SECS + 1);
        assert_eq!(
            a.open_prepared(&record).await.unwrap_err(),
            BatchSettlementError::OpenLapsed(record.channel().clone())
        );
        assert_eq!(a.balance(), 10_000);
    }

    /// EVM: an open prepared long ago is still sendable, since its opening
    /// deposit never expires.
    #[tokio::test]
    async fn a_prepared_evm_open_never_lapses() {
        let (a, b) = peers(PayerExit::Withdrawal);
        let record = a
            .prepare_open(b.published_terms(), 1_000)
            .await
            .expect("prepare");
        a.chain.advance_time(10 * ONE_DAY);
        a.open_prepared(&record).await.expect("still opens");
        assert_eq!(a.balance(), 9_000);
    }

    /// A node cannot pay more into a channel than its account holds.
    #[tokio::test]
    async fn a_node_cannot_open_beyond_its_balance() {
        let (a, b) = peers(PayerExit::Withdrawal);
        let err = a.open(b.published_terms(), 10_001).await.unwrap_err();
        assert!(matches!(err, BatchSettlementError::Backend(_)), "{err:?}");
        assert_eq!(a.balance(), 10_000);
    }
}
