//! The proof issue #566 says nothing else in the epic can give: a claim
//! signed by this workspace's *production* peer signing path redeems
//! against the `TokenNetwork` actually deployed and resolved on Base
//! Sepolia -- through the same `TokenNetworkRegistry` every fleet config's
//! `[settlement.evm]` names (`infra/linode-store/connector-rust.toml` and
//! `infra/linode-relay/connector-rust.toml`, asserted identical by
//! `devnet_configs_load.rs`'s `FLEET_LIVE_REGISTRY`) -- not a disposable
//! local `anvil` fixture (issue #577).
//!
//! Every other chain-backed test in this workspace (`contract_suite.rs`,
//! `gas_and_nonce.rs`) proves the same backend against a `TokenNetwork`
//! this test run itself
//! deployed a minute ago. That is real proof that the Rust code and the
//! *source* `packages/contracts/src/TokenNetwork.sol` agree -- but #566's
//! own acceptance criterion is explicit that it is not enough: whether a
//! claim this workspace's signer produces is accepted by the bytecode
//! actually sitting on a public chain is only observable where the two
//! meet. `abi_provenance.rs` (issue #572) already proves the committed ABI
//! matches that bytecode; this file is what proves a *signature* does too.
//!
//! ## What each half of the production path this exercises actually is
//!
//! - **Sign**: [`connector_signer::LocalSigner::sign`] over
//!   [`connector_signer::evm_balance_proof_digest`] -- the exact call
//!   `connector_runtime::ClaimBook::record_fulfillment` makes when it signs
//!   an outbound claim (`connector-runtime/src/claim.rs:881`), not a
//!   hand-rolled digest.
//! - **Wire**: [`connector_runtime::WireClaim::encode`]/`decode` -- the
//!   exact peer-role byte shape (peer-semantics-pre-868.md §3.5), round-tripped
//!   before anything is submitted, so a bug in the wire codec would show up
//!   here as a decode failure rather than being silently bypassed.
//! - **Verify**: [`connector_signer::verify_evm_balance_proof`] -- the
//!   exact check `ClaimBook::verify_signature`
//!   (`connector-runtime/src/claim.rs:1020`) runs -- the first thing
//!   `accept_inbound_inner` does -- before a claim is ever handed to
//!   settlement.
//! - **Redeem**: [`connector_settlement_evm::EvmSettlementBackend::redeem`]
//!   -- which normalises the wire's raw libsecp256k1 `{0,1}` recovery id to
//!   the `{27,28}` range `TokenNetwork`'s `ECDSA.recover` requires (issue
//!   #590/#591) immediately before calling `claimFromChannel` for real.
//!
//! ## LOCAL / DEV ONLY, and inert unless driven
//!
//! Every test below returns immediately unless `BASE_SEPOLIA_PROOF_KEY` is
//! set -- the funded EVM private key that pays for opening a channel,
//! depositing into it, and every `claimFromChannel` call, exactly the
//! account `.github/workflows/funded-ops.yml` already knows how to derive
//! and hand to a job. Its absence is the ordinary case: a plain `cargo
//! test` run, or CI's automated gate, must never need a funded testnet key
//! just to compile-check this file.
//!
//! That single variable is also the ENTIRE opt-in gate. Once it is set,
//! every other input below is read with a hard-coded default matching the
//! committed fleet configs' `[settlement.evm]` section
//! (`BASE_SEPOLIA_PROOF_RPC`/`_REGISTRY`/`_TOKEN`/`_DECIMALS` may
//! override it, but nothing else silently no-ops). A caller that means to
//! run this -- a dispatched gate job -- must itself refuse to proceed on a
//! blank secret before it ever reaches `cargo test`, the same way
//! `funded-ops.yml`'s own `ops.mjs` does (`if (!MNEMONIC || !MNEMONIC.trim())
//! fail(...)`): a Rust `env::var` cannot tell "deliberately not opted in"
//! apart from "opted in with an empty secret" once both look like an absent
//! variable, so that distinction has to be enforced one layer up, by
//! whatever derives the key in the first place.
//!
//! The payer never touches a funded account at all: its key is generated
//! fresh for each run
//! ([`connector_signer::LocalSigner::generate`]) and never holds ETH or
//! USDC. `TokenNetwork.setTotalDeposit` lets the CALLER credit any
//! `participant`'s slot from the caller's own tokens
//! (`TokenNetwork.sol:255-286`), so the funded receiver key deposits
//! directly into the ephemeral payer's slot -- there is nothing for the
//! payer to sign on chain, only the off-chain balance proof this test
//! exists to prove redeems.
//!
//!   BASE_SEPOLIA_PROOF_KEY=<funded 32-byte hex key, NEVER committed> \
//!     cargo test -p connector --test base_sepolia_redeem_proof -- --nocapture
//!
//! ## Reproducibility
//!
//! A run prints, in order: the payer and receiver addresses, the channel
//! id `open` returned and the deposit `fund` moved, the two
//! deliberately-wrong domains attempted and the error each produced, the
//! correct claim's digest inputs, and the redeeming transaction's block and
//! hash -- everything issue #577's AC asks a reader who was not present to
//! be able to reconstruct: what was funded, what was signed, and the
//! transaction it produced.
//!
//! That last hash is read off the `ChannelClaimed` log this channel's claim
//! emitted, not off the latest block, so it always names the redeem itself
//! rather than whatever unrelated transaction shared its block -- see
//! `redeeming_transaction` below.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Duration as ChronoDuration;
use connector_runtime::{ClaimSignature, WireClaim};
use connector_settlement::{Claim, SettlementBackend, SettlementError};
use connector_settlement_evm::EvmSettlementBackend;
use connector_signer::{
    derive_evm_address, evm_balance_proof_digest, to_hex, verify_evm_balance_proof,
    EvmBalanceProof, LocalSigner, Signer,
};
use ethers::contract::{abigen, LogMeta};
use ethers::providers::{Http, Middleware, Provider};
use ethers::types::{Address as EvmAddress, U256, U64};

