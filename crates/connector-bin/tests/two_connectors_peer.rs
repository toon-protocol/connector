//! Issue #734: **two real connectors, peered, moving a paid packet** --
//! and, where that is not yet possible, exactly which wiring is missing.
//!
//! The repo used to have this test. `paid_write_end_to_end.rs`'s own header
//! records its deletion: `two_connectors_and_a_stub_app.rs`, *"since deleted
//! with the raw-TCP transport it proved -- ADR 0027, issue #679."* That
//! deletion was right and nothing replaced it, so the nine PRs that landed
//! the peer migration are verified only at unit and vector level, inside one
//! process.
//!
//! This file restores the capability at the level `main` can currently
//! support, and states -- mechanically, not in prose -- where that level
//! stops.
//!
//! # The five assertions #734 asks for, and where each one stands
//!
//! | # | Assertion | Here |
//! | - | --------- | ---- |
//! | 1 | a packet crosses the peering and is fulfilled | [`two_connectors_move_a_paid_packet_over_btp`] / [`_over_http`](two_connectors_move_a_paid_packet_over_http) -- **runs, green** |
//! | 2 | a claim advances the peer ledger and is acknowledged (§6) | [`a_peer_claim_is_acknowledged_over_btp`] / [`_over_http`](a_peer_claim_is_acknowledged_over_http) -- **runs, green** |
//! | 3 | the idempotent re-ack of a byte-identical retransmission (§6.3) | same two tests, second half -- **runs, green** |
//! | 4 | role-by-claim on a real socket (§1.9) | [`a_claim_that_fails_p2_or_p3_reaches_no_peer_handling_over_http`] and [`_over_btp`](a_claim_that_fails_p2_or_p3_reaches_no_peer_handling_over_btp) -- **runs, green** |
//! | 5 | both carriages | every test above exists in a `wss://` and an `https://` form |
//!
//! # The client leg (issue #620, ADR 0028)
//!
//! Assertion 1 above proves the *peer hop* is paid, and until ADR 0028 it
//! deliberately said nothing about the *client* leg -- it could not: the
//! payer's forwarded route was unpriced by construction, since `price`
//! alongside `peer_id` was a hard config error. A client therefore reached
//! the payee across the peering for nothing while the payer signed a real
//! peer claim for the value it carried, which is the free-gateway shape
//! issue #557 exists to catch and could not see.
//!
//! [`two_connectors_move_a_paid_packet`] now drives the whole path: a
//! claimless client is answered with the route's x402 terms quoting
//! [`CLIENT_PRICE`], and every crossing carries a real client claim that
//! lands in the payer's own client-edge journal. The `[[client_channels]]`
//! row and the second funded channel on [`PeerFixture`] are what make that
//! a real payment rather than a stubbed one.
//!
//! # What #678 closed, and where
//!
//! All five assertions run because issue #678 wired the three things this
//! file's own header used to list as missing. Named here because a reader
//! arriving from #734 needs to know where the wiring lives:
//!
//! 1. **The accept side is bound to this node's own listeners.**
//!    `connector-client-edge`'s `peer` module reads the covering claim on
//!    `POST /ilp` and on `GET /ilp/btp`, calls
//!    [`connector_peer_btp::role_gate::decide`], and hands a peer-role
//!    interaction to `connector-peer-http` or `connector-peer-btp`. There is
//!    **no second socket**: `docs/operators/btp-peer-transport-bringup.md`
//!    settles that peer carriages *"ride this node's own listeners"*, and
//!    §1.3 forbids the listener deciding role in any case.
//! 2. **The dial side is built from config.**
//!    `connector_cli::peer_transport` turns `[[peers]]` into a
//!    `PeerTransport` that dispatches by peer id -- `wss://` to
//!    `BtpPeerTransport`, `https://` to `HttpPeerTransport` -- and a peer it
//!    cannot dial still answers `T01` (§2.2). `[[peer_channels]]` reaches
//!    `ClaimBook` in the same commit, which is what makes a peer claim
//!    verifiable at all.
//! 3. **A plaintext endpoint is one explicit, default-false opt-in.**
//!    `peer_allow_plaintext_endpoints` lets `ws://` and `http://` resolve
//!    onto the same two carriages their TLS twins do, for loopback and
//!    tests. Every config that does not set it -- which is every deployed
//!    one -- still refuses them with `PeerEndpointScheme`, and a node that
//!    does set it logs a `WARN` naming every plaintext peering at startup.
//!    [`spawn_payer`] is the only config in this repo that sets it.
//!
//! # EVM only, on purpose (#732)
//!
//! Peer claims are EIP-712 balance proofs; `ClaimBook` verifies nothing else
//! here. `[[peer_channels]]` itself grew a Solana shape (issue #759,
//! `program_id` alongside `channel_account`/`counterparty_key`), and issue
//! #998 gave `connector-cli::runtime::wire_peer_channels` the Solana arm
//! that wires it into `ClaimBook` (`Connector::with_solana_channel`/
//! `with_solana_signer`) -- so a Solana row is no longer inert at the
//! config level, unit- and connector-level-tested in
//! `connector-runtime::connector::tests::
//! forwarding_to_a_solana_peer_signs_and_is_accepted_as_a_solana_claim` and
//! `connector-cli::runtime::tests::solana_peer_channels_reach_the_claim_ledger`.
//! This fixture's chain setup stays EVM regardless: proving it here needs a
//! disposable `solana-test-validator` running the real `payment-channel`
//! program alongside `anvil` in the same fixture, which is its own,
//! separately-scoped follow-up, not part of #998. Everything below is
//! therefore parameterised by *carriage*, never by chain, and the chain
//! setup is one function ([`PeerFixture::spawn`]). A future issue extends
//! this file by giving that function a Solana arm and the claim signer a
//! second shape -- not by adding a second test.

use std::io::Write as _;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use chrono::Duration as ChronoDuration;
use futures_util::{SinkExt as _, StreamExt as _};
use libsecp256k1::{Message, PublicKey, SecretKey};

use connector_btp::{
    decode_frame, encode_message, ProtocolData, BTP_RESPONSE, CLAIM_ACK_PROTOCOL, CLAIM_PROTOCOL,
    CONTENT_TYPE_TEXT,
};
use connector_domain::{Prepare, Reject};
use connector_settlement::SettlementBackend;
use connector_settlement_evm::test_support::{require_anvil, Anvil, DEPLOYER_PRIVATE_KEY};
use connector_settlement_evm::EvmSettlementBackend;
use connector_signer::{
    derive_evm_address, evm_balance_proof_digest, to_hex, EvmBalanceProof, PublicKeyBytes,
};

mod support;
use support::{
    identity_from_key_seed, sample_prepare, sealed_prepare_data, spawn_connector, spawn_stub_app,
    write_config, write_raw_key_file, ConnectorProcess,
};

/// `anvil`'s own default chain id, and so the EIP-712 domain a claim
/// against its deployed `TokenNetwork` must be signed under.
const ANVIL_CHAIN_ID: u64 = 31_337;

/// This test binary's own base port for [`Anvil::spawn`], distinct from
/// every other test binary's in this workspace (`devnet_configs_load.rs`
/// uses `18_500`, `connector-settlement-evm` `18_600`, `connector-cli`
/// `18_700`/`18_800`, `connector-client-edge` `18_900`,
/// `paid_write_end_to_end.rs` `19_000`) so concurrent binaries under
/// `cargo test --workspace` do not contend for a range.
const ANVIL_BASE_PORT: u16 = 19_100;

