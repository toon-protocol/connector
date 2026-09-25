//! The Solana half of ADR 0074 decision 5 (issue #1344): the watcher that
//! carries every channel this node sponsors through `payment-channels`'
//! lifecycle, from Open to reclaimed rent.
//!
//! - **Open.** Settle the latest held voucher, on the sweep's cadence. Not
//!   what protects this node -- as `payee` it can always still land a voucher
//!   once a payer asks to close -- but what keeps what is unlanded small.
//! - **Closing.** The payer called `request_close`; only this node, as
//!   `payee`, can land a voucher now, with `settle_and_seal`, and only before
//!   `closure_started_at + grace_period`. The watcher does that on its next
//!   tick. With nothing left to land it seals at once instead, so the payer's
//!   refund and this node's rent do not wait out the grace period.
//! - **Sealed.** `distribute`: this node's share to its receiving account, the
//!   rest to the payer, the escrow's rent home.
//! - **Distributed.** `reclaim` the channel's own rent once the program's slot
//!   window has passed.
//!
//! Which channels those are is **rediscovered** from the chain every tick,
//! with `getProgramAccounts` filtered to `dataSize` 256 and `rent_payer` at
//! offset 216 equal to the sponsor key (decision 5). The journal is not the
//! index: a channel the node sponsored but never took a voucher on still
//! holds its rent float, and one that went Closing while the node was down
//! must still be sealed. None of this needs a channel to have been admitted
//! in this process -- admission refuses a Closing channel -- so it reads the
//! accounts directly.
//!
//! What to do with each channel is [`next_step`], a pure function of the
//! account, the voucher held on it, the chain's clock and whether the sweep
//! is due. Everything else here is the shell that reads and sends.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use connector_chain_rpc::retry_read;
use connector_settlement::batch::{BatchSettlementError, ChannelPresentation, HeldVouchers};
use connector_settlement::ChannelId;
use solana_account_decoder_client_types::UiAccountEncoding;
use solana_rpc_client_api::config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_rpc_client_api::filter::{Memcmp, RpcFilterType};
use solana_sdk::clock::Clock;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer;

use super::{backend_error, wire, SolanaBatchSettlement};

/// How often the watcher reads every sponsored channel. The grace period is
/// at least this node's published minimum -- a day by default, never below
/// x402's 900 seconds (ADR 0074 decision 5) -- so ten seconds leaves the
/// Closing watch ninety chances inside even the shortest one, for one
/// filtered `getProgramAccounts` per tick.
pub const CLOSING_WATCH_INTERVAL: Duration = Duration::from_secs(10);

/// How often an Open channel's latest voucher is settled. The EVM sweep's
/// cadence, for the same reason: a transaction per busy channel every ten
/// minutes, not per voucher.
pub const OPEN_SETTLE_INTERVAL: Duration = Duration::from_secs(600);

/// Most `reclaim`s one transaction carries: two accounts each, no signer.
const MAX_RECLAIMS_PER_TRANSACTION: usize = 10;

/// What the watcher does next with one sponsored channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Nothing, this tick.
    Wait,
    /// Open: `settle` the held voucher for `amount`.
    Settle { amount: u64 },
    /// Closing, inside the grace period: land the held voucher for `amount`
    /// with `settle_and_seal`, sealing the channel.
    SettleAndSeal { amount: u64 },
    /// Closing, inside the grace period, nothing held above what is already
    /// settled: seal now with `settle_and_seal` and no voucher.
    SealEarly,
    /// Closing, the grace period over: the permissionless `seal` crank.
    /// `missed` is a held voucher that can no longer land -- what the watcher
    /// exists to prevent, and the one step that loses money.
    Seal { missed: Option<u64> },
    /// Sealed: pay the channel out.
    Distribute,
    /// Distributed, past the slot window: bring the channel's rent home.
    Reclaim,
    /// A channel naming this node as `rent_payer` whose `payee` or
    /// distribution is not this node's, which its sponsor endpoint never
    /// co-signs. There is nothing this node can land or pay out on it.
    NotOurs,
}