abigen!(
    Erc20,
    r#"[
        function balanceOf(address account) external view returns (uint256)
    ]"#
);

// `TokenNetwork`'s claim event, declared here rather than reused from
// `connector_settlement_evm`'s generated bindings because those are
// `pub(crate)` (`connector-settlement-evm/src/bindings.rs`) -- the same
// reason `Erc20` above is redeclared. Its shape is
// `packages/contracts/src/TokenNetwork.sol:171`, and it is what pins the
// transaction this run actually produced: see `redeeming_transaction`.
abigen!(
    TokenNetworkClaims,
    r#"[
        event ChannelClaimed(bytes32 indexed channelId, address indexed claimant, uint256 claimedAmount, uint256 totalClaimed)
    ]"#
);

/// The public endpoint both surviving fleet configs
/// (`infra/linode-store/`, `infra/linode-relay/`) and `funded-ops.yml`
/// already use.
const DEFAULT_RPC: &str = "https://base-sepolia-rpc.publicnode.com";
/// The `TokenNetworkRegistry` `[settlement.evm] contract_address` names --
/// a factory, resolved to a `TokenNetwork` at connect time, never pinned
/// directly (issue #566).
const DEFAULT_REGISTRY: &str = "0x0c41D9D424d6B075A3cEa1068a694f7847a8CCa5";
const DEFAULT_TOKEN: &str = "0x49beE1Bca5d15Fb0963117923403F9498119a9Ce";
const DEFAULT_DECIMALS: u8 = 6;