/// The peer route's fee, and so the amount one crossing of the peering owes
/// -- deliberately small so "the watermark advanced by exactly this" reads
/// plainly.
const PEER_FEE: u64 = 100;

/// The **client-facing** price of the payer's forwarded route (ADR 0028):
/// what the payer's own client edge charges a client for a packet to
/// [`APP_PREFIX`], as distinct from [`PEER_FEE`], which is only what the
/// payer retains of it.
///
/// Equal to [`peer_bound_prepare`]'s declared `amount`, which is the whole
/// arithmetic in one line: the payer collects `CLIENT_PRICE` from the
/// client, forwards `CLIENT_PRICE - PEER_FEE` to the payee
/// (`peer-semantics-pre-868.md` §4), and so earns exactly `PEER_FEE`. A larger
/// declared amount is refused -- see
/// [`a_client_may_not_declare_more_than_the_forwarded_route_charges`].
const CLIENT_PRICE: u64 = 10 * PEER_FEE;

/// The client's own EIP-712 signing identity -- not the payer's, and not
/// the payee's. A client leg is a *third* party to the peering: it holds
/// its own channel with the payer, on the same `TokenNetwork`, and the
/// payer's `[[client_channels]]` is what makes its signature recognisable.
const CLIENT_SECRET_SEED: u8 = 23;

/// **One id, written by both operators.** `[[peers]].id` names the peering
/// *relation*, and every `[[peer_channels]]` row of that relation binds its
/// channels under it -- so the peer id a verified claim resolves to only
/// matches a `[[routes]]` entry when the two files agree on the literal
/// string. The bring-up doc says the same thing from the other end: when
/// the `peer_auth_refused` event you expect never arrives, *"check the id
/// spelling on both sides"*.
///
/// This file used to carry two distinct ids -- `"alpha"` and `"beta"` --
/// on the reasoning that each side names the *other*. Nothing had ever
/// dialed, so nothing had contradicted it; the first real dial did, by
/// naming a relation the far side had no entry for.
const PEERING_ID: &str = "alpha-beta";

/// The payee, as the payer's `[[routes]]` and `[[peers]]` name it -- the
/// relation id, since that is the only name a peering has.
const PAYEE_ID: &str = PEERING_ID;

/// The payer, as the payee's `[[peers]]` names it. The same string, for the
/// same reason.
const PAYER_ID: &str = PEERING_ID;

/// `[signer] key_file` seeds, so a test can seal a packet to a spawned
/// binary's real identity without asking it over the wire.
const PAYER_SIGNER_SEED: u8 = 71;
const PAYEE_SIGNER_SEED: u8 = 72;

/// anvil's second pre-funded account. The payer is the deployer (it holds
/// the whole mock-ERC-20 supply and so is the side that can actually
/// deposit), which leaves this key for the payee's own settlement identity
/// -- it needs gas and nothing else, being structurally the side that is
/// owed.
const PAYEE_PRIVATE_KEY: &str = "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";

/// The prefix the payee terminates, and the prefix the payer routes to the
/// payee. The route is a strict prefix of the termination so a packet
/// addressed to the app has exactly one path: through the peering.
const PEER_ROUTE_PREFIX: &str = "g.example.beta";
const APP_PREFIX: &str = "g.example.beta.app";

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn channel_id_bytes(id: &str) -> [u8; 32] {
    let hex_digits = id.trim_start_matches("0x");
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex_digits[i * 2..i * 2 + 2], 16)
            .expect("channel id is 0x-prefixed 64-hex");
    }
    out
}

/// Sign `digest` the way the production signing path does
/// (`connector_signer::crypto::sign_digest`): 65 bytes `r || s || v` with
/// `v` in libsecp256k1's raw `{0, 1}` range, never the wallet `{27, 28}`
/// convention -- §4.2 is explicit that both carriages carry that byte
/// unchanged and the conversion happens only immediately before on-chain
/// submission.
fn sign_evm(secret: &SecretKey, digest: &[u8; 32]) -> Vec<u8> {
    let message = Message::parse(digest);
    let (signature, recovery_id) = libsecp256k1::sign(&message, secret);
    let mut bytes = signature.serialize().to_vec();
    let recovery_byte: u8 = recovery_id.into();
    bytes.push(recovery_byte);
    bytes
}

/// §4's claim, as a **single fixed string**.
///
/// The fixture keeps the rendered string rather than re-rendering it,
/// because §6.3's re-ack rule is about bytes: the claim JSON carries a
/// `timestamp`, so re-rendering with a fresh `now` produces a *different*
/// claim at the same nonce, which a payee MUST refuse `nonce_not_advancing`.
/// #727 found the payer must cache the exact string per channel for exactly
/// this reason; a test that re-rendered would be proving the wrong thing.
fn evm_claim_json(
    secret: &SecretKey,
    sender_id: &str,
    channel_id_hex: &str,
    nonce: u64,
    transferred_amount: u128,
    token_network_address: [u8; 20],
) -> String {
    let public = PublicKey::from_secret_key(secret);
    let address = derive_evm_address(&public.serialize());

    let proof = EvmBalanceProof {
        channel_id: channel_id_bytes(channel_id_hex),
        nonce,
        transferred_amount,
        locked_amount: 0,
        locks_root: [0u8; 32],
        chain_id: ANVIL_CHAIN_ID,
        token_network_address,
    };
    let signature = sign_evm(secret, &evm_balance_proof_digest(&proof));

    format!(
        r#"{{"version":"1.0","blockchain":"evm","messageId":"msg-{nonce}",\
"timestamp":"2026-02-02T12:00:00.000Z","senderId":"{sender_id}",\
"channelId":"{channel_id_hex}","nonce":{nonce},\
"transferredAmount":"{transferred_amount}","lockedAmount":"0",\
"locksRoot":"0x{zeros}","signature":"0x{signature}",\
"signerAddress":"{address}","chainId":{ANVIL_CHAIN_ID},\
"tokenNetworkAddress":"{token_network_address}"}}"#,
        zeros = "0".repeat(64),
        signature = hex_encode(&signature),
        address = to_hex(&address),
        token_network_address = to_hex(&token_network_address),
    )
    .replace("\\\n", "")
}

/// Which carriage a parameterised test is running over. §9 is blunt that a
/// peer behaviour on one carriage and not the other is a defect, so every
/// assertion in this file is written once and run twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Carriage {
    Btp,
    Http,
}

impl Carriage {
    /// The `[[peers]] endpoint` scheme that selects this carriage (§2.1).
    /// **Dial** and **expose** are separate axes, and so are their
    /// spellings: an endpoint's scheme is a URL scheme, `peer_expose`'s
    /// value is a carriage name. Confusing them is a load-time error
    /// (`InvalidPeerExposure`), which is the point of keeping the two
    /// spellings in one place.
    ///
    /// **Plaintext, because this harness stands up no TLS terminator**
    /// (issue #678, gap 3). The production spellings are `wss://` and
    /// `https://`, and they stay the only ones a config accepts unless it
    /// sets `peer_allow_plaintext_endpoints` -- which [`spawn_payer`] does
    /// and every deployed config does not. `ws://` selects the same BTP
    /// carriage `wss://` does and `http://` the same ILP-over-HTTP one:
    /// the switch widens which schemes resolve, never what they resolve
    /// to.
    fn scheme(self) -> &'static str {
        match self {
            Carriage::Btp => "ws",
            Carriage::Http => "http",
        }
    }

    /// The `peer_expose` value that opens a listener for this carriage
    /// (§2.1).
    fn expose(self) -> &'static str {
        match self {
            Carriage::Btp => "btp",
            Carriage::Http => "http",
        }
    }
}

