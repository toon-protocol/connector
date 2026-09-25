//! ADR 0073 decision 5, EVM half: boot's reads are retried before they fail
//! the node, and none of them can hang it.
//!
//! The endpoint here is a real `anvil` behind a `FakeRpc` that loses the
//! first answer to every method boot calls, the way a circuit still being
//! built loses a round trip.

mod support;

use connector_chain_rpc::{FakeRpc, RpcReply};
use connector_settlement_evm::{EvmSettlementBackend, RpcTransport};
use support::{require_anvil, Anvil, DEPLOYER_PRIVATE_KEY};

#[tokio::test]
async fn a_boot_that_loses_the_first_round_trip_of_every_read_still_connects() {
    if !require_anvil() {
        return;
    }
    let anvil = Anvil::spawn().await;
    let token = EvmSettlementBackend::deploy_mock_token(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, 1)
        .await
        .expect("deploy mock USDC");
    let deployed = EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
        .await
        .expect("deploy a TokenNetwork through a fresh registry");

    let flaky = FakeRpc::spawn_in_front_of(&anvil.rpc_url, |call| {
        if call.nth == 0 {
            RpcReply::Drop
        } else {
            RpcReply::Forward
        }
    })
    .await;
    let backend = EvmSettlementBackend::connect(
        &RpcTransport::direct(&flaky.url()).expect("transport"),
        DEPLOYER_PRIVATE_KEY,
        deployed.registry_address(),
        token,
        6,
    )
    .await
    .expect("every boot read is retried, so one lost round trip each is survivable");
    assert_eq!(backend.address(), deployed.address());
}
