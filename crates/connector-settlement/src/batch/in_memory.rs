use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard};

use async_trait::async_trait;

use super::port::{
    AdmissionRefusal, BatchChannelState, BatchChannelStatus, BatchSettlementBackend,
    BatchSettlementError, ChannelPresentation, EvmChannelConfig, Voucher, VoucherSigner,
};
use crate::port::ChannelId;

/// How the payer of a channel on this fake leaves it, which is the one place
/// the two chains' lifecycles differ in a way the port can see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayerExit {
    /// EVM-shaped: `initiateWithdraw`. The channel goes `Withdrawing`, keeps
    /// accepting vouchers up to what the withdrawal leaves, and never seals.
    /// Presentations are [`ChannelPresentation::Evm`], carrying a config.
    Withdrawal,
    /// Solana-shaped: `request_close`. The channel goes `Closing`, accepts
    /// no new voucher, and landing one seals it. Presentations are
    /// [`ChannelPresentation::Solana`].
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

/// A channel the payer opened, as a client would present it, and the signer
/// the chain recorded for it.
#[derive(Debug, Clone)]
pub struct OpenedChannel {
    pub presentation: ChannelPresentation,
    pub voucher_signer: VoucherSigner,
}

const THIS_NODE: [u8; 20] = [0xaa; 20];
const SOMEONE_ELSE: [u8; 20] = [0xbb; 20];
const SETTLED_TOKEN: [u8; 20] = [0x70; 20];
const OTHER_TOKEN: [u8; 20] = [0x71; 20];
const PAYER: [u8; 20] = [0xcc; 20];
/// The payer's session key: ADR 0074 decision 6 makes a `payerAuthorizer`
/// the ordinary case, so the fake's channels all name one.
const SESSION_KEY: [u8; 20] = [0xdd; 20];
const SOLANA_SIGNER: [u8; 32] = [0xdd; 32];

struct FakeChannel {
    terms: ChannelTerms,
    /// The config the channel was opened with, on an EVM-shaped fake.
    config: Option<EvmChannelConfig>,
    deposit: u128,
    landed: u128,
    /// EVM-shaped: the amount a pending withdrawal will take.
    pending_withdrawal: u128,
    status: BatchChannelStatus,
}

/// The in-memory [`BatchSettlementBackend`]: "the chain" is a map in this
/// process, and the client's own transactions are the inherent methods
/// [`open`](Self::open), [`deposit`](Self::deposit) and
/// [`begin_exit`](Self::begin_exit). It is the fake this workspace's tests
/// use, and the first implementation to pass [`super::contract`] (ADR
/// 0007), in both [`PayerExit`] shapes.
///
/// It verifies no signature: the port lands vouchers already verified.
pub struct InMemoryBatchSettlement {
    exit: PayerExit,
    minimum_delay_secs: u64,
    chain: Mutex<HashMap<ChannelId, FakeChannel>>,
    admitted: Mutex<HashSet<ChannelId>>,
}

impl InMemoryBatchSettlement {
    /// A fake whose payers leave by `exit`, admitting channels whose delay
    /// is at least `minimum_delay_secs`.
    pub fn new(exit: PayerExit, minimum_delay_secs: u64) -> Self {
        InMemoryBatchSettlement {
            exit,
            minimum_delay_secs,
            chain: Mutex::new(HashMap::new()),
            admitted: Mutex::new(HashSet::new()),
        }
    }