/// A real chain, a real `TokenNetwork`, and a real peer channel funded by
/// the payer -- everything below the carriage, shared by every test here.
///
/// One struct rather than a helper per test because #732's Solana extension
/// is a second constructor here and nothing else; if the chain setup were
/// inlined per test, extending it would mean touching every test.
struct PeerFixture {
    _anvil: Anvil,
    rpc_url: String,
    registry_address: ethers::types::Address,
    token: ethers::types::Address,
    token_network_address: [u8; 20],
    payer_secret: SecretKey,
    payer_address: [u8; 20],
    payee_address: [u8; 20],
    channel_id: String,
    /// The **client leg** (ADR 0028): a second real, funded channel on the
    /// same `TokenNetwork`, opened by the payer naming an ordinary client
    /// as counterparty. Distinct from `channel_id` in every sense that
    /// matters -- a different counterparty, a different namespace
    /// (`[[client_channels]]` rather than `[[peer_channels]]`, which
    /// `ChannelInBothNamespaces` refuses to conflate) and a different
    /// watermark journal.
    client_secret: SecretKey,
    client_address: [u8; 20],
    client_channel_id: String,
}

impl PeerFixture {
    /// Spawn the chain and fund the peering's channel. `None` when `anvil`
    /// is unavailable, so callers report the same skip
    /// `paid_write_end_to_end.rs` does rather than each inventing one.
    async fn spawn() -> Option<PeerFixture> {
        if !require_anvil() {
            return None;
        }
        let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
        let token = EvmSettlementBackend::deploy_mock_token(
            &anvil.rpc_url,
            DEPLOYER_PRIVATE_KEY,
            1_000_000,
        )
        .await
        .expect("mint a fresh mock ERC-20 for this test");
        // The payer *is* the deployer: it holds the supply, so it is the
        // side that can genuinely deposit, and debt flows in the direction
        // packets flow (§6.4).
        let payer_backend =
            EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
                .await
                .expect("deploy a TokenNetwork through a fresh registry");
        let token_network_address = payer_backend.address().to_fixed_bytes();
        let registry_address = payer_backend.registry_address();
        let payer_address = payer_backend.own_address().to_fixed_bytes();

        let payer_secret = SecretKey::parse_slice(
            &hex::decode(DEPLOYER_PRIVATE_KEY.trim_start_matches("0x"))
                .expect("deployer key is hex"),
        )
        .expect("deployer key is a valid secp256k1 secret");

        let payee_backend = EvmSettlementBackend::connect(
            &anvil.rpc_url,
            PEER_SETTLEMENT_KEY_PLACEHOLDER,
            registry_address,
            token,
            6,
        )
        .await
        .expect("connect the payee's settlement identity to the same TokenNetwork");
        let payee_address = payee_backend.own_address().to_fixed_bytes();

        // The payer opens the peering's channel naming the payee, and puts
        // its OWN collateral behind it -- read back from the chain's own
        // receipt, never invented here. The payer is the side that signs
        // claims here, so the payer's own deposit is what backs them:
        // `fund` is a self-deposit (issue #1118), and this is the shape it
        // exists for.
        let channel = payer_backend
            .open(payee_address.to_vec(), ChronoDuration::hours(1))
            .await
            .expect("the payer opens the peering's channel");
        let state = payer_backend
            .fund(&channel, u128::from(100 * PEER_FEE))
            .await
            .expect("fund the peering channel with real ERC-20 value");
        assert_eq!(
            state.own_deposited,
            u128::from(100 * PEER_FEE),
            "a real transaction genuinely moved this value on chain"
        );

        // The client leg: a second channel on the same `TokenNetwork`,
        // opened by the payer naming an ordinary client, and funded from
        // the same real supply. Nothing about it is synthetic -- the
        // payer's claim gate resolves it on chain like any other, and an
        // undercollateralised one would be refused (issue #646).
        let client_secret = SecretKey::parse(&[CLIENT_SECRET_SEED; 32])
            .expect("the client's secp256k1 secret is valid");
        let client_address =
            derive_evm_address(&PublicKey::from_secret_key(&client_secret).serialize());
        let client_channel = payer_backend
            .open(client_address.to_vec(), ChronoDuration::hours(1))
            .await
            .expect("the payer opens a client channel");
        // The other direction: on the client leg the *client* signs and
        // the payer redeems, so the collateral has to sit on the client's
        // side. On a real deployment the client deposits it themselves;
        // here the fixture-only delegate deposit stands in (issue #1118).
        payer_backend
            .fund_counterparty(&client_channel, u128::from(100 * CLIENT_PRICE))
            .await
            .expect("fund the client channel with real ERC-20 value");

        Some(PeerFixture {
            rpc_url: anvil.rpc_url.clone(),
            _anvil: anvil,
            registry_address,
            token,
            token_network_address,
            payer_secret,
            payer_address,
            payee_address,
            channel_id: channel.0,
            client_secret,
            client_address,
            client_channel_id: client_channel.0,
        })
    }

    /// The claim the payer would sign for the `n`th crossing of this
    /// peering: cumulative `n * PEER_FEE` at nonce `n`.
    fn claim(&self, nonce: u64) -> String {
        evm_claim_json(
            &self.payer_secret,
            PAYER_ID,
            &self.channel_id,
            nonce,
            u128::from(nonce) * u128::from(PEER_FEE),
            self.token_network_address,
        )
    }

    /// The claim a **client** would sign for the `n`th packet it sends
    /// across the payer's forwarded route: cumulative `n * CLIENT_PRICE` at
    /// nonce `n`, on the client channel rather than the peering's.
    fn client_claim(&self, nonce: u64) -> String {
        evm_claim_json(
            &self.client_secret,
            "client",
            &self.client_channel_id,
            nonce,
            u128::from(nonce) * u128::from(CLIENT_PRICE),
            self.token_network_address,
        )
    }

    /// The `[settlement]` block naming the chain this fixture just
    /// deployed, for whichever side `key_file` belongs to.
    fn settlement_block(&self, key_file: &std::path::Path) -> String {
        format!(
            r#"
[settlement]
chain = "evm"
rpc_url = "{rpc_url}"
contract_address = "{registry:?}"
token_address = "{token:?}"
decimals = 6

[settlement.key]
key_file = "{key_file}"
"#,
            rpc_url = self.rpc_url,
            registry = self.registry_address,
            token = self.token,
            key_file = key_file.display(),
        )
    }
}

/// The payee's settlement private key, as `EvmSettlementBackend::connect`
/// takes it. Named separately from [`PAYEE_PRIVATE_KEY`] only because the
/// constant is used both here and to write the spawned binary's key file,
/// and a reader should see that they are the same key.
const PEER_SETTLEMENT_KEY_PLACEHOLDER: &str = PAYEE_PRIVATE_KEY;