/// The next [`Step`] for `account`, a channel whose `rent_payer` is
/// `sponsor`. `held` is the highest voucher amount this node holds on it,
/// already checked to be the channel's signer's; `now` and `slot` are the
/// chain's clock; `settle_open` is whether the Open-channel sweep is due.
///
/// A held voucher is landable only above `settled` and at most `deposit`:
/// the program refuses anything else, and a voucher above the deposit is one
/// the claim gate would never have accepted.
pub fn next_step(
    account: &wire::ChannelAccount,
    sponsor: &Pubkey,
    held: Option<u64>,
    now: i64,
    slot: u64,
    settle_open: bool,
) -> Step {
    if account.status == wire::ChannelStatus::Distributed {
        let unlocked = account.open_slot.saturating_add(wire::OPEN_SLOT_WINDOW);
        return if slot > unlocked {
            Step::Reclaim
        } else {
            Step::Wait
        };
    }
    if account.payee != *sponsor
        || account.distribution_hash != wire::distribution_hash(&wire::sole_recipient(sponsor))
    {
        return Step::NotOurs;
    }
    let landable = held.filter(|amount| *amount > account.settled && *amount <= account.deposit);
    match account.status {
        wire::ChannelStatus::Open => match landable {
            Some(amount) if settle_open => Step::Settle { amount },
            _ => Step::Wait,
        },
        wire::ChannelStatus::Closing => {
            let deadline = account
                .closure_started_at
                .saturating_add(i64::from(account.grace_period));
            match (now < deadline, landable) {
                (true, Some(amount)) => Step::SettleAndSeal { amount },
                (true, None) => Step::SealEarly,
                (false, missed) => Step::Seal { missed },
            }
        }
        wire::ChannelStatus::Sealed => Step::Distribute,
        wire::ChannelStatus::Distributed => unreachable!("answered above"),
    }
}

/// A sponsored channel, as `getProgramAccounts` found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SponsoredChannel {
    pub address: Pubkey,
    pub account: wire::ChannelAccount,
}

impl SolanaBatchSettlement {
    /// Every channel of this program whose `rent_payer` is this node's
    /// sponsor key, read from the chain now (ADR 0074 decision 5). Includes
    /// Distributed channels still holding their rent; a channel `distribute`
    /// or `reclaim` deallocated is gone.
    pub async fn sponsored_channels(&self) -> Result<Vec<SponsoredChannel>, BatchSettlementError> {
        let sponsor = self.sponsor.pubkey();
        let config = RpcProgramAccountsConfig {
            filters: Some(vec![
                RpcFilterType::DataSize(wire::CHANNEL_ACCOUNT_LEN as u64),
                RpcFilterType::Memcmp(Memcmp::new_base58_encoded(
                    wire::RENT_PAYER_OFFSET,
                    sponsor.as_ref(),
                )),
            ]),
            account_config: RpcAccountInfoConfig {
                // A 256-byte account does not fit the default base58.
                encoding: Some(UiAccountEncoding::Base64),
                commitment: Some(CommitmentConfig::confirmed()),
                ..RpcAccountInfoConfig::default()
            },
            with_context: None,
            sort_results: None,
        };
        let accounts = retry_read(|| {
            self.rpc
                .get_program_accounts_with_config(&self.program_id, config.clone())
        })
        .await
        .map_err(backend_error)?;
        Ok(accounts
            .into_iter()
            .filter_map(|(address, account)| {
                let account = wire::ChannelAccount::parse(&account.data)?;
                // Bytes only describe a channel at the address they derive.
                (account.derive_address(&self.program_id) == address)
                    .then_some(SponsoredChannel { address, account })
            })
            .collect())
    }

    /// The chain's clock: its slot and unix time, from one read.
    pub async fn clock(&self) -> Result<Clock, BatchSettlementError> {
        let clock = solana_sdk::sysvar::clock::id();
        let account = retry_read(|| self.rpc.get_account(&clock))
            .await
            .map_err(backend_error)?;
        solana_sdk::account::from_account::<Clock, _>(&account).ok_or_else(|| {
            BatchSettlementError::Backend("the clock sysvar did not decode".to_string())
        })
    }

