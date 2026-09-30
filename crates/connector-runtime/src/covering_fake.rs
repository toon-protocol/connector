//! **A payable hop, for a test that forwards** (ADR 0042, ADR 0075
//! decision 6): a `Connector` wired to cover every forward to one peer with
//! a voucher on its own outbound x402 channel, over the in-memory paying
//! half.
//!
//! Every test that forwards a packet to a peer needs one, and that is the
//! point of issue #1145: a peering with nothing to pay it on is refused
//! outright by `Connector::forward_via_peer_route`, and `Config::load`
//! refuses the file that would have produced it
//! (`ConfigError::PayChannelUnbound`). A fixture that forwards without this
//! is not a simpler fixture -- it is one no configuration can produce.
//!
//! Behind `test-support` rather than `#[cfg(test)]` because the carriage
//! and client-edge crates forward in their own tests too, and a
//! `#[cfg(test)]` item is invisible outside its own crate. It opens no
//! socket and touches no disk, but it is still a fake, so the shipped
//! binary does not carry it.
//!
//! Over [`InMemoryBatchSettlement`], which the paying contract suite holds
//! to the chains' behaviour (ADR 0007), with an in-memory journal. Its
//! futures complete without waiting on anything, which is what lets
//! [`covering`] stay a plain builder every call site can keep calling.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use async_trait::async_trait;
use connector_domain::client_claim::{parse_client_claim, ClientClaim};
use connector_settlement::batch::{
    BatchSettlementPayer, ChannelPresentation, InMemoryBatchChain, InMemoryBatchSettlement,
    PayerExit,
};

use crate::batch_channels::OutboundChannels;
use crate::claim::Covering;
use crate::connector::Connector;
use crate::journal::InMemoryJournal;
use crate::outbound_voucher::VoucherStateSource;
use crate::SettlementChain;

/// The opening deposit behind [`covering`]'s channel: large enough that no
/// forward any test makes -- dealing ones across an 18-decimal boundary
/// included -- runs a voucher past its collateral.
const COVERING_DEPOSIT: u128 = 1 << 100;

/// A next hop that has accepted nothing on the channel yet: the
/// claim-state answer a fresh peering gets (ADR 0075 decision 6). A fake
/// upholding the port's contract, not a stub with expectations.
struct NothingAcceptedYet;

#[async_trait]
impl VoucherStateSource for NothingAcceptedYet {
    async fn watermark(
        &self,
        _presentation: &ChannelPresentation,
        _expires: u64,
        _signature: &[u8],
    ) -> Result<u128, String> {
        Ok(0)
    }
}

/// Drive a future the in-memory paying half returns to completion. It never
/// waits on anything, so one poll is enough, and a second is a bug here
/// rather than something to wait out.
fn complete<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the in-memory paying half completes without waiting"),
    }
}

/// `connector`, paying `peer_id` on an outbound x402 channel of its own
/// that it opened with [`COVERING_DEPOSIT`]: every forward to `peer_id` is
/// covered by a voucher on it.
///
/// # Panics
///
/// Never, in practice: the in-memory journal is empty and the fake chain
/// funds the deposit first.
#[must_use]
pub fn covering(connector: Connector, peer_id: &str) -> Connector {
    let chain = InMemoryBatchChain::new(PayerExit::Withdrawal);
    let payer = Arc::new(InMemoryBatchSettlement::on(
        Arc::clone(&chain),
        0x01,
        86_400,
    ));
    payer.fund(COVERING_DEPOSIT);
    let receiver = InMemoryBatchSettlement::on(chain, 0x02, 86_400);
    let outbound = Arc::new(
        complete(OutboundChannels::restore(
            Arc::new(InMemoryJournal::new()),
            vec![(SettlementChain::Evm, payer as Arc<dyn BatchSettlementPayer>)],
        ))
        .expect("an empty journal"),
    );
    let (opened, _) = complete(outbound.open(receiver.published_terms(), COVERING_DEPOSIT))
        .expect("open the covering channel");
    let connector = connector.with_outbound_channels(
        outbound,
        vec![(SettlementChain::Evm, "eip155:31337".to_string())],
    );
    connector.insert_voucher_hop(peer_id, &opened.on_chain.id.0, Arc::new(NothingAcceptedYet));
    connector
}

/// The cumulative amount a voucher covering carried: the figure a test
/// reads a covering off the wire by.
///
/// # Panics
///
/// When `covering` is not an EVM voucher this node rendered.
#[must_use]
pub fn voucher_amount(covering: &Covering) -> u128 {
    let Covering::Voucher(json) = covering else {
        panic!("expected a voucher, got {covering:?}");
    };
    match parse_client_claim(json).expect("a voucher this node rendered parses") {
        ClientClaim::EvmVoucher(voucher) => voucher.max_claimable_amount,
        other => panic!("expected an EVM voucher, got {other:?}"),
    }
}
