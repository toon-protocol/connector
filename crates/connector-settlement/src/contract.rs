//! One contract suite (ADR 0007): the definition of the [`SettlementBackend`]
//! port, written so any implementation -- in-process or, per ADR 0002, a
//! real chain in a separate crate -- can be run against it and is not an
//! implementation of the port until it passes unmodified.
//!
//! Gated behind the `test-util` feature (rather than `#[cfg(test)]` alone)
//! because, unlike `connector-runtime`'s `PeerTransport` contract suite,
//! this port's implementations live in separate crates
//! (`connector-settlement-evm`, `connector-settlement-solana`): a suite
//! hidden behind `#[cfg(test)]` is invisible outside this crate's own test
//! build, so those crates could never hold their implementation to it.
//! They add this crate under `[dev-dependencies]` with `features =
//! ["test-util"]` and call [`assert_upholds_the_contract`] from their own
//! tests instead.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use chrono::Duration;

use crate::port::{ChannelId, ChannelStatus, Claim, SettlementBackend, SettlementError};

/// Everything a [`SettlementBackend`] implementation hands the suite about
/// itself, beyond the backend value, so the suite can exercise it without
/// hardcoding assumptions no real chain actually holds (issue #574,
/// issue #576).
pub struct ContractFixture {
    pub backend: Arc<dyn SettlementBackend>,
    /// A counterparty identity [`open`](SettlementBackend::open) is called
    /// with -- a real signing address/pubkey on a chain that needs one
    /// (issue #574), not a plain ASCII peer name.
    pub counterparty: Vec<u8>,
    /// A second, distinct counterparty identity, opened against but never
    /// redeemed from.
    pub other_counterparty: Vec<u8>,
    /// A third, distinct counterparty identity for the instant-settlement
    /// channel this suite drives all the way to
    /// [`ChannelStatus::Settled`] (issue #567): the deployed Solana
    /// `payment-channel` program holds exactly one live channel per
    /// (participant pair, mint) -- its channel PDA is seeded
    /// `["channel", min, max, mint]` and `InitializeChannel` rejects a
    /// still-existing account with `ChannelAlreadyExists` -- so that
    /// channel cannot reuse [`counterparty`](Self::counterparty) while the
    /// first channel is still sitting in its challenge window. Like
    /// `counterparty` (and unlike
    /// [`other_counterparty`](Self::other_counterparty)) this channel is
    /// funded and, post-settlement, redeemed against, so a backend whose
    /// chain requires the depositing participant's own signature must hold
    /// a real key for this identity too.
    pub instant_counterparty: Vec<u8>,
    /// Produces the bytes this suite puts in a [`Claim`]'s `signature`,
    /// given the channel it redeems against and that claim's
    /// `nonce`/`cumulative_amount` (issue #576): a backend whose chain
    /// actually verifies a claim's signature (`TokenNetwork
    /// .claimFromChannel`'s EIP-712 recovery, unlike the old, unverified
    /// `SettlementChannel.sol` this port originally shipped with) needs a
    /// real one produced by `counterparty`'s own key, not an arbitrary
    /// literal nothing checks. A backend that does not verify the
    /// signature at all (`InMemorySettlementBackend`) can return one that
    /// ignores its arguments.
    pub sign: SignFn,
    /// The `settlement_timeout` [`open`](SettlementBackend::open) is called
    /// with for the channel this suite proves [`settle`](SettlementBackend::settle)
    /// eventually reaches [`ChannelStatus::Settled`] on (issue #576): some
    /// chains enforce a protocol-level minimum no implementation can be
    /// asked to skip (`TokenNetwork.sol`'s `MIN_SETTLEMENT_TIMEOUT`, one
    /// hour), so this suite does not assume every backend can open a
    /// channel with an arbitrarily short (or zero) one.
    pub instant_settlement_timeout: Duration,
    /// Make the channel's **counterparty** deposit `amount` on *their*
    /// own side, raising [`crate::ChannelState::counterparty_deposited`] --
    /// which is the only collateral a claim this backend redeems is ever
    /// drawn from, and which [`SettlementBackend::fund`] deliberately
    /// does not touch (issue #1118).
    ///
    /// A fixture capability rather than a port method, because on a real
    /// chain this is somebody else's signed transaction from somebody
    /// else's wallet. `packages/solana-program`'s `Deposit` credits
    /// strictly by signer, so no node can ever perform it for a
    /// counterparty; `TokenNetwork.setTotalDeposit` happens to allow it,
    /// and defining the *port* around that one chain's affordance is
    /// exactly what left `fund` unconditionally broken on the other.
    /// Whatever standing in for that external actor costs -- a held
    /// keypair, a second wallet, an in-process write -- is the fixture's
    /// problem, not the port's.
    ///
    /// Called only against a channel this suite has open; it may assume
    /// the channel is `counterparty`'s or `instant_counterparty`'s.
    pub fund_counterparty: FundCounterpartyFn,
    /// Called once, after that channel is closed and before this suite
    /// asks it to settle, to make `instant_settlement_timeout` have
    /// elapsed without this suite waiting out real wall-clock time itself
    /// (issue #576): a real sleep for a backend with no meaningful
    /// minimum, a chain-clock advance (e.g. `anvil`'s `evm_increaseTime`)
    /// for one that has to open a channel with a long real timeout.
    pub advance_past_instant_settlement_timeout: Box<dyn Fn() -> BoxFuture<'static, ()> + Send>,
}