/// Spawn the **payee**: a real compiled `connector` binary that terminates
/// [`APP_PREFIX`] at a real stub app, exposes both peer carriages, and
/// holds a `[[peer_channels]]` binding for the payer -- so a claim the
/// payer signs on that channel satisfies both P2 and P3 (§1.2), and
/// nothing else does.
fn spawn_payee(
    fixture: &PeerFixture,
    state_dir: &std::path::Path,
    stub_app_addr: &str,
) -> (
    ConnectorProcess,
    tempfile::NamedTempFile,
    tempfile::NamedTempFile,
) {
    let key_file = write_raw_key_file(PAYEE_SIGNER_SEED);
    let mut settlement_key_file = tempfile::NamedTempFile::new().expect("temp settlement key file");
    settlement_key_file
        .write_all(PAYEE_PRIVATE_KEY.as_bytes())
        .expect("write settlement key file");

    let config = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"
peer_expose = "both"

[signer]
key_file = "{key_file}"
{settlement}
[[routes]]
prefix = "{APP_PREFIX}"
handler_url = "http://{stub_app_addr}"
price = 0

# The peering this node accepts. No `endpoint`: the payer dials us, which
# on HTTP makes us structurally the payee (§6.4).
[[peers]]
id = "{PAYER_ID}"

# P2: the channel this relation's claims are judged against, and the
# EIP-712 domain they are signed under (ADR 0024, §11).
[[peer_channels]]
peer_id = "{PAYER_ID}"
channel_id = "{channel_id}"
counterparty_key = "{payer}"
chain_id = {ANVIL_CHAIN_ID}
token_network = "{token_network}"

# §1.9's P2-alone case -- a peering with no `[[peer_channels]]` row -- is
# deliberately *absent* here, because it cannot be written:
# `ConfigError::PeerChannelUnbound` refuses it at load. See
# `a_peer_with_no_channel_binding_refuses_to_start`, which is the only form
# that case can take against a live binary.
"#,
        state_dir = state_dir.display(),
        key_file = key_file.path().display(),
        settlement = fixture.settlement_block(settlement_key_file.path()),
        channel_id = fixture.channel_id,
        payer = to_hex(&fixture.payer_address),
        token_network = to_hex(&fixture.token_network_address),
    ));
    let connector = spawn_connector(config.path());
    (connector, config, settlement_key_file)
}

/// Spawn the **payer**: a real compiled `connector` binary whose only route
/// to [`APP_PREFIX`] is the peering, dialed at `payee_endpoint` on
/// `carriage` -- priced at [`CLIENT_PRICE`] for its own clients (ADR 0028)
/// and holding a `[[client_channels]]` row for the one this fixture funded,
/// so both legs of the path it sits on are payable.
fn spawn_payer(
    fixture: &PeerFixture,
    state_dir: &std::path::Path,
    carriage: Carriage,
    payee_endpoint: &str,
    payee_client_edge: &str,
) -> (
    ConnectorProcess,
    tempfile::NamedTempFile,
    tempfile::NamedTempFile,
) {
    let key_file = write_raw_key_file(PAYER_SIGNER_SEED);
    let mut settlement_key_file = tempfile::NamedTempFile::new().expect("temp settlement key file");
    settlement_key_file
        .write_all(DEPLOYER_PRIVATE_KEY.as_bytes())
        .expect("write settlement key file");

    let config = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"
peer_expose = "{expose}"
# Issue #678, gap 3. The payee below is a plain loopback socket with no TLS
# terminator in front of it, and a peer endpoint is `wss://`/`https://`
# only unless a node says otherwise -- one top-level line, default false,
# and the node logs a WARN naming every plaintext peering at startup.
peer_allow_plaintext_endpoints = true

[signer]
key_file = "{key_file}"
{settlement}
# The only path to the app: a `peer_id`-targeted route. Nothing about this
# node terminates `{APP_PREFIX}`, so a fulfilled packet addressed there can
# only have crossed the peering.
#
# ADR 0028's `price`, and the whole of issue #620's client leg: what this
# node's own client edge charges a client for a packet to this prefix --
# greeted, claim-gated and journaled exactly as a terminated route's price
# is. Before ADR 0028 `price` here was a hard config error, which is
# precisely why a forwarded route was a free gateway that also paid its own
# peer. What this hop retains of it is the peering's `fee` below (ADR 0061),
# and the difference is what reaches the payee.
[[routes]]
prefix = "{PEER_ROUTE_PREFIX}"
peer_id = "{PAYEE_ID}"
price = {CLIENT_PRICE}

[[peers]]
id = "{PAYEE_ID}"
endpoint = "{payee_endpoint}"
fee = {PEER_FEE}

[[peer_channels]]
peer_id = "{PAYEE_ID}"
channel_id = "{channel_id}"
counterparty_key = "{payee}"
chain_id = {ANVIL_CHAIN_ID}
token_network = "{token_network}"

# ADR 0042, and REQUIRED of any peering this node forwards to since issue
# #1145: the channel this node pays the payee from, as an ordinary client
# of it, covering each PREPARE **before** it is sent. Without this row the
# binary refuses to start (`ConfigError::PayChannelUnbound`), because there
# is no longer an uncovered path for a forward to take -- ADR 0004's
# postpay convention, where the claim covering crossing n rode crossing
# n + 1, is deleted.
#
# The same `channel_id` the `[[peer_channels]]` row above names, which is
# the deployed shape: the peer role for what arrives, the client role for
# what this node sends. `client_edge_url` is the payee's own `POST /ilp`,
# where this node asks where its claims on that channel stand -- answered
# out of the payee's PEER book, because that is the book the channel is
# bound in there (issue #1102). It is HTTP even when the peering itself is
# carried over BTP: the claim-state ask is its own request, and a `wss://`
# peering has no HTTP client edge to derive one from, which is why this is
# written out rather than inferred.
[[pay_channels]]
peer_id = "{PAYEE_ID}"
channel_id = "{channel_id}"
chain_id = {ANVIL_CHAIN_ID}
token_network = "{token_network}"
client_edge_url = "{payee_client_edge}"

# The client leg's channel, in the *other* namespace (§1.8): whose
# signature this node accepts on a claim presented at its client edge. A
# channel id may appear in exactly one of the two tables --
# `ChannelInBothNamespaces` refuses a file that lists one in both -- so
# these two rows are two genuinely separate relationships, judged against
# two separate watermark journals.
[[client_channels]]
channel_id = "{client_channel_id}"
counterparty = "{client}"
chain_id = {ANVIL_CHAIN_ID}
token_network_address = "{token_network}"
"#,
        state_dir = state_dir.display(),
        key_file = key_file.path().display(),
        expose = carriage.expose(),
        settlement = fixture.settlement_block(settlement_key_file.path()),
        channel_id = fixture.channel_id,
        payee = to_hex(&fixture.payee_address),
        payee_client_edge = payee_client_edge,
        client_channel_id = fixture.client_channel_id,
        client = to_hex(&fixture.client_address),
        token_network = to_hex(&fixture.token_network_address),
    ));
    let connector = spawn_connector(config.path());
    (connector, config, settlement_key_file)
}

