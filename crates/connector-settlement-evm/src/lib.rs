//! EVM settlement (ADR 0001, ADR 0074, ADR 0075): both halves of the
//! settlement port over x402's `x402BatchSettlement`, the one contract every
//! EVM channel lives in ([`EvmBatchSettlementBackend`]), and the watcher and
//! sweep that land held vouchers on it ([`EvmBatchWatcher`]).
//!
//! TOON's own `EvmSettlementBackend` over `TokenNetwork`, its channel-id
//! derivation and its ABI are deleted (ADR 0075 decision 12, issue #1385).

mod batch_payer;
mod batch_settlement;
mod batch_watch;
mod bindings;
mod channel_id;
mod log_query;
mod send;
#[cfg(any(test, feature = "test-util"))]
pub mod test_support;

pub use batch_payer::{
    DepositRoute, ERC3009_DEPOSIT_COLLECTOR_ADDRESS, PERMIT2_ADDRESS,
    PERMIT2_DEPOSIT_COLLECTOR_ADDRESS,
};
pub use batch_settlement::EvmBatchSettlementBackend;
pub use batch_watch::{
    claim_target, settle_due, Claimed, EvmBatchWatcher, SweepReport, BATCH_SWEEP_INTERVAL,
    WITHDRAWAL_WATCH_INTERVAL,
};
/// A settlement table's endpoint, which [`EvmBatchSettlementBackend::connect`]
/// takes in place of a URL (ADR 0073).
/// Re-exported so a caller building one does not need a second dependency
/// to name it.
pub use connector_chain_rpc::RpcTransport;

use std::sync::Arc;

use connector_chain_rpc::evm::EvmRpc;
use connector_chain_rpc::retry_read;
use ethers::middleware::Middleware;
use ethers::providers::Provider;
use ethers::signers::{LocalWallet, Signer as EvmSigner};

use send::{ConfirmPolicy, Sender};

/// The client every contract binding reads through: a plain provider over
/// the table's transport, holding no key. Writes do not go through it; they
/// are built by a binding and handed to [`Sender`] (ADR 0073), which owns
/// the key, the nonce and the one lock concurrent writes are ordered by.
type EvmClient = Provider<EvmRpc>;

/// What [`build_client`] assembles from a transport and a key.
struct BuiltClient {
    client: Arc<EvmClient>,
    sender: Arc<Sender>,
    chain_id: u64,
    confirm: ConfirmPolicy,
}

/// The read client, the sender and the chain id, from `transport` and
/// `private_key`. The chain id is read once (retried, since it is boot's
/// first call) and bound into the key, so every signature carries it.
async fn build_client(transport: &RpcTransport, private_key: &str) -> Result<BuiltClient, String> {
    let client = Arc::new(EvmRpc::provider(transport.clone()));
    let chain_id = retry_read(|| client.get_chainid())
        .await
        .map_err(|error| error.to_string())?
        .as_u64();
    let wallet: LocalWallet = private_key.parse().map_err(|error| format!("{error}"))?;
    let sender = Arc::new(Sender::new(
        Arc::clone(&client),
        wallet.with_chain_id(chain_id),
    ));
    Ok(BuiltClient {
        client,
        sender,
        chain_id,
        confirm: ConfirmPolicy::for_transport(transport),
    })
}