    fn chain_name(&self) -> &'static str {
        match self.exit {
            PayerExit::Withdrawal => "evm",
            PayerExit::Close => "solana",
        }
    }

    fn chain(&self) -> MutexGuard<'_, HashMap<ChannelId, FakeChannel>> {
        self.chain
            .lock()
            .expect("InMemoryBatchSettlement lock poisoned")
    }

    fn admitted(&self) -> MutexGuard<'_, HashSet<ChannelId>> {
        self.admitted
            .lock()
            .expect("InMemoryBatchSettlement lock poisoned")
    }

    fn channel_id(&self, index: usize) -> ChannelId {
        match self.exit {
            PayerExit::Withdrawal => ChannelId(format!("0x{index:064x}")),
            PayerExit::Close => ChannelId(format!("batch-channel-{index}")),
        }
    }

    fn presentation(
        &self,
        channel: ChannelId,
        config: Option<EvmChannelConfig>,
    ) -> ChannelPresentation {
        match config {
            Some(config) => ChannelPresentation::Evm { channel, config },
            None => ChannelPresentation::Solana { channel },
        }
    }

    /// Stand in for a client opening and funding a channel on `terms`,
    /// always as the same payer. Not on the port: on a real chain this is
    /// the client's own transaction.
    pub fn open(&self, terms: ChannelTerms) -> OpenedChannel {
        let mut chain = self.chain();
        let id = self.channel_id(chain.len());
        let (config, voucher_signer) = match self.exit {
            PayerExit::Withdrawal => {
                let mut salt = [0u8; 32];
                salt[..8].copy_from_slice(&(chain.len() as u64).to_be_bytes());
                let receiver = if terms.pays_this_node {
                    THIS_NODE
                } else {
                    SOMEONE_ELSE
                };
                let config = EvmChannelConfig {
                    payer: PAYER,
                    payer_authorizer: SESSION_KEY,
                    receiver,
                    receiver_authorizer: receiver,
                    token: if terms.in_settled_token {
                        SETTLED_TOKEN
                    } else {
                        OTHER_TOKEN
                    },
                    withdraw_delay: terms.delay_secs,
                    salt,
                };
                (Some(config), VoucherSigner::Evm(SESSION_KEY))
            }
            PayerExit::Close => (None, VoucherSigner::Solana(SOLANA_SIGNER)),
        };
        chain.insert(
            id.clone(),
            FakeChannel {
                terms,
                config: config.clone(),
                deposit: terms.deposit,
                landed: 0,
                pending_withdrawal: 0,
                status: BatchChannelStatus::Open,
            },
        );
        OpenedChannel {
            presentation: self.presentation(id, config),
            voucher_signer,
        }
    }

    /// A presentation, for this fake's chain, of a channel nobody opened.
    pub fn unopened_presentation(&self) -> ChannelPresentation {
        let channel = ChannelId(format!("unopened-{}", self.chain_name()));
        match self.exit {
            PayerExit::Withdrawal => ChannelPresentation::Evm {
                channel,
                config: EvmChannelConfig {
                    payer: PAYER,
                    payer_authorizer: SESSION_KEY,
                    receiver: THIS_NODE,
                    receiver_authorizer: THIS_NODE,
                    token: SETTLED_TOKEN,
                    withdraw_delay: self.minimum_delay_secs,
                    salt: [0xff; 32],
                },
            },
            PayerExit::Close => ChannelPresentation::Solana { channel },
        }
    }

    /// Stand in for the payer adding `amount` to the deposit (EVM `deposit`,
    /// Solana `top_up`).
    pub fn deposit(&self, channel: &ChannelId, amount: u128) {
        let mut chain = self.chain();
        let stored = chain
            .get_mut(channel)
            .expect("deposit into an opened channel");
        stored.deposit += amount;
    }

    /// Stand in for the payer beginning to leave with everything not yet
    /// landed: EVM `initiateWithdraw(balance − totalClaimed)`, Solana
    /// `request_close`.
    pub fn begin_exit(&self, channel: &ChannelId) {
        let mut chain = self.chain();
        let stored = chain.get_mut(channel).expect("exit an opened channel");
        match self.exit {
            PayerExit::Withdrawal => {
                stored.pending_withdrawal = stored.deposit - stored.landed;
                stored.status = BatchChannelStatus::Withdrawing;
            }
            PayerExit::Close => stored.status = BatchChannelStatus::Closing,
        }
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
            voucher_signer: match self.exit {
                PayerExit::Withdrawal => VoucherSigner::Evm(SESSION_KEY),
                PayerExit::Close => VoucherSigner::Solana(SOLANA_SIGNER),
            },
            landed: stored.landed,
            collateral,
        }
    }

    fn judge(&self, stored: &FakeChannel) -> Option<AdmissionRefusal> {
        if !stored.terms.pays_this_node {
            return Some(AdmissionRefusal::NotPayableToThisNode {
                field: match self.exit {
                    PayerExit::Withdrawal => "receiver",
                    PayerExit::Close => "payee",
                },
            });
        }
        if !stored.terms.in_settled_token {
            return Some(AdmissionRefusal::TokenNotSettled);
        }
        if stored.terms.delay_secs < self.minimum_delay_secs {
            return Some(AdmissionRefusal::DelayBelowMinimum {
                delay_secs: stored.terms.delay_secs,
                minimum_secs: self.minimum_delay_secs,
            });
        }
        if self.exit == PayerExit::Close && stored.status != BatchChannelStatus::Open {
            return Some(AdmissionRefusal::NotOpen);
        }
        None
    }

    fn require_admitted(&self, channel: &ChannelId) -> Result<(), BatchSettlementError> {
        if self.admitted().contains(channel) {
            Ok(())
        } else {
            Err(BatchSettlementError::ChannelNotAdmitted(channel.clone()))
        }
    }
}