/// The peer claim journal a node keeps under its `state_dir`
/// (`connector_cli::runtime`'s `PEER_CLAIM_JOURNAL`). Reading it is how
/// §1.9's *"nothing was appended to the peer claim ledger"* is asserted
/// against a live binary rather than against a fake -- and how §6's
/// *"a claim advanced the peer ledger"* is asserted without asking the node
/// to describe itself.
fn peer_journal(state_dir: &std::path::Path) -> String {
    std::fs::read_to_string(state_dir.join("peer-claims.log")).unwrap_or_default()
}

/// A packet addressed **across** the peering, carrying enough value to
/// survive the hop's own fee.
///
/// `support::sample_prepare` mints an `amount: 0` packet, which is right
/// for a terminated route and wrong for a forwarding one: a hop forwards
/// `A - fee` and refuses `R01` when its own fee alone exceeds `A` (ADR
/// 0010; RFC 0027's own meaning for that code, which ADR 0057 as corrected
/// leaves standing), so a zero-amount packet never reaches the peer
/// transport at all -- it is refused one layer earlier, by arithmetic. A
/// test that used a zero amount here would be asserting the fee check, not
/// the peering.
fn peer_bound_prepare(destination: &str, body: &'static [u8], payee: &PublicKeyBytes) -> Prepare {
    let (data, _shared_secret) = sealed_prepare_data(body, payee);
    Prepare {
        amount: 10 * PEER_FEE,
        ..sample_prepare(destination, data)
    }
}

/// Present `claim` to a running connector's HTTP carriage exactly as §3/§4
/// require, and return the response's status, body and `Toon-Claim-Ack`
/// header.
///
/// There is no credential parameter and no `Toon-Peer-Auth` header: ADR
/// 0060 deleted both. The claim is the whole of what identifies the peering
/// (§1.2), which is why every one of these calls carries one or deliberately
/// does not.
async fn post_peer_request(
    client: &reqwest::Client,
    addr: &str,
    claim: Option<&str>,
    prepare: &Prepare,
) -> (reqwest::StatusCode, Vec<u8>, Option<String>) {
    let mut request = client
        .post(format!("http://{addr}/ilp"))
        .body(prepare.encode());
    if let Some(claim) = claim {
        request = request.header("ilp-payment-channel-claim", BASE64.encode(claim));
    }
    let response = request.send().await.expect("POST /ilp");
    let status = response.status();
    let ack = response
        .headers()
        .get("toon-claim-ack")
        .map(|value| value.to_str().expect("ack header is ASCII").to_string());
    let body = response.bytes().await.expect("response body").to_vec();
    (status, body, ack)
}

/// The BTP twin of [`post_peer_request`]: one websocket session to
/// `/ilp/btp` carrying **one** MESSAGE -- the claim entry and the OER
/// PREPARE together -- returning its answer and its `claim-ack` entry's
/// payload, if one rode back.
///
/// **One frame, not two.** It used to send an `auth` frame first, because a
/// credential bound the session's role and §1.5 required that to happen
/// before anything else. ADR 0060 deleted the credential and §1.5 inverted:
/// role is a property of the frame, decided from the claim that frame
/// carries, so the claim and the packet ride together and there is nothing
/// to send ahead of them.
async fn send_peer_message(
    addr: &str,
    claim: Option<&str>,
    prepare: &Prepare,
) -> (Vec<u8>, Option<String>) {
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ilp/btp"))
        .await
        .expect("upgrade the peer carriage's websocket");

    let mut protocol_data = Vec::new();
    if let Some(claim) = claim {
        protocol_data.push(ProtocolData {
            name: CLAIM_PROTOCOL.to_string(),
            content_type: CONTENT_TYPE_TEXT,
            data: claim.as_bytes().to_vec(),
        });
    }
    let frame = encode_message(2, &protocol_data, &prepare.encode());
    socket
        .send(tokio_tungstenite::tungstenite::Message::Binary(frame))
        .await
        .expect("send the peer MESSAGE");

    let reply = next_binary(&mut socket).await;
    let decoded = decode_frame(&reply).expect("decode the answering frame");
    assert_eq!(
        decoded.frame_type, BTP_RESPONSE,
        "§6.2: a claim verdict is never a BTP ERROR -- ERROR stays reserved for \
         undecodable frames"
    );
    let ack = decoded
        .protocol_data
        .iter()
        .find(|entry| entry.name == CLAIM_ACK_PROTOCOL)
        .map(|entry| String::from_utf8(entry.data.clone()).expect("ack entry is UTF-8"));
    (decoded.ilp_packet, ack)
}

/// The next binary frame off a websocket, skipping whatever ping/pong or
/// text the transport layer interleaves.
async fn next_binary<S>(socket: &mut S) -> Vec<u8>
where
    S: futures_util::Stream<
            Item = Result<
                tokio_tungstenite::tungstenite::Message,
                tokio_tungstenite::tungstenite::Error,
            >,
        > + Unpin,
{
    loop {
        let message = socket
            .next()
            .await
            .expect("the session answered")
            .expect("websocket read");
        if let tokio_tungstenite::tungstenite::Message::Binary(bytes) = message {
            return bytes;
        }
    }
}

// ---------------------------------------------------------------------------
// Assertion 4 (§1.9), on a live binary, over both carriages. These run.
// ---------------------------------------------------------------------------

/// **The named regression, over HTTP** (`peer-carriage-spec.md` §1.9).
///
/// The invariant exists because the TypeScript fleet violated it:
/// `toon-sandbox` admitted an anonymous BTP session with
/// `btp_auth … success:true mode:"no-auth"` and then treated it as a
/// quasi-peer. §1.9 requires **both carriages** to carry a stop-ship
/// regression named for it, asserting that each shape below is classified
/// `client` and reaches no peer handling whatsoever.
///
/// "Reaches no peer handling" is asserted here the way §1.9 defines it: no
/// `Toon-Claim-Ack` was emitted, and nothing was appended to the peer claim
/// ledger -- read off the binary's own `state_dir`, not from anything the
/// node says about itself.
///
/// Since ADR 0060 the discriminator **is** the claim: what each case varies
/// is which claim rides the request, because there is nothing else left to
/// vary. The positive control is the other tests in this file, every one of
/// which presents a genuine claim on the bound channel and is admitted.
#[tokio::test]
async fn a_claim_that_fails_p2_or_p3_reaches_no_peer_handling_over_http() {
    let Some(fixture) = PeerFixture::spawn().await else {
        return;
    };
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let stub_app = spawn_stub_app();
    let (payee, _config, _key) = spawn_payee(&fixture, state_dir.path(), &stub_app.addr);
    let payee_identity = identity_from_key_seed(PAYEE_SIGNER_SEED);
    let client = reqwest::Client::new();

    for (case, claim) in fixture.refused_claims() {
        let (data, _shared) = sealed_prepare_data(case.as_bytes(), &payee_identity);
        let prepare = sample_prepare(APP_PREFIX, data);
        let (status, _body, ack) =
            post_peer_request(&client, &payee.client_edge_addr, claim.as_deref(), &prepare).await;
        assert!(
            status.is_success() || status == reqwest::StatusCode::BAD_REQUEST,
            "{case}: §6.2 reserves non-200 for a malformed request, never a claim verdict"
        );
        assert_eq!(
            ack, None,
            "{case}: §1.7 -- a connector MUST NOT emit a claim-ack on a client interaction"
        );
        assert_eq!(
            peer_journal(state_dir.path()),
            "",
            "{case}: §1.9 -- nothing may be appended to the peer claim ledger"
        );
    }
}