/// 0.001 USDC -- enough for `InsufficientChannelBalance` to never be the
/// reason a redeem fails, negligible as real value spent.
const DEPOSIT: u128 = 1_000;
const TRANSFERRED: u128 = 1;

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// The opt-in trigger and the only funded account this file ever spends
/// from -- see this module's own doc for why its absence is a silent skip
/// and why that is the caller's responsibility to guard, not this
/// function's.
fn receiver_key() -> Option<String> {
    env("BASE_SEPOLIA_PROOF_KEY")
}

fn rpc_url() -> String {
    env("BASE_SEPOLIA_PROOF_RPC").unwrap_or_else(|| DEFAULT_RPC.to_string())
}

fn registry() -> EvmAddress {
    env("BASE_SEPOLIA_PROOF_REGISTRY")
        .unwrap_or_else(|| DEFAULT_REGISTRY.to_string())
        .parse()
        .expect("BASE_SEPOLIA_PROOF_REGISTRY: not a 0x address")
}

fn token() -> EvmAddress {
    env("BASE_SEPOLIA_PROOF_TOKEN")
        .unwrap_or_else(|| DEFAULT_TOKEN.to_string())
        .parse()
        .expect("BASE_SEPOLIA_PROOF_TOKEN: not a 0x address")
}

fn decimals() -> u8 {
    env("BASE_SEPOLIA_PROOF_DECIMALS")
        .map(|v| v.parse().expect("BASE_SEPOLIA_PROOF_DECIMALS: not a u8"))
        .unwrap_or(DEFAULT_DECIMALS)
}

/// Seconds since the epoch, used as the balance proof's nonce -- always
/// greater than the `0` a fresh channel's `ParticipantState.nonce` starts
/// at, per `TokenNetwork.sol`'s `balanceProof.nonce > counterpartyState.nonce`.
fn now_nonce() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

fn parse_channel_id(id: &str) -> [u8; 32] {
    hex::decode(id.trim_start_matches("0x"))
        .expect("channel id is hex")
        .try_into()
        .expect("32-byte channel id")
}

/// A real `EvmSettlementBackend`, connected exactly the way a deployed
/// node's own is: through the registry, with the funded receiver key.
async fn receiver_backend(rpc: &str) -> EvmSettlementBackend {
    let key = receiver_key().expect("BASE_SEPOLIA_PROOF_KEY");
    EvmSettlementBackend::connect(
        &connector_settlement_evm::RpcTransport::direct(rpc).expect("rpc transport"),
        &key,
        registry(),
        token(),
        decimals(),
    )
    .await
    .expect("connect through the TokenNetworkRegistry")
}

/// Sign `proof` through the exact production path
/// `ClaimBook::record_fulfillment` uses -- deliberately no `+27` anywhere in
/// this file, so the wire's raw `{0,1}` recovery id and
/// `EvmSettlementBackend::redeem`'s own normalisation (issue #590/#591) are
/// what get proven, not a test helper standing in for them.
fn production_signature(
    signer: &LocalSigner,
    proof: &EvmBalanceProof,
) -> connector_signer::Signature {
    signer.sign(&evm_balance_proof_digest(proof)).expect("sign")
}

/// Round-trip `proof`/`signature` through the real peer-role codec and hand
/// back the on-chain redemption `Claim` the decoded side would submit --
/// the same construction `ClaimBook::latest_inbound_claim` performs
/// (`connector-runtime/src/claim.rs:798-802`).
fn wire_round_trip(
    channel_id: &str,
    proof: &EvmBalanceProof,
    signature: connector_signer::Signature,
) -> Claim {
    let sent = WireClaim {
        channel_id: channel_id.to_string(),
        nonce: proof.nonce,
        cumulative_amount: proof.transferred_amount as u64,
        signature: ClaimSignature::Evm(signature),
    };
    let encoded = sent.encode();
    let (decoded, consumed) =
        WireClaim::decode(&encoded).expect("decode a claim this file just encoded");
    assert_eq!(
        consumed,
        encoded.len(),
        "WireClaim::decode must consume exactly what encode wrote"
    );
    assert_eq!(decoded, sent, "a wire round trip must not change any field");

    Claim {
        nonce: decoded.nonce,
        cumulative_amount: decoded.cumulative_amount as u128,
        signature: decoded.signature.to_bytes(),
    }
}