#[async_trait]
impl BatchSettlementBackend for InMemoryBatchSettlement {
    async fn admit(
        &self,
        presentation: ChannelPresentation,
    ) -> Result<BatchChannelState, BatchSettlementError> {
        let id = self.locate(&presentation)?;
        let chain = self.chain();
        let stored = chain
            .get(&id)
            .ok_or_else(|| BatchSettlementError::ChannelNotFound(id.clone()))?;
        if let Some(refusal) = self.judge(stored) {
            return Err(BatchSettlementError::NotAdmissible {
                channel: id,
                refusal,
            });
        }
        let state = self.state(&id, stored);
        drop(chain);
        self.admitted().insert(id);
        Ok(state)
    }

    async fn restore(
        &self,
        presentation: ChannelPresentation,
    ) -> Result<BatchChannelState, BatchSettlementError> {
        let id = self.locate(&presentation)?;
        let chain = self.chain();
        let stored = chain
            .get(&id)
            .ok_or_else(|| BatchSettlementError::ChannelNotFound(id.clone()))?;
        let state = self.state(&id, stored);
        drop(chain);
        self.admitted().insert(id);
        Ok(state)
    }

    async fn channel_state(
        &self,
        channel: &ChannelId,
    ) -> Result<BatchChannelState, BatchSettlementError> {
        self.require_admitted(channel)?;
        let chain = self.chain();
        let stored = chain
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
        let mut chain = self.chain();
        let stored = chain
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

impl InMemoryBatchSettlement {
    /// The channel `presentation` names, once it is shown to be for this
    /// chain and to name itself consistently: what both
    /// [`admit`](BatchSettlementBackend::admit) and
    /// [`restore`](BatchSettlementBackend::restore) check before anything
    /// else.
    fn locate(
        &self,
        presentation: &ChannelPresentation,
    ) -> Result<ChannelId, BatchSettlementError> {
        if presentation.chain() != self.chain_name() {
            return Err(BatchSettlementError::WrongChain {
                presented: presentation.chain(),
                backend: self.chain_name(),
            });
        }
        let id = presentation.channel().clone();
        let chain = self.chain();
        if let ChannelPresentation::Evm { config, .. } = presentation {
            // The real backend hashes the config and compares. The fake has
            // no hash, so a config derives the id of the channel opened
            // under it, and a config nothing was opened under derives an id
            // that is not the presented one whenever the presented one
            // exists (it was opened under a different config).
            let derived = chain
                .iter()
                .find(|(_, stored)| stored.config.as_ref() == Some(config))
                .map(|(derived, _)| derived.clone());
            let mismatch = match &derived {
                Some(derived) => *derived != id,
                None => chain.contains_key(&id),
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
        let opened = fake.open(admissible());
        let channel = opened.presentation.channel().clone();
        fake.admit(opened.presentation).await.expect("admit");

        fake.begin_exit(&channel);
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
        let opened = fake.open(admissible());
        fake.begin_exit(opened.presentation.channel());

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
        let opened = fake.open(admissible());
        let channel = opened.presentation.channel().clone();
        fake.admit(opened.presentation).await.expect("admit");

        fake.begin_exit(&channel);
        let state = fake.land(&channel, voucher(400)).await.expect("claim");
        assert_eq!(state.status, BatchChannelStatus::Withdrawing);
        assert!(state.status.accepts_vouchers());
        assert_eq!(state.landed, 400);
        // The withdrawal took everything unlanded, which was all 1000; the
        // claim that landed first leaves collateral saturated at zero.
        assert_eq!(state.collateral, 0);

        // A fresh deposit during the withdrawal is collateral again.
        fake.deposit(&channel, 250);
        let state = fake.channel_state(&channel).await.expect("state");
        assert_eq!(
            state.collateral, 0,
            "1250 - 400 landed - 1000 pending saturates"
        );
        fake.deposit(&channel, 500);
        let state = fake.channel_state(&channel).await.expect("state");
        assert_eq!(state.collateral, 350);
        assert_eq!(state.voucher_ceiling(), 750);
    }

    /// EVM: the connector recomputes the id from the presented config and
    /// refuses a config that does not derive the id it came with.
    #[tokio::test]
    async fn an_evm_config_that_derives_another_id_is_refused() {
        let fake = InMemoryBatchSettlement::new(PayerExit::Withdrawal, ONE_DAY);
        let first = fake.open(admissible());
        let second = fake.open(admissible());
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
}