    /// Take `step` on `channel`: `voucher` is the held voucher's signature
    /// for the steps that land one. Distribution needs a treasury owner the
    /// deployment was built with; `treasury` is tried first when known.
    async fn take(
        &self,
        channel: &SponsoredChannel,
        step: Step,
        signature: Option<[u8; 64]>,
        treasury: &Mutex<Option<Pubkey>>,
    ) -> Result<(), BatchSettlementError> {
        let address = &channel.address;
        let account = &channel.account;
        let voucher = |amount: u64| {
            signature
                .map(|signature| (amount, signature))
                .ok_or_else(|| {
                    BatchSettlementError::Backend(format!(
                        "no voucher signature held for {amount} on '{address}'"
                    ))
                })
        };
        match step {
            Step::Wait | Step::NotOurs | Step::Reclaim => Ok(()),
            Step::Settle { amount } => {
                let (amount, signature) = voucher(amount)?;
                self.submit(&wire::settle_instructions(
                    &self.program_id,
                    address,
                    &account.authorized_signer,
                    amount,
                    &signature,
                ))
                .await
            }
            Step::SettleAndSeal { amount } => {
                let (amount, signature) = voucher(amount)?;
                self.submit(&wire::settle_and_seal_instructions(
                    &self.program_id,
                    &self.sponsor.pubkey(),
                    address,
                    &account.authorized_signer,
                    amount,
                    &signature,
                ))
                .await
            }
            Step::SealEarly => {
                self.submit(&[wire::seal_without_voucher_instruction(
                    &self.program_id,
                    &self.sponsor.pubkey(),
                    address,
                )])
                .await
            }
            Step::Seal { .. } => {
                self.submit(&[wire::seal_instruction(&self.program_id, address)])
                    .await
            }
            Step::Distribute => self.distribute(channel, treasury).await,
        }
    }

    /// `distribute` a Sealed channel. Creates this node's receiving account
    /// and the treasury's, idempotently, in the same transaction: an unusable
    /// receiving account forfeits this node's share to the treasury, and a
    /// missing treasury account refuses the whole instruction.
    ///
    /// The treasury owner is not derivable from the program id, which is the
    /// same on every cluster, so each known one is tried in turn; a wrong one
    /// fails the preflight simulation and costs nothing. The one that works
    /// is remembered in `treasury`.
    async fn distribute(
        &self,
        channel: &SponsoredChannel,
        treasury: &Mutex<Option<Pubkey>>,
    ) -> Result<(), BatchSettlementError> {
        let account = &channel.account;
        let mint = retry_read(|| self.rpc.get_account(&account.mint))
            .await
            .map_err(backend_error)?;
        let token_program = mint.owner;
        let sponsor = self.sponsor.pubkey();
        let recipients = wire::sole_recipient(&self.receiver());
        let known = *treasury.lock().expect("treasury lock poisoned");
        let candidates: Vec<Pubkey> = known
            .into_iter()
            .chain(
                wire::treasury_owner_candidates()
                    .into_iter()
                    .filter(|candidate| Some(*candidate) != known),
            )
            .collect();
        let mut last = None;
        for owner in candidates {
            let instructions = [
                spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                    &sponsor,
                    &self.receiver(),
                    &account.mint,
                    &token_program,
                ),
                spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                    &sponsor,
                    &owner,
                    &account.mint,
                    &token_program,
                ),
                wire::distribute_instruction(
                    &self.program_id,
                    &channel.address,
                    account,
                    &token_program,
                    &owner,
                    &recipients,
                ),
            ];
            match self.submit(&instructions).await {
                Ok(()) => {
                    *treasury.lock().expect("treasury lock poisoned") = Some(owner);
                    return Ok(());
                }
                Err(error) => last = Some(error),
            }
        }
        Err(last.expect("at least one treasury owner is tried"))
    }

    /// `reclaim` every one of `channels`, several to a transaction.
    async fn reclaim(&self, channels: &[&SponsoredChannel]) -> Result<(), BatchSettlementError> {
        for batch in channels.chunks(MAX_RECLAIMS_PER_TRANSACTION) {
            let instructions: Vec<Instruction> = batch
                .iter()
                .map(|channel| {
                    wire::reclaim_instruction(
                        &self.program_id,
                        &channel.address,
                        &channel.account.rent_payer,
                    )
                })
                .collect();
            self.submit(&instructions).await?;
        }
        Ok(())
    }
}

/// A held voucher on a Solana channel, as the watcher lands it.
struct Held {
    amount: u64,
    signature: [u8; 64],
}

