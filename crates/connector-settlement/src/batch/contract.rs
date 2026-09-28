//! The contract suites (ADR 0007) that define the batch-settlement port, one
//! per half, run unmodified against the in-memory fake here and against each
//! chain's implementation from its own crate. [`assert_upholds_the_contract`]
//! is the receiving half ([`BatchSettlementBackend`], issues #1342, #1343);
//! [`assert_upholds_the_paying_contract`] is the paying half
//! ([`BatchSettlementPayer`], ADR 0075 decision 2), which the chain
//! implementations join in issues #1374 and #1375.
//!
//! Gated behind `test-util` for the same reason as [`crate::contract`]: the
//! real implementations live in other crates, which add this one under
//! `[dev-dependencies]` with `features = ["test-util"]` and call the suites
//! from their own tests.
//!
//! Each suite asserts only what holds on **both** chains. Where they differ
//! -- an EVM payer's exit is a withdrawal the channel survives, a Solana
//! payer's is a close that sealing ends -- it asserts the part they share:
//! the exiting channel stops backing new vouchers, and the voucher already
//! held still lands.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use super::port::{
    AdmissionRefusal, BatchChannelStatus, BatchSettlementBackend, BatchSettlementError,
    BatchSettlementPayer, ChannelPresentation, EvmReceiverTerms, ReceiverTerms,
    SolanaReceiverTerms, Voucher,
};
use crate::port::ChannelId;

