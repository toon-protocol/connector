//! One contract suite (ADR 0007): the definition of the
//! [`BatchSettlementBackend`] port, run unmodified against the in-memory
//! fake here and against each chain's implementation from its own crate
//! (issues #1342, #1343).
//!
//! Gated behind `test-util` for the same reason as [`crate::contract`]: the
//! real implementations live in other crates, which add this one under
//! `[dev-dependencies]` with `features = ["test-util"]` and call
//! [`assert_upholds_the_contract`] from their own tests.
//!
//! The suite asserts only what holds on **both** chains. Where they differ
//! -- an EVM payer's exit is a withdrawal the channel survives, a Solana
//! payer's is a close that sealing ends -- it asserts the part they share:
//! the exiting channel stops backing new vouchers, and the voucher already
//! held still lands.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use super::port::{
    AdmissionRefusal, BatchChannelStatus, BatchSettlementBackend, BatchSettlementError,
    ChannelPresentation, Voucher,
};
use crate::port::ChannelId;

pub use super::in_memory::{ChannelTerms, OpenedChannel};

/// A boxed, `'static`, `Send` future, for the fixture's asynchronous
/// capabilities.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The shape of [`BatchContractFixture::open`].
pub type OpenFn = Box<dyn Fn(ChannelTerms) -> BoxFuture<'static, OpenedChannel> + Send>;
/// The shape of [`BatchContractFixture::deposit`].
pub type DepositFn = Box<dyn Fn(&ChannelId, u128) -> BoxFuture<'static, ()> + Send>;
/// The shape of [`BatchContractFixture::begin_exit`].
pub type BeginExitFn = Box<dyn Fn(&ChannelId) -> BoxFuture<'static, ()> + Send>;
/// The shape of [`BatchContractFixture::sign`].
pub type SignFn = Box<dyn Fn(&ChannelId, u128) -> Vec<u8> + Send>;

/// Everything an implementation hands the suite besides itself. Each
/// capability stands in for the **client**, the payer who owns the channel:
/// on a real chain these are the payer's own signed transactions, which is
/// why they are the fixture's and not the port's.
pub struct BatchContractFixture {
    pub backend: Arc<dyn BatchSettlementBackend>,
    /// The minimum `withdrawDelay` or `grace_period` the backend was built
    /// with, in seconds. Must exceed 901, so that a channel one second short
    /// of it is still one the EVM contract lets exist.
    pub minimum_delay_secs: u64,
    /// Open and fund a channel on these terms, **always as the same payer**
    /// (the suite relies on it to show a second channel from one payer is
    /// admitted). On EVM a deposit through `x402BatchSettlement`; on Solana
    /// an `open` this backend's sponsor key co-signs.
    pub open: OpenFn,
    /// The payer adds `amount` to the channel's deposit: EVM `deposit`,
    /// Solana `top_up`.
    pub deposit: DepositFn,
    /// The payer begins to leave with **everything not yet landed**: EVM
    /// `initiateWithdraw` for `balance − totalClaimed`, Solana
    /// `request_close`.
    pub begin_exit: BeginExitFn,
    /// The payer's voucher signature for this cumulative amount on this
    /// channel, by the key [`OpenedChannel::voucher_signer`] names.
    pub sign: SignFn,
    /// A presentation, for this backend's chain, of a channel that does not
    /// exist.
    pub unopened: ChannelPresentation,
}

fn voucher(sign: &SignFn, channel: &ChannelId, cumulative_amount: u128) -> Voucher {
    Voucher {
        cumulative_amount,
        signature: sign(channel, cumulative_amount),
    }
}

fn refusal(result: Result<impl std::fmt::Debug, BatchSettlementError>) -> AdmissionRefusal {
    match result {
        Err(BatchSettlementError::NotAdmissible { refusal, .. }) => refusal,
        other => panic!("expected an admission refusal, got {other:?}"),
    }
}

