use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};

use crate::port::{
    ChannelId, ChannelState, ChannelStatus, Claim, SettlementBackend, SettlementError,
};

struct StoredChannel {
    counterparty: Vec<u8>,
    status: ChannelStatus,
    /// The counterparty's own deposit -- what `redeem` is bounded by.
    /// Raised only by [`InMemorySettlementBackend::fund_counterparty`],
    /// never by `fund` (issue #1118).
    counterparty_deposited: u128,
    /// This backend's own deposit -- what `fund` raises.
    own_deposited: u128,
    redeemed: u128,
    redeemed_nonce: u64,
    settlement_timeout: Duration,
    /// When `close` ran, if it has -- the challenge period's start.
    /// `settle` measures its own timeout from here (issue #574).
    closed_at: Option<DateTime<Utc>>,
}

impl StoredChannel {
    fn state(&self, id: &ChannelId) -> ChannelState {
        ChannelState {
            id: id.clone(),
            counterparty: self.counterparty.clone(),
            status: self.status,
            counterparty_deposited: self.counterparty_deposited,
            own_deposited: self.own_deposited,
            redeemed: self.redeemed,
        }
    }
}

/// The in-memory [`SettlementBackend`]: channel state lives only in this
/// process and nothing is ever submitted to a chain. This is the fake this
/// workspace's own tests use, and the first implementation to pass the
/// contract suite in [`crate::contract`] (ADR 0007) -- proving the port's
/// shape is satisfiable before any real chain code exists.
#[derive(Default)]
pub struct InMemorySettlementBackend {
    channels: Mutex<HashMap<ChannelId, StoredChannel>>,
    next_id: AtomicU64,
}

impl InMemorySettlementBackend {
    pub fn new() -> Self {
        InMemorySettlementBackend::default()
    }

    /// Stand in for the counterparty depositing `amount` on *their* own
    /// side of `channel` -- the only thing that raises
    /// [`ChannelState::counterparty_deposited`], and so the only thing
    /// that makes a claim this backend can [`redeem`](SettlementBackend::redeem)
    /// worth anything (issue #1118).
    ///
    /// Not on the [`SettlementBackend`] port, and deliberately so: on a
    /// real chain this is the counterparty's own signed transaction from
    /// their own wallet, which no node can perform on their behalf
    /// (`packages/solana-program`'s `Deposit` credits strictly by signer).
    /// This is the fake standing in for that external actor, which is
    /// what [`crate::contract::ContractFixture::fund_counterparty`] asks
    /// every implementation's fixture for.
    pub async fn fund_counterparty(
        &self,
        channel: &ChannelId,
        amount: u128,
    ) -> Result<ChannelState, SettlementError> {
        self.with_open_channel(channel, |c| {
            c.counterparty_deposited += amount;
            Ok(c.state(channel))
        })
    }

    fn channels(&self) -> MutexGuard<'_, HashMap<ChannelId, StoredChannel>> {
        self.channels
            .lock()
            .expect("InMemorySettlementBackend lock poisoned")
    }

    /// Look up `id`, requiring the channel still be `Open` -- used by `fund`
    /// and `close`, which both refuse a `Closed` or `Settled` channel alike
    /// (issue #574: unlike `redeem`, neither has a reason to distinguish
    /// the two).
    fn with_open_channel<T>(
        &self,
        id: &ChannelId,
        f: impl FnOnce(&mut StoredChannel) -> Result<T, SettlementError>,
    ) -> Result<T, SettlementError> {
        let mut channels = self.channels();
        let channel = channels
            .get_mut(id)
            .ok_or_else(|| SettlementError::ChannelNotFound(id.clone()))?;
        match channel.status {
            ChannelStatus::Open => {}
            ChannelStatus::Closed => return Err(SettlementError::ChannelClosed(id.clone())),
            ChannelStatus::Settled => return Err(SettlementError::ChannelSettled(id.clone())),
        }
        f(channel)
    }

    /// Look up `id`, refusing only a `Settled` channel -- used by `redeem`,
    /// which succeeds against both `Open` and `Closed` (issue #574: a
    /// channel's challenge period is exactly the window `redeem` must keep
    /// working in).
    fn with_redeemable_channel<T>(
        &self,
        id: &ChannelId,
        f: impl FnOnce(&mut StoredChannel) -> Result<T, SettlementError>,
    ) -> Result<T, SettlementError> {
        let mut channels = self.channels();
        let channel = channels
            .get_mut(id)
            .ok_or_else(|| SettlementError::ChannelNotFound(id.clone()))?;
        if channel.status == ChannelStatus::Settled {
            return Err(SettlementError::ChannelSettled(id.clone()));
        }
        f(channel)
    }
}