/// The transaction that actually redeemed `channel_id` -- located by the
/// one artefact only this redeem could have produced.
///
/// [`SettlementBackend::redeem`] hands back a `ChannelState` and no
/// transaction hash, and Base Sepolia produces a block every ~2s that this
/// redeem shares with unrelated third-party traffic. So "the last
/// transaction of the latest block" is almost never the redeem -- printing
/// it under the words CLAIM REDEEMED would put a stranger's hash in the
/// record issue #577's AC asks a reader who was not present to reconstruct
/// from, which is worse than printing nothing. Instead the claim is found
/// by the `ChannelClaimed` log `claimFromChannel` emits for exactly this
/// channel id and this claimant (`TokenNetwork.sol:364`), searched from a
/// block number captured before the redeem was sent.
async fn redeeming_transaction(
    provider: &Provider<Http>,
    token_network: EvmAddress,
    from_block: U64,
    channel_id: [u8; 32],
    claimant: EvmAddress,
) -> (ChannelClaimedFilter, LogMeta) {
    let contract = TokenNetworkClaims::new(token_network, Arc::new(provider.clone()));
    let logs = contract
        .channel_claimed_filter()
        .from_block(from_block)
        .query_with_meta()
        .await
        .expect("read this TokenNetwork's ChannelClaimed logs");
    logs.into_iter()
        .find(|(event, _)| event.channel_id == channel_id && event.claimant == claimant)
        .expect("redeem confirmed, so its own ChannelClaimed log must be on chain")
}