/// A boxed, `'static`, `Send` future -- the shape
/// [`ContractFixture::advance_past_instant_settlement_timeout`] returns,
/// since a plain `async fn` cannot be named as a trait object field type.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The shape of [`ContractFixture::sign`], named so clippy's
/// `type_complexity` lint (and any reader) sees one name rather than the
/// spelled-out trait object at every use site.
pub type SignFn = Box<dyn Fn(&ChannelId, u64, u128) -> Vec<u8> + Send>;

/// The shape of [`ContractFixture::fund_counterparty`] -- named for the
/// same reason [`SignFn`] is. Asynchronous where `SignFn` is not: standing
/// in for the counterparty's deposit means a real, confirmed transaction
/// on every backend whose chain is real.
pub type FundCounterpartyFn = Box<dyn Fn(&ChannelId, u128) -> BoxFuture<'static, ()> + Send>;

/// Run every assertion the [`SettlementBackend`] port makes, against a
/// freshly built implementation from `build`. A conforming implementation
/// passes this function without modification -- that unmodified pass is
/// what "upholds the contract" means (ADR 0007).
pub async fn assert_upholds_the_contract<F, Fut>(build: F)
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = ContractFixture>,
{
    let ContractFixture {
        backend,
        counterparty,
        other_counterparty,
        instant_counterparty,
        sign,
        fund_counterparty,
        instant_settlement_timeout,
        advance_past_instant_settlement_timeout,
    } = build().await;
    let timeout = Duration::seconds(3600);

    // ADR 0059: "do I already have a channel with this counterparty?" is
    // answerable *before* opening one, and the answer is no.
    assert_eq!(
        backend
            .live_channel_with(counterparty.clone())
            .await
            .expect("live_channel_with on a pair with no channel"),
        None,
        "a pair that has never opened a channel must report none"
    );

    // Opening a channel reports it open, unfunded, to the counterparty given.
    let channel = backend
        .open(counterparty.clone(), timeout)
        .await
        .expect("open");

    // ...and now the same question finds it, from the participants alone.
    // This is what makes establishing a peering from a URL idempotent
    // (ADR 0058): a second attempt lands on the channel the first opened
    // rather than opening a second one.
    assert_eq!(
        backend
            .live_channel_with(counterparty.clone())
            .await
            .expect("live_channel_with after open"),
        Some(channel.clone()),
        "an open channel must be findable from its counterparty alone"
    );
    let state = backend
        .channel_state(&channel)
        .await
        .expect("channel_state");
    assert_eq!(state.status, ChannelStatus::Open);
    assert_eq!(state.counterparty_deposited, 0);
    assert_eq!(state.own_deposited, 0);
    assert_eq!(state.redeemed, 0);
    assert_eq!(state.counterparty, counterparty);

    // `fund` is a SELF-deposit (issue #1118): it raises this backend's own
    // collateral, cumulatively across calls, and moves the counterparty's
    // side not at all. A backend on a chain that *could* credit the
    // counterparty from the caller's own balance
    // (`TokenNetwork.setTotalDeposit`) must not do so here -- this pair of
    // assertions is what catches it if it does.
    let state = backend.fund(&channel, 100).await.expect("fund");
    assert_eq!(state.own_deposited, 100);
    assert_eq!(state.counterparty_deposited, 0);
    let state = backend.fund(&channel, 50).await.expect("fund");
    assert_eq!(state.own_deposited, 150);
    assert_eq!(state.counterparty_deposited, 0);

    // ...and that self-deposit is durable, not merely whatever the
    // funding call happened to return.
    let state = backend
        .channel_state(&channel)
        .await
        .expect("channel_state");
    assert_eq!(state.own_deposited, 150);
    assert_eq!(state.counterparty_deposited, 0);

    // `fund_to` is the retry-safe form (ADR 0073 decision 5): it raises the
    // own deposit TO a total, so repeating it -- the retry of a call whose
    // answer was lost -- deposits nothing more, and a total already reached
    // is a no-op rather than an error.
    let state = backend.fund_to(&channel, 200).await.expect("fund_to");
    assert_eq!(state.own_deposited, 200);
    let state = backend
        .fund_to(&channel, 200)
        .await
        .expect("fund_to, repeated");
    assert_eq!(state.own_deposited, 200, "a repeated fund_to deposits once");
    let state = backend
        .fund_to(&channel, 120)
        .await
        .expect("fund_to below the current total");
    assert_eq!(state.own_deposited, 200, "a lower total takes nothing back");
    assert_eq!(state.counterparty_deposited, 0);

    // The counterparty, depositing on their own side, is what puts value
    // behind the claims *this* backend redeems -- and it leaves this
    // backend's own collateral exactly where `fund_to` left it.
    fund_counterparty(&channel, 150).await;
    let state = backend
        .channel_state(&channel)
        .await
        .expect("channel_state");
    assert_eq!(state.counterparty_deposited, 150);
    assert_eq!(state.own_deposited, 200);

    // Redeeming a valid claim moves the redeemed total to the claim's
    // cumulative amount. The deposit is untouched -- it is the channel's
    // total funding, not what remains unredeemed.
    let state = backend
        .redeem(
            &channel,
            Claim {
                nonce: 1,
                cumulative_amount: 60,
                signature: sign(&channel, 1, 60),
            },
        )
        .await
        .expect("redeem");
    assert_eq!(state.redeemed, 60);
    assert_eq!(state.counterparty_deposited, 150);
    assert_eq!(state.own_deposited, 200);

    // A later claim supersedes an earlier one: redeeming again for a
    // higher cumulative amount succeeds and moves the total further.
    let state = backend
        .redeem(
            &channel,
            Claim {
                nonce: 2,
                cumulative_amount: 120,
                signature: sign(&channel, 2, 120),
            },
        )
        .await
        .expect("redeem");
    assert_eq!(state.redeemed, 120);

    // A claim that does not supersede the highest one redeemed so far is
    // rejected outright (ADR 0005: only the highest-nonce claim is ever
    // honored) rather than silently ignored or double-paid. Same nonce as
    // the claim just redeemed, matching the amount replay this asserts --
    // `connector-settlement-evm`/`-solana` settle through contracts with no
    // nonce field of their own yet (issue #566), so this exact replay is
    // the one nonce/amount scenario this shared suite can hold every
    // backend to identically; `InMemorySettlementBackend`'s own,
    // additional nonce-ordering rule is exercised separately in
    // `in_memory.rs`'s own tests.
    let err = backend
        .redeem(
            &channel,
            Claim {
                nonce: 2,
                cumulative_amount: 120,
                signature: sign(&channel, 2, 120),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        err,
        SettlementError::StaleClaim {
            claimed: 120,
            already_redeemed: 120,
        }
    );

    // A claim beyond the channel's funded balance is rejected -- a
    // redeemer is never paid more than was actually deposited.
    let err = backend
        .redeem(
            &channel,
            Claim {
                nonce: 3,
                cumulative_amount: 1_000,
                signature: sign(&channel, 3, 1_000),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        err,
        SettlementError::InsufficientChannelBalance {
            requested: 1_000,
            deposited: 150,
        }
    );

    // The bound is the *counterparty's* deposit and nothing else: this
    // backend's own 200 of collateral is not spare change a claim against
    // it may draw on. Raising only the self-deposit leaves the identical
    // claim refused with the identical number.
    backend.fund(&channel, 900).await.expect("fund");
    let err = backend
        .redeem(
            &channel,
            Claim {
                nonce: 3,
                cumulative_amount: 1_000,
                signature: sign(&channel, 3, 1_000),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        err,
        SettlementError::InsufficientChannelBalance {
            requested: 1_000,
            deposited: 150,
        },
        "a self-deposit must not raise the ceiling on claims this backend redeems"
    );

    // ...and that rejection is retryable, not terminal (issue #662). The
    // refusal must leave the channel exactly as it found it -- in
    // particular the nonce the refused claim was signed for must still be
    // unused -- so that funding the channel up past the claimed amount
    // makes the *identical* claim redeemable. Both real chains rely on
    // this: `TokenNetwork.claimFromChannel` reverts before writing
    // `participants[.][signer].nonce`, and `packages/solana-program`'s
    // `ClaimFromChannel` returns `TransferredAmountExceedsDeposit` before
    // writing `nonce_x`. If either consumed the nonce on refusal, bounding
    // a claim by the deposit would burn an honest, already-signed proof
    // rather than merely deferring it.
    fund_counterparty(&channel, 900).await;
    let state = backend
        .channel_state(&channel)
        .await
        .expect("channel_state");
    assert_eq!(state.counterparty_deposited, 1_050);
    let state = backend
        .redeem(
            &channel,
            Claim {
                nonce: 3,
                cumulative_amount: 1_000,
                signature: sign(&channel, 3, 1_000),
            },
        )
        .await
        .expect("the refused claim redeems once the deposit covers it");
    assert_eq!(state.redeemed, 1_000);

    // Closing a channel starts its challenge period (issue #574): its own
    // state reports Closed, that status is durable when queried back
    // separately, funding is refused, and it cannot be closed a second
    // time -- but redeeming during the window that follows still
    // succeeds. Forfeiting that window (the old behaviour here) hands the
    // whole outstanding balance back to whichever party closed the
    // channel; `TokenNetwork.claimFromChannel` deliberately accepts both
    // `Opened` and `Closed` for exactly this reason
    // (`packages/contracts/src/TokenNetwork.sol:262-263`, `:273`).
    let state = backend.close(&channel).await.expect("close");
    assert_eq!(state.status, ChannelStatus::Closed);

    let state = backend
        .channel_state(&channel)
        .await
        .expect("channel_state");
    assert_eq!(state.status, ChannelStatus::Closed);

    let err = backend.fund(&channel, 10).await.unwrap_err();
    assert_eq!(err, SettlementError::ChannelClosed(channel.clone()));

    // A later, still-superseding claim redeems during the challenge
    // window -- this is the window's whole point (issue #574).
    let state = backend
        .redeem(
            &channel,
            Claim {
                nonce: 4,
                cumulative_amount: 1_001,
                signature: sign(&channel, 4, 1_001),
            },
        )
        .await
        .expect("redeem during the challenge window");
    assert_eq!(state.redeemed, 1_001);

    let err = backend.close(&channel).await.unwrap_err();
    assert_eq!(err, SettlementError::ChannelClosed(channel.clone()));

    // A closed channel still occupies its pair's identifier: it holds
    // collateral and still redeems, so reporting the pair as having none
    // would hand a caller an `open` that fails.
    assert_eq!(
        backend
            .live_channel_with(counterparty.clone())
            .await
            .expect("live_channel_with during the challenge window"),
        Some(channel.clone()),
        "a channel inside its challenge window is still the pair's live channel"
    );

    // `timeout` above is a full hour and no real time has elapsed since
    // `close` -- settling this channel now must fail with the named
    // "not yet due" error (issue #574), not a generic backend string.
    let err = backend.settle(&channel).await.unwrap_err();
    assert_eq!(err, SettlementError::SettlementNotYetDue(channel.clone()));

    // A channel becomes settleable once its own challenge period has
    // genuinely elapsed -- proving `settle` reaches a terminal,
    // no-longer-redeemable state (not just that it refuses early), without
    // this suite waiting out `instant_settlement_timeout` in real
    // wall-clock time itself (issue #576:
    // `advance_past_instant_settlement_timeout` is how that elapsing is
    // actually achieved, since a chain like `TokenNetwork` cannot be asked
    // to open a channel with a shorter timeout than its own
    // `MIN_SETTLEMENT_TIMEOUT`, one hour, and this suite is not going to
    // sleep for one). Opened against its own dedicated counterparty
    // identity rather than reusing `counterparty`, whose first channel is
    // still sitting Closed in its challenge window -- see
    // [`ContractFixture::instant_counterparty`]'s doc for the chain that
    // makes that reuse impossible.
    let immediate = backend
        .open(instant_counterparty.clone(), instant_settlement_timeout)
        .await
        .expect("open the instant-settlement-proof channel");
    let state = backend.fund(&immediate, 200).await.expect("fund");
    assert_eq!(state.own_deposited, 200);
    backend.close(&immediate).await.expect("close");
    advance_past_instant_settlement_timeout().await;
    let state = backend.settle(&immediate).await.expect("settle");
    assert_eq!(state.status, ChannelStatus::Settled);

    let err = backend
        .redeem(
            &immediate,
            Claim {
                nonce: 1,
                cumulative_amount: 50,
                signature: sign(&immediate, 1, 50),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err, SettlementError::ChannelSettled(immediate.clone()));

    let err = backend.settle(&immediate).await.unwrap_err();
    assert_eq!(err, SettlementError::ChannelSettled(immediate));

    // ...and once it has settled, the pair reports no live channel again,
    // which is what lets two parties who have finished one start another
    // (`CONTEXT.md`, **Payment channel**; ADR 0059's epoch is the EVM
    // mechanism, a freed PDA the Solana one).
    assert_eq!(
        backend
            .live_channel_with(instant_counterparty.clone())
            .await
            .expect("live_channel_with after settlement"),
        None,
        "a settled pair must report no live channel, so it can start a fresh one"
    );

    // A channel id from one open() call names only that channel -- a
    // second channel to a different counterparty has its own independent,
    // freshly-unfunded state.
    let other = backend
        .open(other_counterparty.clone(), timeout)
        .await
        .expect("open");
    assert_ne!(other, channel);
    let other_state = backend.channel_state(&other).await.expect("channel_state");
    assert_eq!(other_state.status, ChannelStatus::Open);
    assert_eq!(other_state.counterparty_deposited, 0);
    assert_eq!(other_state.own_deposited, 0);
    assert_eq!(
        backend
            .live_channel_with(other_counterparty.clone())
            .await
            .expect("live_channel_with for the second counterparty"),
        Some(other.clone()),
        "each pair's live channel is that pair's, not whichever was opened last"
    );

    // Operating on an id nothing ever opened is reported, not panicked.
    let missing = ChannelId("does-not-exist".to_string());
    let err = backend.channel_state(&missing).await.unwrap_err();
    assert_eq!(err, SettlementError::ChannelNotFound(missing.clone()));
    let err = backend.fund(&missing, 1).await.unwrap_err();
    assert_eq!(err, SettlementError::ChannelNotFound(missing.clone()));
    let err = backend.close(&missing).await.unwrap_err();
    assert_eq!(err, SettlementError::ChannelNotFound(missing.clone()));
    let err = backend.settle(&missing).await.unwrap_err();
    assert_eq!(err, SettlementError::ChannelNotFound(missing));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InMemorySettlementBackend;

    #[tokio::test]
    async fn in_memory_settlement_backend_upholds_the_contract() {
        assert_upholds_the_contract(|| async {
            let backend = Arc::new(InMemorySettlementBackend::new());
            ContractFixture {
                backend: Arc::clone(&backend) as Arc<dyn SettlementBackend>,
                counterparty: b"counterparty-a".to_vec(),
                other_counterparty: b"counterparty-b".to_vec(),
                instant_counterparty: b"counterparty-c".to_vec(),
                // `InMemorySettlementBackend` never verifies a claim's
                // signature, so any bytes suffice.
                sign: Box::new(
                    |_channel: &ChannelId, _nonce: u64, _cumulative_amount: u128| vec![0u8],
                ),
                fund_counterparty: {
                    let backend = Arc::clone(&backend);
                    Box::new(move |channel: &ChannelId, amount: u128| {
                        let backend = Arc::clone(&backend);
                        let channel = channel.clone();
                        Box::pin(async move {
                            backend
                                .fund_counterparty(&channel, amount)
                                .await
                                .expect("the counterparty deposits");
                        })
                    })
                },
                // No real minimum, so a zero-length challenge period is
                // already due the instant the channel closes -- no
                // advancing needed.
                instant_settlement_timeout: Duration::zero(),
                advance_past_instant_settlement_timeout: Box::new(|| Box::pin(async {})),
            }
        })
        .await;
    }
}