/// The BTP twin of
/// [`a_claim_that_fails_p2_or_p3_reaches_no_peer_handling_over_http`].
/// §1.9 requires the regression on **both** carriages, and §9 makes any
/// peer behaviour present on one and absent on the other a defect rather
/// than a carriage property -- so this is the same cases, the same two
/// observations, over a real websocket to the same real binary.
#[tokio::test]
async fn a_claim_that_fails_p2_or_p3_reaches_no_peer_handling_over_btp() {
    let Some(fixture) = PeerFixture::spawn().await else {
        return;
    };
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let stub_app = spawn_stub_app();
    let (payee, _config, _key) = spawn_payee(&fixture, state_dir.path(), &stub_app.addr);
    let payee_identity = identity_from_key_seed(PAYEE_SIGNER_SEED);

    for (case, claim) in fixture.refused_claims() {
        let (data, _shared) = sealed_prepare_data(case.as_bytes(), &payee_identity);
        let prepare = sample_prepare(APP_PREFIX, data);
        let (_packet, ack) =
            send_peer_message(&payee.client_edge_addr, claim.as_deref(), &prepare).await;
        assert_eq!(
            ack, None,
            "{case}: §1.7 -- a connector MUST NOT emit a claim-ack on a client interaction"
        );
        assert_eq!(
            peer_journal(state_dir.path()),
            "",
            "{case}: §1.9 -- nothing may be appended to the peer claim ledger"
        );
    }
}

/// §1.9's cases that a **wire** can present, as `(name, claim)` -- shared
/// so the two carriages cannot drift in *which* cases they cover, which is
/// exactly the drift §9 warns about.
///
/// §1.9's P2-alone case -- a peering with **no** `[[peer_channels]]` entry
/// -- is missing because **it cannot be presented to a Rust connector at
/// all**: this implementation refuses such a peering at config load
/// (`ConfigError::PeerChannelUnbound`), so no running binary can hold one
/// for a claim to name. That is a *stronger* position than §1.9 requires,
/// and it moves the case from the wire to startup; the live-binary form of
/// it is [`a_peer_with_no_channel_binding_refuses_to_start`].
impl PeerFixture {
    fn refused_claims(&self) -> Vec<(&'static str, Option<String>)> {
        // A key that is nobody's counterparty on this channel.
        let stranger = SecretKey::parse(&[0x5au8; 32]).expect("a valid secp256k1 key");
        // A channel this node holds no `[[peer_channels]]` row for.
        let unbound = format!("0x{:064x}", 0xdeadu64);
        vec![
            ("no claim at all", None),
            (
                "a claim whose signature does not recover to the row's key",
                Some(evm_claim_json(
                    &stranger,
                    PAYER_ID,
                    &self.channel_id,
                    1,
                    u128::from(PEER_FEE),
                    self.token_network_address,
                )),
            ),
            (
                "a claim on a channel no [[peer_channels]] row binds",
                Some(evm_claim_json(
                    &self.payer_secret,
                    PAYER_ID,
                    &unbound,
                    1,
                    u128::from(PEER_FEE),
                    self.token_network_address,
                )),
            ),
            (
                "a claim header that is not a claim",
                Some("not a claim at all".to_string()),
            ),
        ]
    }
}