#[async_trait]
impl SettlementBackend for InMemorySettlementBackend {
    async fn open(
        &self,
        counterparty: Vec<u8>,
        settlement_timeout: Duration,
    ) -> Result<ChannelId, SettlementError> {
        // A plain decimal counter (issue #575) -- this fake's channel id
        // doubles as the peer-role `ClaimBook`'s channel id in this
        // workspace's own tests, which now requires a decimal or hex
        // on-chain-`bytes32`-shaped string. `EvmSettlementBackend`'s own
        // channel ids are the `0x`-prefixed hex `bytes32` `TokenNetwork`
        // assigns (issue #576); this backend keeps the decimal shape as
        // its own, still-accepted alternative rather than mimicking a real
        // chain it does not otherwise resemble.
        let id = ChannelId(self.next_id.fetch_add(1, Ordering::SeqCst).to_string());
        self.channels().insert(
            id.clone(),
            StoredChannel {
                counterparty,
                status: ChannelStatus::Open,
                counterparty_deposited: 0,
                own_deposited: 0,
                redeemed: 0,
                redeemed_nonce: 0,
                settlement_timeout,
                closed_at: None,
            },
        );
        Ok(id)
    }

    async fn fund(
        &self,
        channel: &ChannelId,
        amount: u128,
    ) -> Result<ChannelState, SettlementError> {
        self.with_open_channel(channel, |c| {
            c.own_deposited += amount;
            Ok(c.state(channel))
        })
    }

    /// Atomic under the backend's one lock: the read and the raise are one
    /// step, so two concurrent calls for the same total deposit once.
    async fn fund_to(
        &self,
        channel: &ChannelId,
        own_total: u128,
    ) -> Result<ChannelState, SettlementError> {
        self.with_open_channel(channel, |c| {
            c.own_deposited = c.own_deposited.max(own_total);
            Ok(c.state(channel))
        })
    }

    async fn redeem(
        &self,
        channel: &ChannelId,
        claim: Claim,
    ) -> Result<ChannelState, SettlementError> {
        self.with_redeemable_channel(channel, |c| {
            if claim.cumulative_amount <= c.redeemed {
                return Err(SettlementError::StaleClaim {
                    claimed: claim.cumulative_amount,
                    already_redeemed: c.redeemed,
                });
            }
            if claim.cumulative_amount > c.counterparty_deposited {
                return Err(SettlementError::InsufficientChannelBalance {
                    requested: claim.cumulative_amount,
                    deposited: c.counterparty_deposited,
                });
            }
            // Checked after `cumulative_amount`, not before: the two rules
            // are independent (a claim's amount can supersede while its
            // nonce does not), and ordering the amount check first keeps
            // a claim that merely replays the last one accepted -- same
            // nonce, same amount -- reported as `StaleClaim`, matching what
            // `connector-settlement-evm`/`-solana` also report for that
            // same replay today, since neither backend's contract has a
            // nonce field to check against yet (issue #566).
            if claim.nonce <= c.redeemed_nonce {
                return Err(SettlementError::StaleNonce {
                    claimed: claim.nonce,
                    already_redeemed: c.redeemed_nonce,
                });
            }
            c.redeemed = claim.cumulative_amount;
            c.redeemed_nonce = claim.nonce;
            Ok(c.state(channel))
        })
    }

    async fn close(&self, channel: &ChannelId) -> Result<ChannelState, SettlementError> {
        self.with_open_channel(channel, |c| {
            c.status = ChannelStatus::Closed;
            c.closed_at = Some(Utc::now());
            Ok(c.state(channel))
        })
    }