pub use super::in_memory::ChannelTerms;
pub use super::port::OpenedChannel;

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

    // -- Restoring (ADR 0074 decision 5) --

    // A channel this node already holds a voucher on is restored for
    // landing without being judged: a policy tightened across a restart
    // must not strand a voucher already accepted. The short-delay channel
    // stands in for one admitted under a laxer minimum.
    let held_on = short_delay.presentation.channel().clone();
    let state = backend
        .restore(short_delay.presentation.clone())
        .await
        .expect("restoring judges no admission rule");
    assert_eq!(state.id, held_on);
    assert_eq!(state.landed, 0);
    let state = backend
        .land(&held_on, voucher(&sign, &held_on, 100))
        .await
        .expect("a voucher held on a restored channel lands");
    assert_eq!(state.landed, 100);
    assert_eq!(
        backend.channel_state(&held_on).await.expect("state").landed,
        100
    );

    // Restoring is not admitting: the rules still refuse the channel for
    // new vouchers.
    assert_eq!(
        refusal(backend.admit(short_delay.presentation.clone()).await),
        AdmissionRefusal::DelayBelowMinimum {
            delay_secs: minimum_delay_secs - 1,
            minimum_secs: minimum_delay_secs,
        }
    );

    // What makes landing possible at all is still checked: a channel that is
    // not there is not restored.
    assert_eq!(
        backend.restore(unopened.clone()).await.unwrap_err(),
        BatchSettlementError::ChannelNotFound(unopened.channel().clone())
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

/// The shape of [`PayingContractFixture::payer_balance`].
pub type BalanceFn = Box<dyn Fn() -> BoxFuture<'static, u128> + Send>;
/// The shape of [`PayingContractFixture::let_delay_pass`].
pub type LetDelayPassFn = Box<dyn Fn() -> BoxFuture<'static, ()> + Send>;

/// Everything the paying half's suite needs: a payer, and a counterparty on
/// the same chain for it to pay. Unlike [`BatchContractFixture`], nothing
/// here stands in for a client: both ends are the implementation under test,
/// because a peering is two nodes paying each other (ADR 0075 decision 4).
pub struct PayingContractFixture {
    /// The implementation under test, as payer. Its settlement account holds
    /// at least 10,000 base units of the shared token.
    pub payer: Arc<dyn BatchSettlementPayer>,
    /// A second instance of the same implementation, with its own settlement
    /// key, as the receiver that published [`terms`](Self::terms).
    pub receiver: Arc<dyn BatchSettlementBackend>,
    /// The receiver's published terms, in the token the payer settles in. On
    /// Solana its minimum deposit is at most 1,000.
    pub terms: ReceiverTerms,
    /// The payer's settlement account's balance of the shared token, read
    /// from the chain.
    pub payer_balance: BalanceFn,
    /// Let the receiver's minimum delay pass on the chain, so a withdrawal
    /// started before it may finish: anvil moves its clock, a Solana
    /// validator is waited on.
    pub let_delay_pass: LetDelayPassFn,
    /// A channel, in this chain's spelling, that the payer did not open.
    pub not_outbound: ChannelId,
}

/// `terms`, naming a token no node settles in.
fn in_another_token(terms: &ReceiverTerms) -> ReceiverTerms {
    match terms.clone() {
        ReceiverTerms::Evm(terms) => ReceiverTerms::Evm(EvmReceiverTerms {
            token: terms.token.map(|byte| !byte),
            ..terms
        }),
        ReceiverTerms::Solana(terms) => ReceiverTerms::Solana(SolanaReceiverTerms {
            mint: terms.mint.map(|byte| !byte),
            ..terms
        }),
    }
}

/// Terms for the chain `terms` is not for.
fn for_the_other_chain(terms: &ReceiverTerms) -> ReceiverTerms {
    match terms {
        ReceiverTerms::Evm(terms) => ReceiverTerms::Solana(SolanaReceiverTerms {
            sponsor: [0x5a; 32],
            receiver: [0x5a; 32],
            mint: [0x5b; 32],
            min_grace_period_secs: terms.min_withdraw_delay_secs,
            min_deposit: 0,
            sponsor_endpoint: "https://elsewhere.example/ilp/batch-settlement/solana/open"
                .to_string(),
        }),
        ReceiverTerms::Solana(terms) => ReceiverTerms::Evm(EvmReceiverTerms {
            receiver: [0x5a; 20],
            token: [0x5b; 20],
            min_withdraw_delay_secs: terms.min_grace_period_secs,
        }),
    }
}

/// Run every assertion the paying half of the port makes, against a payer
/// and a receiver both freshly built from one implementation. Passing it
/// unmodified is what "upholds the contract" means for the paying half
/// (ADR 0007).
///
/// The seams are #1371's: an open, then admission on the receiving side;
/// deposit and top-up; a signed voucher landing; the watermark refusing a
/// voucher that does not advance; and a withdrawal racing a landing, which
/// the landing wins inside the delay.
pub async fn assert_upholds_the_paying_contract<F, Fut>(build: F)
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = PayingContractFixture>,
{
    let PayingContractFixture {
        payer,
        receiver,
        terms,
        payer_balance,
        let_delay_pass,
        not_outbound,
    } = build().await;
    if let ReceiverTerms::Solana(solana) = &terms {
        assert!(
            solana.min_deposit <= 1_000,
            "the fixture's receiver must sponsor the suite's opening deposits"
        );
    }
    let funded = payer_balance().await;
    assert!(funded >= 10_000, "the fixture's payer must hold 10,000");

    // -- Opening (ADR 0075 decision 3) --

    // Terms this node cannot meet open nothing, and spend nothing.
    assert_eq!(
        payer
            .open(in_another_token(&terms), 1_000)
            .await
            .unwrap_err(),
        BatchSettlementError::TokenNotShared,
        "a channel is opened only in a token both ends settle in"
    );
    let other_chain = for_the_other_chain(&terms);
    assert_eq!(
        payer.open(other_chain.clone(), 1_000).await.unwrap_err(),
        BatchSettlementError::WrongChain {
            presented: other_chain.chain(),
            backend: terms.chain(),
        }
    );
    assert_eq!(
        payer_balance().await,
        funded,
        "a refused open spends nothing"
    );

    // An open on the receiver's own terms is a channel the receiver admits,
    // by exactly the rules it admits a client's by: nothing is configured
    // on its side, and the chain, not the payer, names the voucher signer.
    let opened = payer.open(terms.clone(), 1_000).await.expect("open");
    let channel = opened.presentation.channel().clone();
    assert_eq!(opened.presentation.chain(), terms.chain());
    assert_eq!(
        payer_balance().await,
        funded - 1_000,
        "the opening deposit comes from the payer's own account"
    );

    let outbound = payer
        .outbound_state(&channel)
        .await
        .expect("outbound state");
    assert_eq!(outbound.on_chain.id, channel);
    assert_eq!(outbound.on_chain.status, BatchChannelStatus::Open);
    assert_eq!(outbound.on_chain.voucher_signer, opened.voucher_signer);
    assert_eq!(outbound.on_chain.landed, 0);
    assert_eq!(outbound.on_chain.collateral, 1_000);
    assert_eq!(outbound.signed, 0, "nothing is signed by opening");

    let admitted = receiver
        .admit(opened.presentation.clone())
        .await
        .expect("the receiver admits a channel opened on its own terms");
    assert_eq!(
        admitted, outbound.on_chain,
        "the two halves read one channel off one chain"
    );

    // Opening again is a second channel, both live: a node may hold several
    // toward one receiver (ADR 0075 decision 4).
    let second = payer.open(terms.clone(), 1_000).await.expect("open again");
    let second_channel = second.presentation.channel().clone();
    assert_ne!(second_channel, channel, "two opens are two channels");
    receiver
        .admit(second.presentation.clone())
        .await
        .expect("the second channel is admitted on its own merits");
    assert_eq!(payer_balance().await, funded - 2_000);

    // -- Topping up (ADR 0075 decision 11: an increment, never a total) --

    let state = payer.top_up(&channel, 500).await.expect("top up");
    assert_eq!(state.on_chain.collateral, 1_500);
    assert_eq!(payer_balance().await, funded - 2_500);
    assert_eq!(
        receiver.channel_state(&channel).await.expect("state"),
        state.on_chain,
        "the receiver sees a top-up from the chain alone"
    );

    // -- Signing, and landing what was signed --

    let first = payer.sign_voucher(&channel, 300).await.expect("sign");
    assert_eq!(first.cumulative_amount, 300);
    let landed = receiver
        .land(&channel, first.clone())
        .await
        .expect("a voucher the payer signed lands on the receiver's side");
    assert_eq!(landed.landed, 300);
    assert_eq!(landed.collateral, 1_200);
    let state = payer.outbound_state(&channel).await.expect("state");
    assert_eq!(state.on_chain, landed);
    assert_eq!(state.signed, 300);

    // -- The watermark: a voucher that does not advance is refused --

    // The payer never signs one: the receiver's watermark would refuse it,
    // and the packet it paid for would go unpaid.
    for stale in [300, 200] {
        assert_eq!(
            payer.sign_voucher(&channel, stale).await.unwrap_err(),
            BatchSettlementError::VoucherNotAdvancing {
                amount: stale,
                signed: 300,
            }
        );
    }
    // ...and the receiver lands no voucher twice.
    assert_eq!(
        receiver.land(&channel, first).await.unwrap_err(),
        BatchSettlementError::StaleVoucher {
            amount: 300,
            landed: 300,
        }
    );

    // A voucher the channel does not back is not signed either, and leaves
    // the watermark where it was.
    assert_eq!(
        payer.sign_voucher(&channel, 1_501).await.unwrap_err(),
        BatchSettlementError::VoucherUnbacked {
            amount: 1_501,
            backed: 1_500,
        }
    );
    assert_eq!(
        payer.outbound_state(&channel).await.expect("state").signed,
        300
    );

    // Channels are independent: the second one's watermark is its own.
    let on_second = payer
        .sign_voucher(&second_channel, 100)
        .await
        .expect("the second channel signs from nothing");
    assert_eq!(on_second.cumulative_amount, 100);

    // -- A withdrawal racing a landing (ADR 0074 decision 5) --

    // The payer signs 400 and the receiver holds it, unlanded. The payer then
    // starts to take back everything not landed.
    let held = payer.sign_voucher(&channel, 400).await.expect("sign");
    let state = payer
        .start_withdrawal(&channel)
        .await
        .expect("start a withdrawal");
    assert!(
        matches!(
            state.on_chain.status,
            BatchChannelStatus::Withdrawing | BatchChannelStatus::Closing
        ),
        "a withdrawal is a withdrawal on EVM and a close on Solana, got {:?}",
        state.on_chain.status
    );
    assert_eq!(
        state.on_chain.collateral, 0,
        "a channel being withdrawn from backs nothing new"
    );
    assert_eq!(state.signed, 400);
    assert_eq!(
        payer
            .start_withdrawal(&channel)
            .await
            .expect("starting again"),
        state,
        "starting a withdrawal already started changes nothing"
    );
    assert_eq!(
        receiver.channel_state(&channel).await.expect("state"),
        state.on_chain,
        "the receiver sees the withdrawal from the chain alone"
    );

    // Nothing new is signed on it...
    assert_eq!(
        payer.sign_voucher(&channel, 450).await.unwrap_err(),
        BatchSettlementError::VoucherUnbacked {
            amount: 450,
            backed: 300,
        }
    );
    // ...and it cannot finish inside the delay.
    assert!(
        matches!(
            payer.finish_withdrawal(&channel).await.unwrap_err(),
            BatchSettlementError::WithdrawalNotDue { channel: ref due, .. } if *due == channel
        ),
        "a withdrawal does not finish before its delay has run"
    );

    // Inside the delay the receiver lands what it holds. That is the whole
    // of its protection, and it wins.
    let landed = receiver
        .land(&channel, held)
        .await
        .expect("the held voucher lands while the payer withdraws");
    assert_eq!(landed.landed, 400);

    let_delay_pass().await;
    let before = payer_balance().await;
    let state = payer
        .finish_withdrawal(&channel)
        .await
        .expect("finish the withdrawal once it is due");
    assert_eq!(state.on_chain.landed, 400, "what was landed stays landed");
    assert_eq!(state.on_chain.collateral, 0);
    assert_eq!(
        payer_balance().await - before,
        1_100,
        "the payer gets back exactly what the receiver had not landed: 1,500 less 400"
    );
    assert_eq!(
        payer.finish_withdrawal(&channel).await.unwrap_err(),
        BatchSettlementError::NoWithdrawalPending(channel.clone()),
        "a withdrawal finishes once"
    );

    // A withdrawal never started has nothing to finish.
    assert_eq!(
        payer.finish_withdrawal(&second_channel).await.unwrap_err(),
        BatchSettlementError::NoWithdrawalPending(second_channel.clone())
    );

    // -- Only a channel this node opened is its to pay on --

    let not_outbound_error = BatchSettlementError::NotOutbound(not_outbound.clone());
    assert_eq!(
        payer.outbound_state(&not_outbound).await.unwrap_err(),
        not_outbound_error
    );
    assert_eq!(
        payer.top_up(&not_outbound, 1).await.unwrap_err(),
        not_outbound_error
    );
    assert_eq!(
        payer.sign_voucher(&not_outbound, 1).await.unwrap_err(),
        not_outbound_error
    );
    assert_eq!(
        payer.start_withdrawal(&not_outbound).await.unwrap_err(),
        not_outbound_error
    );
    assert_eq!(
        payer.finish_withdrawal(&not_outbound).await.unwrap_err(),
        not_outbound_error
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::in_memory::{InMemoryBatchChain, InMemoryBatchSettlement, PayerExit};

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
                    Box::pin(async move { backend.client_open(terms) })
                })
            },
            deposit: {
                let backend = Arc::clone(&backend);
                Box::new(move |channel, amount| {
                    let backend = Arc::clone(&backend);
                    let channel = channel.clone();
                    Box::pin(async move { backend.client_deposit(&channel, amount) })
                })
            },
            begin_exit: {
                let backend = Arc::clone(&backend);
                Box::new(move |channel| {
                    let backend = Arc::clone(&backend);
                    let channel = channel.clone();
                    Box::pin(async move { backend.client_begin_exit(&channel) })
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

    /// Two fake nodes on one chain: the first pays, the second receives.
    fn paying_fixture(exit: PayerExit) -> PayingContractFixture {
        let chain = InMemoryBatchChain::new(exit);
        let payer = Arc::new(InMemoryBatchSettlement::on(
            Arc::clone(&chain),
            0x01,
            ONE_DAY,
        ));
        let receiver = Arc::new(
            InMemoryBatchSettlement::on(Arc::clone(&chain), 0x02, ONE_DAY)
                .with_min_sponsored_deposit(1_000),
        );
        payer.fund(10_000);
        let terms = receiver.published_terms();
        // A client's channel toward the receiver: real, and not the payer's.
        let not_outbound = receiver
            .client_open(ChannelTerms {
                deposit: 1_000,
                delay_secs: ONE_DAY,
                pays_this_node: true,
                in_settled_token: true,
            })
            .presentation
            .channel()
            .clone();
        PayingContractFixture {
            payer: Arc::clone(&payer) as Arc<dyn BatchSettlementPayer>,
            receiver: receiver as Arc<dyn BatchSettlementBackend>,
            terms,
            payer_balance: {
                let payer = Arc::clone(&payer);
                Box::new(move || {
                    let payer = Arc::clone(&payer);
                    Box::pin(async move { payer.balance() })
                })
            },
            let_delay_pass: Box::new(move || {
                let chain = Arc::clone(&chain);
                Box::pin(async move { chain.advance_time(ONE_DAY) })
            }),
            not_outbound,
        }
    }

    /// The fake's paying half on an EVM-shaped chain: a withdrawal the
    /// channel survives.
    #[tokio::test]
    async fn the_in_memory_fake_with_a_withdrawal_exit_upholds_the_paying_contract() {
        assert_upholds_the_paying_contract(|| async { paying_fixture(PayerExit::Withdrawal) })
            .await;
    }

    /// The fake's paying half on a Solana-shaped chain: a close that the
    /// receiver's landing seals and a distribution ends.
    #[tokio::test]
    async fn the_in_memory_fake_with_a_close_exit_upholds_the_paying_contract() {
        assert_upholds_the_paying_contract(|| async { paying_fixture(PayerExit::Close) }).await;
    }
}