/// **§1.9 case 4, in the only form a live binary can express it.**
///
/// The case is *"a correct `peerId` and correct secret for a peer with no
/// `[[peer_channels]]` entry"*, which §1.9 requires be classified `client`.
/// A Rust connector never gets that far: `ConfigError::PeerUnbound` refuses
/// the configuration outright, so the peering the credential would name
/// does not exist to be authenticated against.
///
/// Asserted here rather than left to `connector-config`'s own unit test
/// because the property #734 cares about is what the **process** does. A
/// node that started and merely logged a warning would satisfy the config
/// test and violate this one -- and it is the process that would then be
/// carrying a peering nobody can account for.
#[test]
fn a_peer_with_no_channel_binding_refuses_to_start() {
    let key_file = write_raw_key_file(PAYEE_SIGNER_SEED);
    let config = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
peer_expose = "both"

[signer]
key_file = "{key_file}"

[[peers]]
id = "unbound"
"#,
        key_file = key_file.path().display(),
    ));

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_connector"))
        .arg(config.path())
        .output()
        .expect("run the connector binary");
    assert!(
        !output.status.success(),
        "§1.2 P2: a peering with no channel binding can never take the peer role, \
         so the node must refuse to start rather than carry it"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unbound") && stderr.contains("[[peer_channels]]"),
        "the refusal must name the peering and the missing table, not fail \
         generically: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Assertions 1, 2 and 3, over both carriages. Unblocked by #678.
// ---------------------------------------------------------------------------

/// **Assertion 1, over `wss://`: a packet crosses the peering and is
/// fulfilled.**
///
/// A sends, B terminates at a real stub app, the FULFILL comes back. The
/// payer terminates nothing under [`APP_PREFIX`], so a fulfilled answer can
/// only have crossed the peering.
///
/// Unblocked by **issue #678**, whose three gaps this exercises at once:
/// the payer builds its dialer from `[[peers]]`, the payee's own listener
/// serves the peer carriage beside the client edge, and
/// `peer_allow_plaintext_endpoints` lets the two find each other on
/// loopback with no TLS terminator in the harness. Nothing here is stubbed
/// around: a stub of the accept side would be a test of the stub.
#[tokio::test]
async fn two_connectors_move_a_paid_packet_over_btp() {
    two_connectors_move_a_paid_packet(Carriage::Btp).await;
}

/// **Assertion 1, over `https://`.** The same proof on the other carriage:
/// §9 makes any peer behaviour present on one and absent on the other a
/// defect, so the packet path is asserted twice or not at all.
///
/// One property is genuinely HTTP's and is not a drift (§6.4): only the
/// dialing side can originate, so this peering is unidirectional for
/// packets and the payer is structurally the payer. That is why the payee's
/// `[[peers]]` entry carries no `endpoint`.
#[tokio::test]
async fn two_connectors_move_a_paid_packet_over_http() {
    two_connectors_move_a_paid_packet(Carriage::Http).await;
}

/// A **claimless** client PREPARE to `destination` on `client_edge_addr`,
/// and the x402 terms it must be answered with (client-edge-spec.md §1.4).
///
/// Claimless in the strictest sense the wire allows: no
/// `ILP-Payment-Channel-Claim`, no wrapped twin, no `Toon-Peer-Auth`. This
/// is an anonymous, unpaying client, and it is the exact shape that used to
/// be forwarded across the peering for nothing.
async fn claimless_client_terms(
    client: &reqwest::Client,
    client_edge_addr: &str,
    prepare: &Prepare,
) -> serde_json::Value {
    let response = client
        .post(format!("http://{client_edge_addr}/ilp"))
        .body(prepare.encode())
        .send()
        .await
        .expect("POST /ilp");
    assert_eq!(
        response.status(),
        reqwest::StatusCode::PAYMENT_REQUIRED,
        "a claimless client request to a priced forwarded route must be greeted, \
         not carried"
    );
    response.json().await.expect("x402 JSON terms")
}

/// The client-edge claim journal the payer keeps under its `state_dir`
/// (`connector_cli::runtime`'s `CLIENT_EDGE_JOURNAL`) -- the *client* leg's
/// counterpart to [`peer_journal`], and how "the client was charged" is
/// asserted against a live binary rather than taken on the node's word.
fn client_journal(state_dir: &std::path::Path) -> String {
    std::fs::read_to_string(state_dir.join("client-edge-claims.log")).unwrap_or_default()
}

async fn two_connectors_move_a_paid_packet(carriage: Carriage) {
    let Some(fixture) = PeerFixture::spawn().await else {
        return;
    };
    let payee_state = tempfile::tempdir().expect("temp payee state dir");
    let payer_state = tempfile::tempdir().expect("temp payer state dir");
    let stub_app = spawn_stub_app();
    let (payee, _payee_config, _payee_key) =
        spawn_payee(&fixture, payee_state.path(), &stub_app.addr);

    // The payee's own listener, plaintext: this harness stands up no TLS
    // terminator, and `peer_allow_plaintext_endpoints` on the payer's
    // config (issue #678, gap 3) is what lets a `ws://`/`http://` endpoint
    // resolve onto the same two carriages `wss://`/`https://` do. The path
    // is the client edge's own -- peer carriages ride this node's
    // listeners, not a second socket.
    let endpoint = format!(
        "{}://{}{}",
        carriage.scheme(),
        payee.client_edge_addr,
        match carriage {
            Carriage::Btp => "/ilp/btp",
            Carriage::Http => "/ilp",
        }
    );
    let payee_client_edge = format!("http://{}/ilp", payee.client_edge_addr);
    let (payer, _payer_config, _payer_key) = spawn_payer(
        &fixture,
        payer_state.path(),
        carriage,
        &endpoint,
        &payee_client_edge,
    );

    // The packet is sealed to the **payee's** identity: it terminates
    // there, and the payer is a forwarding hop that cannot open it (§8.1).
    let payee_identity = identity_from_key_seed(PAYEE_SIGNER_SEED);
    let client = reqwest::Client::new();

    // **The client leg, refused** (ADR 0028, issue #620). Before the
    // forwarded route could carry a `price`, this same request was carried
    // across the peering for nothing -- and the payer signed a real peer
    // claim for the value it carried. It is now answered with the route's
    // terms, quoting the price the config wrote, exactly as a terminated
    // route's would be. Asserted *first*, so a regression that made the
    // route free again could not be masked by the paid crossings below
    // happening to work.
    let terms = claimless_client_terms(
        &client,
        &payer.client_edge_addr,
        &peer_bound_prepare(APP_PREFIX, b"unpaid", &payee_identity),
    )
    .await;
    assert_eq!(
        terms["accepts"][0]["amount"],
        CLIENT_PRICE.to_string(),
        "the greeting must quote the forwarded route's own `price`"
    );
    assert_eq!(
        terms["accepts"][0]["extra"]["price"],
        CLIENT_PRICE.to_string(),
        "§1.4: the top-level `amount` and `extra.price` are always equal"
    );
    assert!(
        client_journal(payer_state.path()).is_empty(),
        "a greeting changes no state: nothing may have been journaled for a \
         request that was never paid for. Journal was:\n{}",
        client_journal(payer_state.path())
    );

    // **The client leg, paid.** Every crossing below carries a real,
    // EIP-712-signed client claim on the client channel, advancing by
    // exactly `CLIENT_PRICE` each time -- what a paying client actually
    // sends, and the only thing that now gets a packet across this route.
    let cross = |body: &'static [u8], nonce: u64| {
        let prepare = peer_bound_prepare(APP_PREFIX, body, &payee_identity);
        let claim = fixture.client_claim(nonce);
        let client = client.clone();
        let addr = payer.client_edge_addr.clone();
        async move {
            let (status, body, _ack) =
                post_peer_request(&client, &addr, Some(&claim), &prepare).await;
            assert_eq!(status, reqwest::StatusCode::OK);
            connector_domain::Fulfill::decode(&body).unwrap_or_else(|_| {
                let reject =
                    Reject::decode(&body).expect("an answer that is neither FULFILL nor REJECT");
                panic!(
                    "the paid packet did not cross the peering: {} {}",
                    reject.code.as_str(),
                    reject.message
                )
            });
        }
    };

    cross(b"across the peering", 1).await;

    // **Twice, and no longer for ADR 0004's reason** (issue #1145). It used
    // to be that the payer owed nothing until the first crossing had
    // fulfilled, so the claim covering crossing *n* was signed after it and
    // rode crossing *n + 1* -- one packet could prove delivery and say
    // nothing about payment. That model is deleted: crossing 1 arrives
    // already covered.
    //
    // The second crossing still earns its place, for `local/two-hop`'s
    // reason instead (issue #1102). A covering payer asks the payee where
    // its claims stand on every packet, and a payee answering out of the
    // wrong book reports nonce 0 forever -- so crossing 2 re-signs crossing
    // 1's cumulative amount at a fresh nonce and advances nothing, which is
    // accepted every time and buys nothing. One crossing cannot see that.
    cross(b"across the peering again", 2).await;

    // Delivery alone is what the deleted test settled for. This asserts the
    // money on the **peer** leg: the payee's peer claim ledger records a
    // claim on this peering's channel, so the crossing was charged and the
    // claim was accepted rather than merely sent.
    assert!(
        peer_journal(payee_state.path()).contains(&fixture.channel_id),
        "§3.2: the sender owes for the crossing, so the payee's peer claim \
         ledger must record a claim on this peering's channel. Ledger was:\n{}",
        peer_journal(payee_state.path())
    );

    // And on the **client** leg (ADR 0028): the payer's own client-edge
    // journal records the client's claims on the client channel. Without
    // this the test above still passes while the payer pays the payee out
    // of its own pocket -- which is exactly the state issue #620 found and
    // the reason the devnet's store leg was never repointed at a peering.
    let charged = client_journal(payer_state.path());
    assert!(
        charged.contains(&fixture.client_channel_id),
        "the client leg must be charged: the payer's client-edge claim \
         journal must record a claim on the client's channel. Journal was:\n{charged}"
    );
}