/// The Closing watcher and Open-channel sweep over one
/// [`SolanaBatchSettlement`], reading the vouchers to land from `held` (the
/// client edge's claim gate, in a running node). [`run`](Self::run) is what
/// the runtime spawns; [`tick`](Self::tick) is one pass, public so a test can
/// drive it.
pub struct SolanaBatchWatcher {
    backend: Arc<SolanaBatchSettlement>,
    held: Arc<dyn HeldVouchers>,
    /// The treasury owner this deployment's `distribute` accepted, once one
    /// has.
    treasury: Mutex<Option<Pubkey>>,
}

impl SolanaBatchWatcher {
    pub fn new(backend: Arc<SolanaBatchSettlement>, held: Arc<dyn HeldVouchers>) -> Self {
        SolanaBatchWatcher {
            backend,
            held,
            treasury: Mutex::new(None),
        }
    }

    /// One pass over every sponsored channel: decide each one's [`Step`] and
    /// take it. `settle_open` is whether Open channels are settled this
    /// pass. Returns the steps taken, other than [`Step::Wait`]; a step
    /// that failed is logged, left out and tried again next pass.
    pub async fn tick(
        &self,
        settle_open: bool,
    ) -> Result<Vec<(ChannelId, Step)>, BatchSettlementError> {
        let channels = self.backend.sponsored_channels().await?;
        if channels.is_empty() {
            return Ok(Vec::new());
        }
        let clock = self.backend.clock().await?;
        let held = self.held();
        let sponsor = self.backend.sponsor();

        let mut taken = Vec::new();
        let mut reclaimable = Vec::new();
        for channel in &channels {
            let voucher = held.get(&channel.address).filter(|held| {
                let genuine = connector_signer::verify_solana_voucher(
                    &channel.address.to_bytes(),
                    held.amount,
                    0,
                    &held.signature,
                    &channel.account.authorized_signer.to_bytes(),
                );
                if !genuine {
                    tracing::warn!(
                        channel = %channel.address,
                        amount = held.amount,
                        "a held batch-settlement voucher is not its channel's signer's; not \
                         landing it"
                    );
                }
                genuine
            });
            let step = next_step(
                &channel.account,
                &sponsor,
                voucher.map(|held| held.amount),
                clock.unix_timestamp,
                clock.slot,
                settle_open,
            );
            match step {
                Step::Wait => continue,
                Step::NotOurs => {
                    tracing::debug!(
                        channel = %channel.address,
                        "a channel this node pays rent on does not pay this node; leaving it"
                    );
                    continue;
                }
                Step::Reclaim => {
                    reclaimable.push(channel);
                    continue;
                }
                Step::Seal {
                    missed: Some(amount),
                } => tracing::error!(
                    channel = %channel.address,
                    amount,
                    settled = channel.account.settled,
                    "a batch-settlement channel's grace period ended before its latest voucher \
                     was landed; sealing at what is settled"
                ),
                _ => {}
            }
            match self
                .backend
                .take(
                    channel,
                    step,
                    voucher.map(|held| held.signature),
                    &self.treasury,
                )
                .await
            {
                Ok(()) => taken.push((ChannelId(channel.address.to_string()), step)),
                Err(error) => tracing::warn!(
                    channel = %channel.address,
                    ?step,
                    %error,
                    "a batch-settlement step failed; retrying next pass"
                ),
            }
        }
        if !reclaimable.is_empty() {
            match self.backend.reclaim(&reclaimable).await {
                Ok(()) => taken.extend(
                    reclaimable
                        .iter()
                        .map(|channel| (ChannelId(channel.address.to_string()), Step::Reclaim)),
                ),
                Err(error) => tracing::warn!(
                    %error,
                    channels = reclaimable.len(),
                    "reclaiming batch-settlement channel rent failed; retrying next pass"
                ),
            }
        }
        Ok(taken)
    }