/// Run every assertion the [`BatchSettlementBackend`] port makes against a
/// freshly built implementation. Passing it unmodified is what "upholds the
/// contract" means (ADR 0007).
pub async fn assert_upholds_the_contract<F, Fut>(build: F)
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = BatchContractFixture>,
{
    let BatchContractFixture {
        backend,
        minimum_delay_secs,
        open,
        deposit,
        begin_exit,
        sign,
        unopened,
    } = build().await;
    assert!(
        minimum_delay_secs > 901,
        "the fixture's minimum must leave room for a channel one second short of it"
    );
    let admissible = ChannelTerms {
        deposit: 1_000,
        delay_secs: minimum_delay_secs,
        pays_this_node: true,
        in_settled_token: true,
    };

    // -- Admission (ADR 0074 decision 2) --

    // A channel nothing opened is reported, not guessed at.
    let err = backend.admit(unopened.clone()).await.unwrap_err();
    assert_eq!(
        err,
        BatchSettlementError::ChannelNotFound(unopened.channel().clone())
    );

    // Each rule the record fixes refuses by its own name.
    let elsewhere = open(ChannelTerms {
        pays_this_node: false,
        ..admissible
    })
    .await;
    assert!(
        matches!(
            refusal(backend.admit(elsewhere.presentation.clone()).await),
            AdmissionRefusal::NotPayableToThisNode { .. }
        ),
        "a channel paying someone else is not this node's to accept vouchers on"
    );

    let foreign_token = open(ChannelTerms {
        in_settled_token: false,
        ..admissible
    })
    .await;
    assert_eq!(
        refusal(backend.admit(foreign_token.presentation.clone()).await),
        AdmissionRefusal::TokenNotSettled
    );

    let short_delay = open(ChannelTerms {
        delay_secs: minimum_delay_secs - 1,
        ..admissible
    })
    .await;
    assert_eq!(
        refusal(backend.admit(short_delay.presentation.clone()).await),
        AdmissionRefusal::DelayBelowMinimum {
            delay_secs: minimum_delay_secs - 1,
            minimum_secs: minimum_delay_secs,
        }
    );

    // A refused channel is not admitted by having been looked at.
    let err = backend
        .land(
            short_delay.presentation.channel(),
            voucher(&sign, short_delay.presentation.channel(), 1),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err,
        BatchSettlementError::ChannelNotAdmitted(short_delay.presentation.channel().clone())
    );
    let err = backend.channel_state(unopened.channel()).await.unwrap_err();
    assert_eq!(
        err,
        BatchSettlementError::ChannelNotAdmitted(unopened.channel().clone())
    );

    // A channel that meets every rule is admitted -- its delay is exactly
    // the minimum, which is "at least", not "more than" -- and reports
    // itself as the chain has it: open, nothing landed, its whole deposit
    // backing vouchers, and the signer the chain recorded rather than any
    // the client could claim.
    let first = open(admissible).await;
    let channel = first.presentation.channel().clone();
    let state = backend
        .admit(first.presentation.clone())
        .await
        .expect("an admissible channel is admitted");
    assert_eq!(state.id, channel);
    assert_eq!(state.status, BatchChannelStatus::Open);
    assert_eq!(state.voucher_signer, first.voucher_signer);
    assert_eq!(state.landed, 0);
    assert_eq!(state.collateral, 1_000);
    assert_eq!(state.voucher_ceiling(), 1_000);

    assert_eq!(
        backend
            .admit(first.presentation.clone())
            .await
            .expect("admitting again is idempotent"),
        state,
        "admitting an admitted channel again changes nothing"
    );

    // A second channel from the same payer is admitted on its own merits,
    // with its own collateral (ADR 0074 decision 2's exception to 0059).
    let second = open(admissible).await;
    let other = second.presentation.channel().clone();
    assert_ne!(other, channel, "two opens are two channels");
    let other_state = backend
        .admit(second.presentation.clone())
        .await
        .expect("a second channel from one payer is not refused");
    assert_eq!(other_state.collateral, 1_000);

    // -- Landing --

    let state = backend
        .land(&channel, voucher(&sign, &channel, 300))
        .await
        .expect("land");
    assert_eq!(state.landed, 300);
    assert_eq!(
        state.collateral, 700,
        "landing moves value from backing to landed"
    );
    assert_eq!(state.voucher_ceiling(), 1_000);
    assert_eq!(state.status, BatchChannelStatus::Open);

    // ...and the chain agrees when asked separately.
    assert_eq!(backend.channel_state(&channel).await.expect("state"), state);

    // A voucher that does not exceed what is landed is refused by name:
    // a replay, and an older voucher arriving late.
    for stale in [300, 200] {
        let err = backend
            .land(&channel, voucher(&sign, &channel, stale))
            .await
            .unwrap_err();
        assert_eq!(
            err,
            BatchSettlementError::StaleVoucher {
                amount: stale,
                landed: 300,
            }
        );
    }

    // A voucher above the deposit is refused before anything moves...
    let err = backend
        .land(&channel, voucher(&sign, &channel, 1_001))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        BatchSettlementError::VoucherExceedsDeposit {
            amount: 1_001,
            deposited: 1_000,
        }
    );
    assert_eq!(
        backend.channel_state(&channel).await.expect("state").landed,
        300,
        "a refused voucher lands nothing"
    );

    // ...and is retryable: once the payer deposits more, the same voucher
    // lands. Collateral is read from the chain, so the deposit shows
    // without anything on this side being told.
    deposit(&channel, 500).await;
    let state = backend.channel_state(&channel).await.expect("state");
    assert_eq!(state.collateral, 1_200);
    let state = backend
        .land(&channel, voucher(&sign, &channel, 1_001))
        .await
        .expect("the refused voucher lands once the deposit covers it");
    assert_eq!(state.landed, 1_001);
    assert_eq!(state.collateral, 499);

    // Channels are independent: none of that touched the second one.
    assert_eq!(
        backend.channel_state(&other).await.expect("state"),
        other_state
    );

    // -- The payer's exit (ADR 0074 decision 5) --

    // Suppose a voucher for 400 has been accepted on the second channel and
    // not yet landed, and the payer now begins to leave with everything
    // unlanded. The channel must stop backing new vouchers at once, and
    // this side must see that from the chain alone.
    begin_exit(&other).await;
    let state = backend.channel_state(&other).await.expect("state");
    assert!(
        matches!(
            state.status,
            BatchChannelStatus::Withdrawing | BatchChannelStatus::Closing
        ),
        "a payer's exit is a withdrawal on EVM and a close on Solana, got {:?}",
        state.status
    );
    assert_eq!(
        state.collateral, 0,
        "an exit taking everything unlanded leaves nothing backing a new voucher"
    );
    assert_eq!(state.voucher_ceiling(), state.landed);

    // ...and the voucher already held still lands. This is the whole of
    // the connector's protection: a voucher accepted but not landed is
    // worth nothing once the exit completes.
    let state = backend
        .land(&other, voucher(&sign, &other, 400))
        .await
        .expect("a held voucher lands while the payer is leaving");
    assert_eq!(state.landed, 400);
    assert_eq!(state.collateral, 0);
    assert_eq!(
        backend.channel_state(&other).await.expect("state").landed,
        400
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::in_memory::{InMemoryBatchSettlement, PayerExit};

    const ONE_DAY: u64 = 86_400;

    fn fixture(exit: PayerExit) -> BatchContractFixture {
        let backend = Arc::new(InMemoryBatchSettlement::new(exit, ONE_DAY));
        let unopened = backend.unopened_presentation();
        BatchContractFixture {
            backend: Arc::clone(&backend) as Arc<dyn BatchSettlementBackend>,
            minimum_delay_secs: ONE_DAY,
            open: {
                let backend = Arc::clone(&backend);
                Box::new(move |terms| {
                    let backend = Arc::clone(&backend);
                    Box::pin(async move { backend.open(terms) })
                })
            },
            deposit: {
                let backend = Arc::clone(&backend);
                Box::new(move |channel, amount| {
                    let backend = Arc::clone(&backend);
                    let channel = channel.clone();
                    Box::pin(async move { backend.deposit(&channel, amount) })
                })
            },
            begin_exit: {
                let backend = Arc::clone(&backend);
                Box::new(move |channel| {
                    let backend = Arc::clone(&backend);
                    let channel = channel.clone();
                    Box::pin(async move { backend.begin_exit(&channel) })
                })
            },
            // The fake verifies no signature, so any bytes do.
            sign: Box::new(|_channel, _amount| vec![0u8; 65]),
            unopened,
        }
    }

    /// The fake with an EVM-shaped exit: the payer's withdrawal.
    #[tokio::test]
    async fn the_in_memory_fake_with_a_withdrawal_exit_upholds_the_contract() {
        assert_upholds_the_contract(|| async { fixture(PayerExit::Withdrawal) }).await;
    }

    /// The fake with a Solana-shaped exit: the payer's close. The suite
    /// passing against both shapes is what shows it asks nothing only one
    /// chain can answer.
    #[tokio::test]
    async fn the_in_memory_fake_with_a_close_exit_upholds_the_contract() {
        assert_upholds_the_contract(|| async { fixture(PayerExit::Close) }).await;
    }
}