#[tokio::test]
async fn a_production_signed_claim_redeems_on_the_deployed_token_network_and_a_wrong_domain_does_not(
) {
    if receiver_key().is_none() {
        eprintln!("BASE_SEPOLIA_PROOF_KEY not set -- skipping (see this file's module doc)");
        return;
    }

    let rpc = rpc_url();
    let backend = receiver_backend(&rpc).await;
    let receiver_address = backend.own_address();
    println!(
        "receiver (funded, redeems on chain): {}",
        to_hex(&receiver_address.0)
    );

    let payer_signer = LocalSigner::generate("base-sepolia-proof-payer");
    let payer_address = derive_evm_address(&payer_signer.public_key().expect("payer public key"));
    println!(
        "payer (ephemeral, signs only, holds nothing on chain): {}",
        to_hex(&payer_address)
    );

    // ── open + fund a real channel on the deployed TokenNetwork ──────────
    let channel = backend
        .open(payer_address.to_vec(), ChronoDuration::hours(1))
        .await
        .expect("open a real channel on the deployed TokenNetwork");
    println!("channel opened: {channel}");
    // The payer's slot, not this backend's own: the claim redeemed below
    // is signed by the payer and drawn from the payer's deposit. On the
    // real network the payer deposits it; this proof run stands in for
    // them with the fixture-only delegate deposit (issue #1118).
    let funded = backend
        .fund_counterparty(&channel, DEPOSIT)
        .await
        .expect("fund the payer's slot with real deposited value");
    assert_eq!(
        funded.counterparty_deposited, DEPOSIT,
        "a real transaction moved this"
    );
    println!("payer's slot funded: {DEPOSIT} base units");

    let channel_id_bytes = parse_channel_id(&channel.0);
    let real_chain_id = backend.chain_id();
    let real_token_network = backend.address();
    let nonce = now_nonce();

    // ── two deliberately wrong domains, each rejected by the chain ───────
    // Both keep every other field identical to the eventually-successful
    // claim below, so what fails here is specifically the domain the
    // signature was produced under -- not the channel id, not the amount,
    // not a malformed signature.
    let wrong_domains = [
        (
            "wrong chainId",
            EvmBalanceProof {
                channel_id: channel_id_bytes,
                nonce,
                transferred_amount: TRANSFERRED,
                locked_amount: 0,
                locks_root: [0u8; 32],
                chain_id: real_chain_id + 1,
                token_network_address: real_token_network.0,
            },
        ),
        (
            "wrong verifyingContract",
            EvmBalanceProof {
                channel_id: channel_id_bytes,
                nonce,
                transferred_amount: TRANSFERRED,
                locked_amount: 0,
                locks_root: [0u8; 32],
                chain_id: real_chain_id,
                // A real, differently-deployed address rather than the zero
                // address, so what the chain rejects is a signature bound to
                // the wrong `verifyingContract` and not one that is
                // structurally degenerate. This field never leaves the
                // signer: `redeem` rebuilds the on-chain `BalanceProof` from
                // the channel id, nonce and amount alone, so the only trace
                // of it on chain is the digest the signature recovers
                // against.
                token_network_address: payer_address,
            },
        ),
    ];

    for (label, proof) in wrong_domains {
        let signature = production_signature(&payer_signer, &proof);
        let claim = wire_round_trip(&channel.0, &proof, signature);
        let error = backend
            .redeem(&channel, claim)
            .await
            .expect_err(&format!("a claim signed under a {label} must be rejected"));
        println!("REJECTED ({label}): {error}");
        assert!(
            matches!(error, SettlementError::Backend(_)),
            "a wrong-domain claim must fail at the chain, not at a local precondition: {error:?}"
        );
    }

    // ── the correct domain redeems for real ───────────────────────────────
    let proof = EvmBalanceProof {
        channel_id: channel_id_bytes,
        nonce,
        transferred_amount: TRANSFERRED,
        locked_amount: 0,
        locks_root: [0u8; 32],
        chain_id: real_chain_id,
        token_network_address: real_token_network.0,
    };
    let signature = production_signature(&payer_signer, &proof);
    assert!(
        signature.recovery_id < 2,
        "the wire carries libsecp256k1's raw {{0,1}} recovery id (issue #590/#591), got {}",
        signature.recovery_id
    );
    let claim = wire_round_trip(&channel.0, &proof, signature);
    assert!(
        verify_evm_balance_proof(&proof, &claim.signature, &payer_address),
        "the exact check ClaimBook::accept_inbound_inner runs must accept this claim"
    );

    let provider = Provider::<Http>::try_from(rpc.as_str()).expect("provider");
    let usdc = Erc20::new(token(), Arc::new(provider.clone()));
    let before: U256 = usdc
        .balance_of(receiver_address)
        .call()
        .await
        .expect("balance before");

    // Captured BEFORE the redeem is sent, so the log search below has a
    // floor that cannot include an earlier claim on this channel and
    // cannot miss this one -- see `redeeming_transaction`.
    let search_from = provider
        .get_block_number()
        .await
        .expect("block number before the redeem");

    let state = backend
        .redeem(&channel, claim)
        .await
        .expect("a correctly-domained, production-signed claim must redeem");
    let after: U256 = usdc
        .balance_of(receiver_address)
        .call()
        .await
        .expect("balance after");

    assert_eq!(state.redeemed, TRANSFERRED);
    assert_eq!(after - before, U256::from(TRANSFERRED), "real value moved");

    let (event, meta) = redeeming_transaction(
        &provider,
        real_token_network,
        search_from,
        channel_id_bytes,
        receiver_address,
    )
    .await;
    assert_eq!(
        event.claimed_amount,
        U256::from(TRANSFERRED),
        "the located ChannelClaimed log must be this claim, not another"
    );
    println!(
        "CLAIM REDEEMED -- TokenNetwork {real_token_network:?} channel {channel} \
         block {} tx {:?}; receiver USDC {before} -> {after} (+{TRANSFERRED})",
        meta.block_number, meta.transaction_hash,
    );
}