    /// Pass every `watch_every`, settling Open channels on the passes at
    /// least `settle_every` apart, the first included, for the life of the
    /// process. A pass that fails is logged and retried; nothing here ever
    /// returns.
    pub async fn run(self, watch_every: Duration, settle_every: Duration) {
        let mut watch = tokio::time::interval(watch_every);
        watch.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_settle: Option<tokio::time::Instant> = None;
        loop {
            let now = watch.tick().await;
            let settle_open = last_settle.is_none_or(|last| now - last >= settle_every);
            match self.tick(settle_open).await {
                Ok(taken) => {
                    if settle_open {
                        last_settle = Some(now);
                    }
                    for (channel, step) in taken {
                        tracing::info!(%channel, ?step, "batch-settlement channel step taken");
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "batch-settlement channel watch failed; retrying");
                }
            }
        }
    }

    /// The held vouchers on Solana channels, by channel account.
    ///
    /// A held voucher that cannot be put in the program's terms is an
    /// invariant breach, never a quiet skip: the claim gate accepts only
    /// a `u64` amount (`connector-domain`'s voucher parser refuses a wider
    /// one) and a 64-byte signature, so either failing here means a voucher
    /// this node was paid with cannot be landed. It is logged as an error,
    /// naming the channel, and left out.
    fn held(&self) -> HashMap<Pubkey, Held> {
        self.held
            .held_vouchers()
            .into_iter()
            .filter_map(|entry| {
                let ChannelPresentation::Solana { channel } = &entry.presentation else {
                    return None;
                };
                let amount = entry.voucher.cumulative_amount;
                let landable = Pubkey::from_str(&channel.0).ok().zip(
                    u64::try_from(amount)
                        .ok()
                        .zip(<[u8; 64]>::try_from(entry.voucher.signature.as_slice()).ok()),
                );
                let Some((address, (amount, signature))) = landable else {
                    tracing::error!(
                        %channel,
                        amount,
                        signature_len = entry.voucher.signature.len(),
                        "a held batch-settlement voucher is not in payment-channels' terms (a \
                         base58 channel, a u64 amount, a 64-byte signature); it cannot be landed"
                    );
                    return None;
                };
                Some((address, Held { amount, signature }))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRACE: u32 = 86_400;
    const CLOSED_AT: i64 = 1_000_000;

    fn ours(sponsor: Pubkey, status: wire::ChannelStatus) -> wire::ChannelAccount {
        wire::ChannelAccount {
            bump: 255,
            status,
            salt: 0,
            deposit: 1_000,
            settled: 300,
            payout_watermark: 0,
            closure_started_at: if status == wire::ChannelStatus::Closing {
                CLOSED_AT
            } else {
                0
            },
            payer_withdrawn_at: 0,
            grace_period: GRACE,
            distribution_hash: wire::distribution_hash(&wire::sole_recipient(&sponsor)),
            payer: Pubkey::new_unique(),
            payee: sponsor,
            authorized_signer: Pubkey::new_unique(),
            mint: Pubkey::new_unique(),
            rent_payer: sponsor,
            open_slot: 5_000,
        }
    }

    proptest::proptest! {
        /// Whatever the clock and the voucher: a step that lands a voucher
        /// lands one the program would take, above `settled` and within the
        /// deposit; and a Closing channel with such a voucher inside its
        /// grace period is always sealed **with** it, never without.
        #[test]
        fn a_landing_step_lands_a_landable_voucher_and_never_skips_one_in_time(
            status in 0u8..4,
            deposit in 1u64..10_000,
            settled_part in 0u64..10_000,
            held in proptest::option::of(0u64..20_000),
            now_offset in -100_000i64..200_000,
            settle_open: bool,
        ) {
            let sponsor = Pubkey::new_from_array([7; 32]);
            let status = match status {
                0 => wire::ChannelStatus::Open,
                1 => wire::ChannelStatus::Closing,
                2 => wire::ChannelStatus::Sealed,
                _ => wire::ChannelStatus::Distributed,
            };
            let account = wire::ChannelAccount {
                deposit,
                settled: settled_part.min(deposit),
                ..ours(sponsor, status)
            };
            let now = CLOSED_AT + now_offset;
            let step = next_step(&account, &sponsor, held, now, 0, settle_open);
            if let Step::Settle { amount } | Step::SettleAndSeal { amount } = step {
                proptest::prop_assert!(amount > account.settled && amount <= account.deposit);
                proptest::prop_assert_eq!(Some(amount), held);
            }
            let landable = held.is_some_and(|amount| amount > account.settled && amount <= deposit);
            let in_grace = now < CLOSED_AT + i64::from(GRACE);
            if status == wire::ChannelStatus::Closing && landable && in_grace {
                proptest::prop_assert_eq!(step, Step::SettleAndSeal { amount: held.unwrap() });
            }
        }
    }

    #[test]
    fn an_open_channel_is_settled_only_when_the_sweep_is_due_and_a_voucher_is_new() {
        let sponsor = Pubkey::new_unique();
        let open = ours(sponsor, wire::ChannelStatus::Open);
        let step = |held, due| next_step(&open, &sponsor, held, 0, 0, due);
        assert_eq!(step(Some(500), true), Step::Settle { amount: 500 });
        assert_eq!(step(Some(500), false), Step::Wait);
        assert_eq!(step(Some(300), true), Step::Wait, "already settled");
        assert_eq!(step(Some(1_001), true), Step::Wait, "above the deposit");
        assert_eq!(step(None, true), Step::Wait);
    }

    /// ADR 0074 decision 5: once a payer asks to close, only this node can
    /// land its voucher, and only before the grace period ends.
    #[test]
    fn a_closing_channel_is_sealed_with_its_voucher_inside_the_grace_period() {
        let sponsor = Pubkey::new_unique();
        let closing = ours(sponsor, wire::ChannelStatus::Closing);
        let deadline = CLOSED_AT + i64::from(GRACE);
        let step = |held, now| next_step(&closing, &sponsor, held, now, 0, false);

        // Due or not, the Closing watch does not wait for the sweep.
        assert_eq!(
            step(Some(700), CLOSED_AT),
            Step::SettleAndSeal { amount: 700 }
        );
        assert_eq!(
            step(Some(700), deadline - 1),
            Step::SettleAndSeal { amount: 700 }
        );
        assert_eq!(step(None, CLOSED_AT), Step::SealEarly);
        assert_eq!(
            step(Some(300), CLOSED_AT),
            Step::SealEarly,
            "nothing new to land"
        );

        assert_eq!(step(Some(700), deadline), Step::Seal { missed: Some(700) });
        assert_eq!(step(None, deadline), Step::Seal { missed: None });
    }

    #[test]
    fn a_sealed_channel_is_distributed_and_a_distributed_one_reclaimed_after_the_window() {
        let sponsor = Pubkey::new_unique();
        let sealed = ours(sponsor, wire::ChannelStatus::Sealed);
        assert_eq!(
            next_step(&sealed, &sponsor, Some(900), 0, 0, false),
            Step::Distribute
        );

        let distributed = ours(sponsor, wire::ChannelStatus::Distributed);
        let unlocked = distributed.open_slot + wire::OPEN_SLOT_WINDOW;
        let at = |slot| next_step(&distributed, &sponsor, None, 0, slot, true);
        assert_eq!(
            at(unlocked),
            Step::Wait,
            "the program requires slot > open_slot + K"
        );
        assert_eq!(at(unlocked + 1), Step::Reclaim);
    }

    /// A channel this node pays rent on but that does not pay it -- which
    /// its sponsor endpoint never co-signs -- is left alone, except that its
    /// rent still comes home.
    #[test]
    fn a_channel_that_does_not_pay_this_node_is_not_ours_to_land_on() {
        let sponsor = Pubkey::new_unique();
        let someone = Pubkey::new_unique();
        for status in [
            wire::ChannelStatus::Open,
            wire::ChannelStatus::Closing,
            wire::ChannelStatus::Sealed,
        ] {
            let other_payee = wire::ChannelAccount {
                payee: someone,
                ..ours(sponsor, status)
            };
            let other_split = wire::ChannelAccount {
                distribution_hash: wire::distribution_hash(&wire::sole_recipient(&someone)),
                ..ours(sponsor, status)
            };
            for account in [other_payee, other_split] {
                assert_eq!(
                    next_step(&account, &sponsor, Some(700), CLOSED_AT, 0, true),
                    Step::NotOurs
                );
            }
        }
        let distributed = wire::ChannelAccount {
            payee: someone,
            ..ours(sponsor, wire::ChannelStatus::Distributed)
        };
        assert_eq!(
            next_step(&distributed, &sponsor, None, 0, u64::MAX, false),
            Step::Reclaim
        );
    }
}