    async fn settle(&self, channel: &ChannelId) -> Result<ChannelState, SettlementError> {
        let mut channels = self.channels();
        let c = channels
            .get_mut(channel)
            .ok_or_else(|| SettlementError::ChannelNotFound(channel.clone()))?;
        match c.status {
            ChannelStatus::Settled => return Err(SettlementError::ChannelSettled(channel.clone())),
            // An `Open` channel has no `closed_at` to measure a timeout
            // from at all -- folded into `SettlementNotYetDue` rather than
            // a separate variant, since "settlement is not yet permitted"
            // covers both "still open" and "closed but not yet elapsed"
            // (issue #574).
            ChannelStatus::Open => {
                return Err(SettlementError::SettlementNotYetDue(channel.clone()))
            }
            ChannelStatus::Closed => {}
        }
        let closed_at = c
            .closed_at
            .expect("a Closed channel always has closed_at set by `close`");
        if Utc::now() < closed_at + c.settlement_timeout {
            return Err(SettlementError::SettlementNotYetDue(channel.clone()));
        }
        c.status = ChannelStatus::Settled;
        Ok(c.state(channel))
    }

    async fn channel_state(&self, channel: &ChannelId) -> Result<ChannelState, SettlementError> {
        let channels = self.channels();
        let stored = channels
            .get(channel)
            .ok_or_else(|| SettlementError::ChannelNotFound(channel.clone()))?;
        Ok(stored.state(channel))
    }

    /// This fake has no chain to ask, so it answers ADR 0059's question
    /// from the only state it has: the channel it opened to that
    /// counterparty, if one is still live.
    ///
    /// A scan rather than a second index keyed by counterparty, because
    /// this is a cold-path operator question and the map is however many
    /// channels one test opened. What it must reproduce is the *rule* --
    /// at most one live channel per counterparty, and a settled pair
    /// reporting none so it can start again -- which the contract suite
    /// holds every real backend to as well.
    async fn live_channel_with(
        &self,
        counterparty: Vec<u8>,
    ) -> Result<Option<ChannelId>, SettlementError> {
        Ok(self
            .channels()
            .iter()
            .find(|(_, stored)| {
                stored.counterparty == counterparty && stored.status != ChannelStatus::Settled
            })
            .map(|(id, _)| id.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    /// Issue #573: a real chain enforces nonce ordering independently of
    /// amount (`TokenNetwork.claimFromChannel`'s `balanceProof.nonce >
    /// counterpartyState.nonce`), so a claim whose amount happens to
    /// supersede the last one redeemed must still be refused if its nonce
    /// does not -- and refused distinguishably from `StaleClaim`, which is
    /// [`crate::contract::assert_upholds_the_contract`]'s own scenario for
    /// a claim that fails on amount alone. This is not part of that shared
    /// suite because `connector-settlement-evm` and `connector-settlement-solana`
    /// settle through contracts with no nonce field yet (issue #566) and
    /// cannot enforce this client-side until that lands; this backend, with
    /// no such constraint, enforces it today.
    #[tokio::test]
    async fn a_claim_whose_nonce_does_not_advance_is_refused_distinctly_from_a_stale_amount() {
        let backend = InMemorySettlementBackend::new();
        let channel = backend
            .open(b"counterparty-a".to_vec(), Duration::seconds(3600))
            .await
            .expect("open");
        backend
            .fund_counterparty(&channel, 1_000)
            .await
            .expect("the counterparty deposits what these claims draw on");

        backend
            .redeem(
                &channel,
                Claim {
                    nonce: 1,
                    cumulative_amount: 60,
                    signature: vec![1],
                },
            )
            .await
            .expect("redeem");

        // A higher cumulative amount alone is not enough: this claim's
        // nonce (1) does not exceed the one just redeemed (also 1).
        let err = backend
            .redeem(
                &channel,
                Claim {
                    nonce: 1,
                    cumulative_amount: 120,
                    signature: vec![2],
                },
            )
            .await
            .unwrap_err();
        assert_eq!(
            err,
            SettlementError::StaleNonce {
                claimed: 1,
                already_redeemed: 1,
            }
        );

        // Neither the redeemed amount nor the redeemed nonce moved.
        let state = backend
            .channel_state(&channel)
            .await
            .expect("channel_state");
        assert_eq!(state.redeemed, 60);

        // A genuinely advancing claim -- both nonce and amount -- still
        // succeeds afterward.
        let state = backend
            .redeem(
                &channel,
                Claim {
                    nonce: 2,
                    cumulative_amount: 120,
                    signature: vec![3],
                },
            )
            .await
            .expect("redeem");
        assert_eq!(state.redeemed, 120);
    }
}