/// **The amount a priced forwarded route will carry is bounded by its
/// price** (ADR 0028). A client that pays `CLIENT_PRICE` and declares a
/// larger `amount` is asking this connector to forward `amount - PEER_FEE`
/// downstream -- and to sign a peer claim for it -- having paid for
/// `CLIENT_PRICE`. That difference is this connector's money, chosen by
/// whoever sends the packet, which is why it is refused rather than
/// carried.
///
/// `F03` (Invalid Amount), the same code an underpaying claim gets: the
/// packet is fine and the amount is not. Refused *before* the claim is
/// ingested, so the client's watermark is not spent on a packet that never
/// moves.
///
/// One carriage is enough here, unlike every other test in this file: the
/// rule lives in `over_carried_reject`, which both carriages call from the
/// same place, and §9's no-drift concern is about behaviour that exists on
/// one carriage and not the other rather than about which one a shared
/// function is proven through.
#[tokio::test]
async fn a_client_may_not_declare_more_than_the_forwarded_route_charges() {
    let Some(fixture) = PeerFixture::spawn().await else {
        return;
    };
    let payee_state = tempfile::tempdir().expect("temp payee state dir");
    let payer_state = tempfile::tempdir().expect("temp payer state dir");
    let stub_app = spawn_stub_app();
    let (payee, _payee_config, _payee_key) =
        spawn_payee(&fixture, payee_state.path(), &stub_app.addr);
    let endpoint = format!("http://{}/ilp", payee.client_edge_addr);
    let (payer, _payer_config, _payer_key) = spawn_payer(
        &fixture,
        payer_state.path(),
        Carriage::Http,
        &endpoint,
        &endpoint,
    );

    let payee_identity = identity_from_key_seed(PAYEE_SIGNER_SEED);
    let (data, _shared_secret) = sealed_prepare_data(b"over-carried", &payee_identity);
    let over_carried = Prepare {
        amount: CLIENT_PRICE + 1,
        ..sample_prepare(APP_PREFIX, data)
    };

    let response = reqwest::Client::new()
        .post(format!("http://{}/ilp", payer.client_edge_addr))
        .header(
            "ilp-payment-channel-claim",
            BASE64.encode(fixture.client_claim(1)),
        )
        .body(over_carried.encode())
        .send()
        .await
        .expect("POST /ilp");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    // `accumulatedCost` is a carriage-layer field, not a packet one
    // (`peer-carriage-spec.md` §3): it rides the `Toon-Accumulated-Cost`
    // response header, so it is read here rather than off the decoded
    // REJECT, whose own field is only this process's in-memory carrier.
    let accumulated_cost = response
        .headers()
        .get("toon-accumulated-cost")
        .expect("every REJECT this edge answers carries its running cost")
        .to_str()
        .expect("the accumulated-cost header is ASCII")
        .to_string();
    let body = response.bytes().await.expect("response body").to_vec();
    let reject = Reject::decode(&body).expect("an over-carried packet is refused, not fulfilled");
    assert_eq!(
        reject.code.as_str(),
        "F03",
        "an amount larger than the price is an amount problem, not a packet one: {}",
        reject.message
    );
    assert_eq!(
        accumulated_cost,
        CLIENT_PRICE.to_string(),
        "issue #548: the figure the sender must fit under rides home as a \
         number, not only as English"
    );
    assert!(
        client_journal(payer_state.path()).is_empty(),
        "the claim must not be spent on a packet this connector refused to \
         carry. Journal was:\n{}",
        client_journal(payer_state.path())
    );
}

/// **Assertions 2 and 3, over `wss://`: a claim is acknowledged, and a
/// byte-identical retransmission is acknowledged again.**
///
/// The test drives the payee's real binary as a real peer over a real
/// socket, rather than through the payer -- because §6.3's retransmission
/// rule is a property of what the *payee* answers, and a payer that never
/// loses an ack never retransmits. One lost ack otherwise wedges a peering
/// permanently, so this is the assertion whose absence is expensive.
///
/// Three answers are asserted, in order:
///
/// 1. the first claim is `{"result":"accepted"}` and the peer ledger records
///    it (§6.1, §6.2 -- the ack rides the response that already answers the
///    claim-bearing frame, and the status/packet verdict is independent);
/// 2. the **byte-identical** claim, replayed at the current watermark, is
///    `accepted` again and **not** `nonce_not_advancing` (§6.3);
/// 3. a claim at the same nonce differing in any other field *is* refused
///    `nonce_not_advancing` -- the narrowing in (2) is exactly one claim
///    wide, and a payee that answered `accepted` here would be accepting a
///    second, different claim for the same money.
#[tokio::test]
async fn a_peer_claim_is_acknowledged_over_btp() {
    a_peer_claim_is_acknowledged(Carriage::Btp).await;
}

/// **Assertions 2 and 3, over `https://`.** Identical semantics on the
/// other carriage (§9), with the ack riding the `Toon-Claim-Ack` response
/// header instead of the `claim-ack` protocolData entry, and the status
/// `200` regardless of the claim verdict (§6.2).
#[tokio::test]
async fn a_peer_claim_is_acknowledged_over_http() {
    a_peer_claim_is_acknowledged(Carriage::Http).await;
}

async fn a_peer_claim_is_acknowledged(carriage: Carriage) {
    let Some(fixture) = PeerFixture::spawn().await else {
        return;
    };
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let stub_app = spawn_stub_app();
    let (payee, _config, _key) = spawn_payee(&fixture, state_dir.path(), &stub_app.addr);
    let payee_identity = identity_from_key_seed(PAYEE_SIGNER_SEED);
    let client = reqwest::Client::new();

    // §6.3 is about *bytes*: the claim JSON carries a `timestamp`, so the
    // retransmission below reuses this exact string rather than
    // re-rendering it. Re-rendering would produce a different claim at the
    // same nonce, which §6.3 requires a payee to refuse.
    let claim = fixture.claim(1);

    let ack_of = |body: &'static [u8], claim: &str| {
        let (data, _shared) = sealed_prepare_data(body, &payee_identity);
        let prepare = sample_prepare(APP_PREFIX, data);
        let claim = claim.to_string();
        let addr = payee.client_edge_addr.clone();
        let client = client.clone();
        async move {
            match carriage {
                Carriage::Btp => send_peer_message(&addr, Some(&claim), &prepare).await.1,
                Carriage::Http => {
                    let (status, _body, ack) =
                        post_peer_request(&client, &addr, Some(&claim), &prepare).await;
                    assert_eq!(
                        status,
                        reqwest::StatusCode::OK,
                        "§6.2: the status is 200 regardless of the claim verdict"
                    );
                    ack.map(|value| {
                        String::from_utf8(BASE64.decode(value).expect("ack header is base64"))
                            .expect("ack JSON is UTF-8")
                    })
                }
            }
        }
    };

    // (1) The claim is acknowledged, and the peer ledger records it.
    let first = ack_of(b"first crossing", &claim).await;
    assert_eq!(
        first.as_deref(),
        Some(r#"{"result":"accepted"}"#),
        "§6.1: the ack rides the response that already answers the claim-bearing frame"
    );
    assert!(
        peer_journal(state_dir.path()).contains(&fixture.channel_id),
        "§1.7: a peer claim advances a peer watermark and is appended to the peer claim ledger"
    );

    // (2) The idempotent re-ack: the same bytes, at the current watermark.
    let replayed = ack_of(b"retransmission", &claim).await;
    assert_eq!(
        replayed.as_deref(),
        Some(r#"{"result":"accepted"}"#),
        "§6.3: a claim byte-identical to the one already at the watermark MUST be \
         answered `accepted`, never `nonce_not_advancing` -- a lost ack and a lost \
         claim are indistinguishable at the payer, and refusing the retransmission \
         wedges the peering permanently"
    );

    // (3) ...and the narrowing is exactly one claim wide.
    let different_at_the_same_nonce = evm_claim_json(
        &fixture.payer_secret,
        PAYER_ID,
        &fixture.channel_id,
        1,
        u128::from(PEER_FEE) + 1,
        fixture.token_network_address,
    );
    let refused = ack_of(
        b"a different claim at nonce 1",
        &different_at_the_same_nonce,
    )
    .await;
    assert_eq!(
        refused.as_deref(),
        Some(r#"{"result":"rejected","reason":"nonce_not_advancing"}"#),
        "§6.3: a claim at the same nonce differing in any other field is a \
         *different* claim and MUST be refused"
    );
}
