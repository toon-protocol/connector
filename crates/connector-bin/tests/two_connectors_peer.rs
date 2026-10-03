//! Issue #734, as ADR 0075 (decisions 4, 5, 6 and 9) and issue #1380 remake
//! it: **two real connectors, peered by config, moving a paid packet over
//! x402 channels** -- over both carriages, against two spawned `connector`
//! binaries and one disposable `anvil`.
//!
//! # What a config-declared peering is now
//!
//! A peering is **two one-way x402 `x402BatchSettlement` channels**, one
//! opened by each side, and never a TOON `TokenNetwork` channel. A config
//! file names each half:
//!
//! * `[[peer_channels]]` -- the **inbound** half: `peer_id` and the peer's
//!   `voucher_signer` (its EVM settlement address). A voucher on a channel
//!   whose `payerAuthorizer` is that signer is what makes an interaction the
//!   peer's (decision 5). The channel itself is admitted by the claim gate
//!   exactly as a client's is; nobody writes its id down.
//! * `[[pay_channels]]` -- the **outbound** half: this node's own x402
//!   channel toward the hop (`outbound_channel`), which must already be in
//!   its `<state_dir>/outbound-channels.log`, and the hop's `POST /ilp`
//!   (`client_edge_url`), asked where the channel stands on restore
//!   (decision 6). Every PREPARE forwarded to the hop is covered by a
//!   voucher on it.
//!
//! [`PeerFixture::spawn`] opens the payer's channel the way an operator's
//! earlier `POST /channels` under the same `state_dir` would: through
//! `OutboundChannels` over a `FileJournal` at the payer's
//! `outbound-channels.log`, dropped before the payer binary is spawned
//! naming that channel.
//!
//! # What each test holds
//!
//! | # | Claim | Test |
//! | - | ----- | ---- |
//! | 1 | a packet crosses the peering and is fulfilled, and the payee's client-edge journal records the payer's voucher advanced by exactly what was forwarded (`CLIENT_PRICE - PEER_FEE`) -- the client leg paid by a real client voucher on the payer's edge | [`two_connectors_move_a_paid_packet_over_btp`] / [`_over_http`](two_connectors_move_a_paid_packet_over_http) |
//! | 2 | a peer voucher is acked `accepted`, a byte-identical resend is re-acked `accepted` (§6.3), and a voucher below the watermark is refused `amount_not_advancing` | [`a_peer_voucher_is_acknowledged_over_btp`] / [`_over_http`](a_peer_voucher_is_acknowledged_over_http) |
//! | 3 | §1.9's named regression: no evidence, a stranger's voucher on the payer's channel, an unbound signer's voucher on its own channel, a retired `toon-channel` claim (refused by name, #1384), and a garbage header -- each gets no claim-ack and journals nothing on the peering's channel; the one genuine payment to the payee (the unbound voucher) is journaled as a client's and nothing else is | [`a_claim_that_does_not_prove_the_peer_role_reaches_no_peer_handling_over_http`] / [`_over_btp`](a_claim_that_does_not_prove_the_peer_role_reaches_no_peer_handling_over_btp) |
//! | 4 | a peering with no `[[peer_channels]]` row, and a `[[pay_channels]]` row naming a channel this node's journal does not hold, each refuse to start by name | [`a_peer_with_no_channel_binding_refuses_to_start`], [`a_pay_channel_this_node_never_opened_refuses_to_start`] |
//! | 5 | a priced forwarded route carries no more than its price (ADR 0028) | [`a_client_may_not_declare_more_than_the_forwarded_route_charges`] |
//!
//! # The client leg
//!
//! A client of the payer pays on an x402 channel of its own toward the payer
//! (ADR 0075: every claim is a voucher, #1384), opened the way a stock x402
//! client opens one.
//!
//! # EVM only
//!
//! A Solana config peering's vouchers are held at the library level
//! (`connector-cli/tests/solana_peering_from_a_url.rs`); everything below is
//! parameterised by carriage, never by chain.

use std::io::Write as _;
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use ethers::signers::{LocalWallet, Signer as _};
use ethers::types::Address;
use futures_util::{SinkExt as _, StreamExt as _};

use connector_btp::{
    decode_frame, encode_message, ProtocolData, BTP_RESPONSE, CLAIM_ACK_PROTOCOL, CLAIM_PROTOCOL,
    CONTENT_TYPE_TEXT,
};
use connector_domain::{Prepare, Reject};
use connector_runtime::{FileJournal, InMemoryJournal, Journal, OutboundChannels, SettlementChain};
use connector_settlement::batch::{
    BatchSettlementPayer, ChannelPresentation, EvmReceiverTerms, ReceiverTerms, Voucher,
};
use connector_settlement::ChannelId;
use connector_settlement_evm::test_support::x402::X402Chain;
use connector_settlement_evm::test_support::{
    require_anvil, Anvil, COUNTERPARTY_PRIVATE_KEY, DEPLOYER_PRIVATE_KEY,
};
use connector_signer::PublicKeyBytes;

mod support;
use support::{
    identity_from_key_seed, sample_prepare, sealed_prepare_data, spawn_connector, spawn_stub_app,
    write_config, write_raw_key_file, ConnectorProcess,
};

/// This test binary's own base port for [`Anvil::spawn`], distinct from
/// every other test binary's `ANVIL_BASE_PORT` in this workspace.
const ANVIL_BASE_PORT: u16 = 23_900;

/// The peering's fee: what the payer retains of each packet it forwards.
const PEER_FEE: u64 = 100;

/// The **client-facing** price of the payer's forwarded route (ADR 0028),
/// and so the amount a client's packet declares. The payer forwards
/// `CLIENT_PRICE - PEER_FEE` = [`FORWARDED`] and covers exactly that with a
/// voucher; a larger declared amount is refused -- see
/// [`a_client_may_not_declare_more_than_the_forwarded_route_charges`].
const CLIENT_PRICE: u64 = 10 * PEER_FEE;

/// What one crossing of the peering moves, and so what each covering
/// voucher advances the payee's watermark by.
const FORWARDED: u64 = CLIENT_PRICE - PEER_FEE;

/// The payer's opening deposit on its outbound x402 channel toward the
/// payee.
const PEER_DEPOSIT: u128 = 100 * CLIENT_PRICE as u128;

/// The seconds a channel this suite opens may be withdrawn after: the
/// `[settlement.evm]` default a payee requires at least.
const WITHDRAW_DELAY_SECS: u64 = 86_400;

/// The fixed `timestamp` every in-test voucher is rendered with, so a
/// resend is byte-identical (§6.3).
const VOUCHER_TIMESTAMP: &str = "2026-09-28T00:00:00.000Z";

/// The client's own key -- a third party to the peering, with its own x402
/// channel to the payer.
const CLIENT_SECRET_SEED: u8 = 23;

/// A key that is nobody's voucher signer: forges a voucher on the payer's
/// channel.
const STRANGER_SECRET_SEED: u8 = 0x5a;

/// A key with a genuine x402 channel to the payee of its own that no
/// `[[peer_channels]]` row binds.
const UNBOUND_SIGNER_SEED: u8 = 0x77;

/// **One id, written by both operators**: `[[peers]].id` names the peering
/// relation, and both files must spell it the same.
const PEERING_ID: &str = "alpha-beta";
const PAYEE_ID: &str = PEERING_ID;
const PAYER_ID: &str = PEERING_ID;

/// `[signer] key_file` seeds, so a test can seal a packet to a spawned
/// binary's identity without asking it.
const PAYER_SIGNER_SEED: u8 = 71;
const PAYEE_SIGNER_SEED: u8 = 72;

/// The payer settles as anvil's first account, the payee as its second.
const PAYER_SETTLEMENT_KEY: &str = DEPLOYER_PRIVATE_KEY;
const PAYEE_SETTLEMENT_KEY: &str = COUNTERPARTY_PRIVATE_KEY;

/// The prefix the payer routes to the payee, and the one the payee
/// terminates: a packet addressed to the app has exactly one path, through
/// the peering.
const PEER_ROUTE_PREFIX: &str = "g.example.beta";
const APP_PREFIX: &str = "g.example.beta.app";

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn key_bytes(key: &str) -> [u8; 32] {
    hex::decode(key.trim_start_matches("0x"))
        .expect("a hex key")
        .try_into()
        .expect("32 bytes")
}

fn wallet_of(key: &[u8; 32]) -> LocalWallet {
    LocalWallet::from_bytes(key).expect("a valid secp256k1 key")
}

fn spelled(address: Address) -> String {
    format!("{address:#x}")
}

/// Which carriage a parameterised test runs over. §9 makes a peer behaviour
/// on one carriage and not the other a defect, so every assertion here is
/// written once and run twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Carriage {
    Btp,
    Http,
}

impl Carriage {
    /// The `[[peers]] endpoint` scheme selecting this carriage. Plaintext,
    /// because this harness stands up no TLS terminator:
    /// `peer_allow_plaintext_endpoints` on the payer lets `ws://`/`http://`
    /// resolve onto the carriages `wss://`/`https://` do.
    fn scheme(self) -> &'static str {
        match self {
            Carriage::Btp => "ws",
            Carriage::Http => "http",
        }
    }

    /// The `peer_expose` value that opens a listener for this carriage.
    fn expose(self) -> &'static str {
        match self {
            Carriage::Btp => "btp",
            Carriage::Http => "http",
        }
    }
}

/// One `anvil` holding x402's `x402BatchSettlement` at its canonical
/// address and a Circle FiatToken as USDC.
struct Chain {
    anvil: Anvil,
    x402: X402Chain,
    token: Address,
}

impl Chain {
    /// `None` only off CI with no `anvil`: [`require_anvil`] panics when
    /// `CI` is set, so this can never go green in the gate by skipping.
    async fn spawn() -> Option<Chain> {
        if !require_anvil() {
            return None;
        }
        let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
        let mut x402 = X402Chain::place(&anvil.rpc_url).await;
        let token = x402.deploy_fiat_token().await;
        Some(Chain { anvil, x402, token })
    }

    /// `[settlement.evm]`, keyed, with its x402 batch-settlement table, for
    /// whichever side `key_file` belongs to.
    fn settlement_block(&self, key_file: &std::path::Path) -> String {
        format!(
            r#"
[settlement.evm]
rpc_url = "{rpc_url}"
token_address = "{token:?}"
decimals = 6
asset_eip712_name = "USDC"
asset_eip712_version = "2"

[settlement.evm.key]
key_file = "{key_file}"

"#,
            rpc_url = self.anvil.rpc_url,
            token = self.token,
            key_file = key_file.display(),
        )
    }

    /// Open an x402 channel paying `receiver`, as `payer_key`, funded with
    /// `deposit`, recording it in `journal` -- exactly what a node's
    /// `POST /channels` does under its `state_dir`. Everything is dropped
    /// before this returns, so a binary may then restore the same journal.
    async fn open_x402_channel(
        &self,
        payer_key: &str,
        journal: Arc<dyn Journal>,
        receiver: Address,
        deposit: u128,
    ) -> (String, ChannelPresentation) {
        let payer = connector_settlement_evm::EvmBatchSettlementBackend::connect(
            &connector_settlement_evm::RpcTransport::direct(&self.anvil.rpc_url)
                .expect("rpc transport"),
            payer_key,
            self.token,
            6,
            WITHDRAW_DELAY_SECS,
        )
        .await
        .expect("the payer's x402 half");
        let outbound = OutboundChannels::restore(
            journal,
            vec![(
                SettlementChain::Evm,
                Arc::new(payer) as Arc<dyn BatchSettlementPayer>,
            )],
        )
        .await
        .expect("the journal replays");
        let (opened, _) = outbound
            .open(
                ReceiverTerms::Evm(EvmReceiverTerms {
                    receiver: receiver.to_fixed_bytes(),
                    token: self.token.to_fixed_bytes(),
                    min_withdraw_delay_secs: WITHDRAW_DELAY_SECS,
                }),
                deposit,
            )
            .await
            .expect("open an x402 channel toward the payee");
        let id = opened.on_chain.id.0.clone();
        let presentation = outbound.presentation(&id).expect("a journaled channel");
        assert_eq!(
            self.x402.channel(&ChannelId(id.clone())).await,
            (deposit, 0),
            "the channel holds a real deposit, read off the chain"
        );
        (id, presentation)
    }
}

/// Everything below the carriage, shared by every live test: the chain, the
/// payer's own outbound x402 channel toward the payee (journaled under the
/// payer's `state_dir`), an unbound signer's channel toward the payee, and
/// the client leg's x402 channel toward the payer.
struct PeerFixture {
    chain: Chain,
    payer_address: Address,
    payee_address: Address,
    /// The payer's `state_dir`: its `outbound-channels.log` already holds
    /// [`Self::payer_channel`].
    payer_state: tempfile::TempDir,
    /// The payer's own x402 channel toward the payee: the one its
    /// `[[pay_channels]]` row names, and the one the payee admits as the
    /// peer's because its `payerAuthorizer` is the bound voucher signer.
    payer_channel: String,
    payer_presentation: ChannelPresentation,
    /// A genuine x402 channel toward the payee, opened and signed for by a
    /// key no `[[peer_channels]]` row binds: an ordinary client of the
    /// payee.
    unbound_channel: String,
    unbound_presentation: ChannelPresentation,
    unbound_key: [u8; 32],
    /// The client leg (ADR 0028): the client's own x402 channel toward the
    /// payer, which the payer admits as a client's.
    client_key: [u8; 32],
    client_channel: String,
    client_presentation: ChannelPresentation,
}

impl PeerFixture {
    async fn spawn() -> Option<PeerFixture> {
        let chain = Chain::spawn().await?;
        let payer_address = wallet_of(&key_bytes(PAYER_SETTLEMENT_KEY)).address();
        let payee_address = wallet_of(&key_bytes(PAYEE_SETTLEMENT_KEY)).address();
        let unbound_key = [UNBOUND_SIGNER_SEED; 32];
        let unbound_address = wallet_of(&unbound_key).address();
        chain
            .x402
            .mint(chain.token, payer_address, 10_000_000)
            .await;
        chain
            .x402
            .mint(chain.token, unbound_address, 1_000_000)
            .await;
        chain.x402.fund_gas(unbound_address).await;

        let client_key = [CLIENT_SECRET_SEED; 32];
        let client_address = wallet_of(&client_key).address();
        chain
            .x402
            .mint(chain.token, client_address, 1_000_000)
            .await;
        chain.x402.fund_gas(client_address).await;

        // The payer's own x402 channel toward the payee, journaled exactly
        // where the payer binary will restore it from.
        let payer_state = tempfile::tempdir().expect("temp payer state dir");
        let (payer_channel, payer_presentation) = chain
            .open_x402_channel(
                PAYER_SETTLEMENT_KEY,
                Arc::new(
                    FileJournal::open(payer_state.path().join("outbound-channels.log"))
                        .expect("open the payer's outbound-channel journal"),
                ),
                payee_address,
                PEER_DEPOSIT,
            )
            .await;

        // A client of the payee with a real channel of its own, signed for
        // by a key nobody bound.
        let (unbound_channel, unbound_presentation) = chain
            .open_x402_channel(
                &hex_encode(&unbound_key),
                Arc::new(InMemoryJournal::new()),
                payee_address,
                10 * u128::from(CLIENT_PRICE),
            )
            .await;

        // The client leg: the client's own x402 channel toward the payer.
        let (client_channel, client_presentation) = chain
            .open_x402_channel(
                &hex_encode(&client_key),
                Arc::new(InMemoryJournal::new()),
                payer_address,
                100 * u128::from(CLIENT_PRICE),
            )
            .await;

        Some(PeerFixture {
            chain,
            payer_address,
            payee_address,
            payer_state,
            payer_channel,
            payer_presentation,
            unbound_channel,
            unbound_presentation,
            unbound_key,
            client_key,
            client_channel,
            client_presentation,
        })
    }

    /// A voucher for `cumulative` on the channel `presentation` names,
    /// signed by `key` over the channel's EIP-712 voucher digest, rendered
    /// as the `batch-settlement` claim JSON a payer sends.
    fn voucher(
        &self,
        presentation: &ChannelPresentation,
        key: &[u8; 32],
        cumulative: u128,
    ) -> String {
        let wallet = wallet_of(key);
        let signature = self
            .chain
            .x402
            .sign_voucher(&wallet, presentation.channel(), cumulative);
        connector_runtime::voucher_json(
            presentation,
            &Voucher {
                cumulative_amount: cumulative,
                signature,
            },
            &spelled(wallet.address()),
            VOUCHER_TIMESTAMP,
        )
    }

    /// The payer's own voucher on its channel toward the payee.
    fn payer_voucher(&self, cumulative: u128) -> String {
        self.voucher(
            &self.payer_presentation,
            &key_bytes(PAYER_SETTLEMENT_KEY),
            cumulative,
        )
    }

    /// The client's voucher for its `n`th packet across the payer's
    /// forwarded route: cumulative `n * CLIENT_PRICE`.
    fn client_claim(&self, n: u64) -> String {
        self.voucher(
            &self.client_presentation,
            &self.client_key,
            u128::from(n) * u128::from(CLIENT_PRICE),
        )
    }

    /// §1.9's cases a wire can present -- shared so the two carriages
    /// cannot drift in which cases they cover. The positive control is
    /// [`a_peer_voucher_is_acknowledged`], which presents the payer's own
    /// voucher to the same binary and is acked.
    ///
    /// One of them is a **genuine payment to the payee**, and a payee is
    /// right to take it -- as a client's: the unbound signer's voucher on its
    /// own x402 channel. It names the channel it pays on and the cumulative
    /// amount it is journaled at, and that is the only thing it may leave
    /// behind: no ack, and nothing on the peering's channel. The retired
    /// `toon-channel` claim is refused by name (#1384) and journals nothing.
    fn refused_claims(&self) -> Vec<RefusedCase> {
        vec![
            RefusedCase {
                name: "no claim at all",
                claim: None,
                pays_as_client: None,
            },
            RefusedCase {
                name: "a voucher on the payer's channel signed by a stranger key",
                claim: Some(self.voucher(
                    &self.payer_presentation,
                    &[STRANGER_SECRET_SEED; 32],
                    u128::from(FORWARDED),
                )),
                pays_as_client: None,
            },
            RefusedCase {
                name: "a voucher on an unbound signer's own channel",
                claim: Some(self.voucher(
                    &self.unbound_presentation,
                    &self.unbound_key,
                    u128::from(FORWARDED),
                )),
                pays_as_client: Some((self.unbound_channel.clone(), FORWARDED)),
            },
            RefusedCase {
                name: "a retired toon-channel claim",
                claim: Some(toon_channel_claim()),
                pays_as_client: None,
            },
            RefusedCase {
                name: "a claim header that is not a claim",
                claim: Some("not a claim at all".to_string()),
                pays_as_client: None,
            },
        ]
    }
}

/// A retired `toon-channel` claim, as a pre-ADR 0075 peer rendered one: no
/// `scheme`, a nonce and a balance proof. Refused by name before anything
/// about it is read (#1384), so its signature does not matter.
fn toon_channel_claim() -> String {
    serde_json::json!({
        "version": "1.0",
        "blockchain": "evm",
        "messageId": "msg-1",
        "timestamp": "2026-02-02T12:00:00.000Z",
        "senderId": PAYER_ID,
        "channelId": format!("0x{}", "07".repeat(32)),
        "nonce": 1,
        "transferredAmount": PEER_FEE.to_string(),
        "lockedAmount": "0",
        "locksRoot": format!("0x{}", "00".repeat(32)),
        "signature": format!("0x{}", "11".repeat(65)),
        "signerAddress": format!("0x{}", "44".repeat(20)),
    })
    .to_string()
}

/// One §1.9 case: what rides the request, and -- when it is a genuine
/// payment to the payee on a channel of its own -- the channel and
/// cumulative amount the client edge journals it at.
struct RefusedCase {
    name: &'static str,
    claim: Option<String>,
    pays_as_client: Option<(String, u64)>,
}

fn settlement_key_file(key: &str) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("temp settlement key file");
    file.write_all(key.as_bytes())
        .expect("write settlement key file");
    file
}

/// The payee: a real `connector` binary terminating [`APP_PREFIX`] at a
/// real stub app, exposing both peer carriages, and binding the payer's
/// settlement address as the peering's voucher signer.
fn spawn_payee(
    fixture: &PeerFixture,
    state_dir: &std::path::Path,
    stub_app_addr: &str,
) -> (
    ConnectorProcess,
    tempfile::NamedTempFile,
    tempfile::NamedTempFile,
    tempfile::NamedTempFile,
) {
    spawn_payee_priced(fixture, state_dir, stub_app_addr, 0)
}

/// [`spawn_payee`] with the terminated route priced at `price`.
fn spawn_payee_priced(
    fixture: &PeerFixture,
    state_dir: &std::path::Path,
    stub_app_addr: &str,
    price: u64,
) -> (
    ConnectorProcess,
    tempfile::NamedTempFile,
    tempfile::NamedTempFile,
    tempfile::NamedTempFile,
) {
    let key_file = write_raw_key_file(PAYEE_SIGNER_SEED);
    let settlement_key = settlement_key_file(PAYEE_SETTLEMENT_KEY);
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
price = {price}

# The peering this node accepts. No `endpoint`: the payer dials us.
[[peers]]
id = "{PAYER_ID}"

# The inbound half (ADR 0075 decision 5): a voucher on a channel whose
# `payerAuthorizer` is this key is the payer's, and nothing else is.
[[peer_channels]]
peer_id = "{PAYER_ID}"
voucher_signer = "{payer}"
"#,
        state_dir = state_dir.display(),
        key_file = key_file.path().display(),
        settlement = fixture.chain.settlement_block(settlement_key.path()),
        payer = spelled(fixture.payer_address),
    ));
    let connector = spawn_connector(config.path());
    (connector, config, key_file, settlement_key)
}

/// The payer: a real `connector` binary whose only route to [`APP_PREFIX`]
/// is the peering, dialed at `payee_endpoint` over `carriage`, priced at
/// [`CLIENT_PRICE`] for its own clients, and covering every forward with a
/// voucher on the x402 channel [`PeerFixture::spawn`] journaled under its
/// `state_dir`.
fn spawn_payer(
    fixture: &PeerFixture,
    carriage: Carriage,
    payee_endpoint: &str,
    payee_client_edge: &str,
) -> (
    ConnectorProcess,
    tempfile::NamedTempFile,
    tempfile::NamedTempFile,
    tempfile::NamedTempFile,
) {
    let key_file = write_raw_key_file(PAYER_SIGNER_SEED);
    let settlement_key = settlement_key_file(PAYER_SETTLEMENT_KEY);
    let config = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"
peer_expose = "{expose}"
peer_allow_plaintext_endpoints = true

[signer]
key_file = "{key_file}"
{settlement}
# The only path to the app. `price` is what this node's own client edge
# charges (ADR 0028); the peering's `fee` is what it keeps of it.
[[routes]]
prefix = "{PEER_ROUTE_PREFIX}"
peer_id = "{PAYEE_ID}"
price = {CLIENT_PRICE}

[[peers]]
id = "{PAYEE_ID}"
endpoint = "{payee_endpoint}"
fee = {PEER_FEE}

# The inbound half: the payee's vouchers, were it ever to pay us.
[[peer_channels]]
peer_id = "{PAYEE_ID}"
voucher_signer = "{payee}"

# The outbound half (ADR 0075 decisions 4 and 6): this node's own x402
# channel toward the payee, already in its outbound-channel journal, and
# the payee's POST /ilp, asked where the channel stands on restore. HTTP
# even when the peering is carried over BTP: the claim-state ask is its own
# request.
[[pay_channels]]
peer_id = "{PAYEE_ID}"
outbound_channel = "{channel}"
client_edge_url = "{payee_client_edge}"
"#,
        state_dir = fixture.payer_state.path().display(),
        key_file = key_file.path().display(),
        expose = carriage.expose(),
        settlement = fixture.chain.settlement_block(settlement_key.path()),
        payee = spelled(fixture.payee_address),
        channel = fixture.payer_channel,
    ));
    let connector = spawn_connector(config.path());
    (connector, config, key_file, settlement_key)
}

/// A node's client-edge claim journal under its `state_dir`. Since ADR 0075
/// a peer's vouchers are journaled here beside a client's -- a voucher
/// channel keeps one watermark whichever role its vouchers arrive under --
/// and the old `peer-claims.log` is no longer written.
fn client_edge_journal(state_dir: &std::path::Path) -> String {
    std::fs::read_to_string(state_dir.join("client-edge-claims.log")).unwrap_or_default()
}

/// Every cumulative amount a node's client-edge journal accepted on x402
/// channel `channel`, in order.
fn journaled(state_dir: &std::path::Path, channel: &str) -> Vec<u64> {
    let key = format!("evm:{channel}");
    client_edge_journal(state_dir)
        .lines()
        .filter(|line| line.starts_with("inbound_claim_accepted\t"))
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            (fields[1] == key).then(|| fields[3].parse().expect("an amount"))
        })
        .collect()
}

/// A packet addressed across the peering, carrying [`CLIENT_PRICE`]: a
/// zero-amount packet cannot survive the hop's own fee (R01).
fn peer_bound_prepare(destination: &str, body: &'static [u8], payee: &PublicKeyBytes) -> Prepare {
    let (data, _shared_secret) = sealed_prepare_data(body, payee);
    Prepare {
        amount: CLIENT_PRICE,
        ..sample_prepare(destination, data)
    }
}

/// Present `claim` to a running connector's HTTP carriage and return the
/// response's status, body and decoded `Toon-Claim-Ack`.
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
    let ack = response.headers().get("toon-claim-ack").map(|value| {
        String::from_utf8(
            BASE64
                .decode(value.as_bytes())
                .expect("ack header is base64"),
        )
        .expect("ack JSON is UTF-8")
    });
    let body = response.bytes().await.expect("response body").to_vec();
    (status, body, ack)
}

/// The BTP twin of [`post_peer_request`]: one websocket session to
/// `/ilp/btp` carrying one MESSAGE -- the claim entry and the PREPARE
/// together -- returning its packet and its `claim-ack` entry, if any.
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
        "§6.2: a claim verdict is never a BTP ERROR"
    );
    let ack = decoded
        .protocol_data
        .iter()
        .find(|entry| entry.name == CLAIM_ACK_PROTOCOL)
        .map(|entry| String::from_utf8(entry.data.clone()).expect("ack entry is UTF-8"));
    (decoded.ilp_packet, ack)
}

/// The next binary frame off a websocket.
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

/// Present `claim` riding `prepare` to `addr` over `carriage`, returning the
/// ack, if one rode back.
async fn present(
    carriage: Carriage,
    client: &reqwest::Client,
    addr: &str,
    claim: Option<&str>,
    prepare: &Prepare,
) -> Option<String> {
    match carriage {
        Carriage::Btp => send_peer_message(addr, claim, prepare).await.1,
        Carriage::Http => {
            let (status, _body, ack) = post_peer_request(client, addr, claim, prepare).await;
            assert!(
                status.is_success() || status == reqwest::StatusCode::BAD_REQUEST,
                "§6.2 reserves non-200 for a malformed request, never a claim verdict: {status}"
            );
            ack
        }
    }
}

// ---------------------------------------------------------------------------
// §1.9, on a live binary, over both carriages.
// ---------------------------------------------------------------------------

/// **The named regression, over HTTP** (`peer-carriage-spec.md` §1.9, as ADR
/// 0075 decision 5 amends ADR 0060): only a voucher on a channel whose
/// `payerAuthorizer` is a bound voucher signer proves the peer role. Each
/// case below is a client interaction -- a retired `toon-channel` claim is
/// refused by name by the client edge a shared listener hands it to -- no
/// claim-ack, and nothing journaled on the peering's channel, read off the
/// binary's own `state_dir`.
#[tokio::test]
async fn a_claim_that_does_not_prove_the_peer_role_reaches_no_peer_handling_over_http() {
    a_claim_that_does_not_prove_the_peer_role_reaches_no_peer_handling(Carriage::Http).await;
}

/// The BTP twin: §1.9 requires the regression on **both** carriages.
#[tokio::test]
async fn a_claim_that_does_not_prove_the_peer_role_reaches_no_peer_handling_over_btp() {
    a_claim_that_does_not_prove_the_peer_role_reaches_no_peer_handling(Carriage::Btp).await;
}

async fn a_claim_that_does_not_prove_the_peer_role_reaches_no_peer_handling(carriage: Carriage) {
    let Some(fixture) = PeerFixture::spawn().await else {
        return;
    };
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let stub_app = spawn_stub_app();
    let (payee, _config, _key, _settlement_key) =
        spawn_payee(&fixture, state_dir.path(), &stub_app.addr);
    let payee_identity = identity_from_key_seed(PAYEE_SIGNER_SEED);
    let client = reqwest::Client::new();

    let mut client_channels: Vec<String> = Vec::new();
    for RefusedCase {
        name: case,
        claim,
        pays_as_client,
    } in fixture.refused_claims()
    {
        let (data, _shared) = sealed_prepare_data(case.as_bytes(), &payee_identity);
        let prepare = sample_prepare(APP_PREFIX, data);
        let ack = present(
            carriage,
            &client,
            &payee.client_edge_addr,
            claim.as_deref(),
            &prepare,
        )
        .await;
        assert_eq!(
            ack, None,
            "{case}: §1.7 -- a connector MUST NOT emit a claim-ack on a client interaction"
        );
        assert!(
            journaled(state_dir.path(), &fixture.payer_channel).is_empty(),
            "{case}: §1.9 -- nothing may be journaled on the peering's channel"
        );
        if let Some((channel, amount)) = pays_as_client {
            assert_eq!(
                journaled(state_dir.path(), &channel),
                vec![amount],
                "{case}: a genuine payment on a channel of its own is admitted as a \
                 client's, and journaled as one"
            );
            client_channels.push(format!("evm:{channel}"));
        }
        let journal = client_edge_journal(state_dir.path());
        assert!(
            journal.lines().all(|line| line
                .split('\t')
                .nth(1)
                .is_some_and(|key| client_channels.iter().any(|client| client == key))),
            "{case}: §1.9 -- nothing may be journaled but a genuine client payment. Journal \
             was:\n{journal}"
        );
    }

    // The retired claim is refused by name, not merely read as nothing: on
    // this shared listener it is a client's, and the client edge names the
    // retirement in its REJECT (#1384).
    let (data, _shared) = sealed_prepare_data(b"toon-channel", &payee_identity);
    let prepare = sample_prepare(APP_PREFIX, data);
    let claim = toon_channel_claim();
    let packet = match carriage {
        Carriage::Btp => {
            send_peer_message(&payee.client_edge_addr, Some(&claim), &prepare)
                .await
                .0
        }
        Carriage::Http => {
            post_peer_request(&client, &payee.client_edge_addr, Some(&claim), &prepare)
                .await
                .1
        }
    };
    let reject = Reject::decode(&packet).expect("a toon-channel claim is refused");
    assert!(
        reject.message.contains("toon-channel") && reject.message.contains("ADR 0075"),
        "refused by name: {}",
        reject.message
    );
}

/// **§1.9 case 4, in the only form a live binary can express it**: a
/// peering with no `[[peer_channels]]` row is refused at load
/// (`ConfigError::PeerChannelUnbound`), so the process never carries a
/// peering nothing can prove.
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

    let stderr = refusal_of(config.path());
    assert!(
        stderr.contains("unbound") && stderr.contains("[[peer_channels]]"),
        "the refusal must name the peering and the missing table: {stderr}"
    );
}

/// **A `[[pay_channels]]` row is this node's OWN channel** (ADR 0075
/// decision 4, issue #1380): one naming an x402 channel this node's
/// outbound-channel journal does not hold -- here, an empty journal -- is
/// refused at boot, naming the peer and the channel, rather than refusing
/// every forward at packet time while the file reads as configured.
#[tokio::test]
async fn a_pay_channel_this_node_never_opened_refuses_to_start() {
    let Some(chain) = Chain::spawn().await else {
        return;
    };
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let key_file = write_raw_key_file(PAYER_SIGNER_SEED);
    let settlement_key = settlement_key_file(PAYER_SETTLEMENT_KEY);
    let never_opened = format!("0x{}", "ab".repeat(32));
    let payee = wallet_of(&key_bytes(PAYEE_SETTLEMENT_KEY)).address();
    let config = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"
peer_allow_plaintext_endpoints = true

[signer]
key_file = "{key_file}"
{settlement}
[[routes]]
prefix = "{PEER_ROUTE_PREFIX}"
peer_id = "{PAYEE_ID}"
price = 0

[[peers]]
id = "{PAYEE_ID}"
endpoint = "http://127.0.0.1:9/ilp"

[[peer_channels]]
peer_id = "{PAYEE_ID}"
voucher_signer = "{payee}"

[[pay_channels]]
peer_id = "{PAYEE_ID}"
outbound_channel = "{never_opened}"
client_edge_url = "http://127.0.0.1:9/ilp"
"#,
        state_dir = state_dir.path().display(),
        key_file = key_file.path().display(),
        settlement = chain.settlement_block(settlement_key.path()),
        payee = spelled(payee),
    ));

    let stderr = refusal_of(config.path());
    assert!(
        stderr.contains(PAYEE_ID) && stderr.contains(&never_opened),
        "the refusal must name the peering and the channel its journal does not hold: {stderr}"
    );
}

/// Run the binary on `config`, which it must refuse, and return its stderr.
/// Bounded: a node that wrongly starts is killed and failed, never waited on
/// forever.
fn refusal_of(config: &std::path::Path) -> String {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_connector"))
        .arg(config)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("run the connector binary");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll the connector") {
            break status;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the node started on a config it must refuse");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let mut stderr = String::new();
    std::io::Read::read_to_string(child.stderr.as_mut().expect("piped stderr"), &mut stderr)
        .expect("read stderr");
    assert!(
        !status.success(),
        "the node must refuse to start rather than carry this peering: {stderr}"
    );
    stderr
}

// ---------------------------------------------------------------------------
// A packet crosses the peering, over both carriages.
// ---------------------------------------------------------------------------

/// **A packet crosses a config-declared x402 peering over `ws://` BTP and is
/// fulfilled**, and the payee's journal shows the payer's voucher advanced
/// by exactly what was forwarded.
#[tokio::test]
async fn two_connectors_move_a_paid_packet_over_btp() {
    two_connectors_move_a_paid_packet(Carriage::Btp).await;
}

/// **The same over `http://`.** Only the dialing side can originate on
/// HTTP (§6.4), which is why the payee's `[[peers]]` entry has no
/// `endpoint`.
#[tokio::test]
async fn two_connectors_move_a_paid_packet_over_http() {
    two_connectors_move_a_paid_packet(Carriage::Http).await;
}

/// A claimless client PREPARE, and the x402 terms it must be answered with
/// (client-edge-spec.md §1.4).
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
        "a claimless client request to a priced forwarded route must be greeted, not carried"
    );
    response.json().await.expect("x402 JSON terms")
}

fn payee_endpoint(carriage: Carriage, payee: &ConnectorProcess) -> String {
    format!(
        "{}://{}{}",
        carriage.scheme(),
        payee.client_edge_addr,
        match carriage {
            Carriage::Btp => "/ilp/btp",
            Carriage::Http => "/ilp",
        }
    )
}

async fn two_connectors_move_a_paid_packet(carriage: Carriage) {
    let Some(fixture) = PeerFixture::spawn().await else {
        return;
    };
    let payee_state = tempfile::tempdir().expect("temp payee state dir");
    let stub_app = spawn_stub_app();
    let (payee, _payee_config, _payee_key, _payee_settlement) =
        spawn_payee(&fixture, payee_state.path(), &stub_app.addr);
    let payee_client_edge = format!("http://{}/ilp", payee.client_edge_addr);
    let (payer, _payer_config, _payer_key, _payer_settlement) = spawn_payer(
        &fixture,
        carriage,
        &payee_endpoint(carriage, &payee),
        &payee_client_edge,
    );

    // Sealed to the payee: it terminates there, and the payer is a
    // forwarding hop that cannot open it (§8.1).
    let payee_identity = identity_from_key_seed(PAYEE_SIGNER_SEED);
    let client = reqwest::Client::new();

    // The client leg, refused when unpaid (ADR 0028) -- asserted first, so a
    // regression that made the route free could not hide behind the paid
    // crossings below.
    let terms = claimless_client_terms(
        &client,
        &payer.client_edge_addr,
        &peer_bound_prepare(APP_PREFIX, b"unpaid", &payee_identity),
    )
    .await;
    assert_eq!(
        terms["extensions"]["toon"]["info"]["price"],
        CLIENT_PRICE.to_string()
    );
    assert!(
        journaled(payee_state.path(), &fixture.payer_channel).is_empty(),
        "an unpaid client request moves nothing across the peering"
    );

    // The client leg, paid: a real client voucher on the payer's edge,
    // advancing by CLIENT_PRICE per packet.
    for (nonce, body) in [
        (1, b"across the peering".as_slice()),
        (2, b"across the peering again".as_slice()),
    ] {
        let (data, _shared) = sealed_prepare_data(body, &payee_identity);
        let prepare = Prepare {
            amount: CLIENT_PRICE,
            ..sample_prepare(APP_PREFIX, data)
        };
        let (status, answer, _ack) = post_peer_request(
            &client,
            &payer.client_edge_addr,
            Some(&fixture.client_claim(nonce)),
            &prepare,
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::OK);
        connector_domain::Fulfill::decode(&answer).unwrap_or_else(|_| {
            let reject =
                Reject::decode(&answer).expect("an answer that is neither FULFILL nor REJECT");
            panic!(
                "crossing {nonce} did not cross the peering: {} {}",
                reject.code.as_str(),
                reject.message
            )
        });
        // The money on the peer leg: the payee journaled the payer's
        // voucher on the payer's own channel, advanced by exactly what the
        // payer forwarded -- the packet carried CLIENT_PRICE, the payer kept
        // its PEER_FEE (ADR 0061), and FORWARDED reached the payee.
        assert_eq!(
            journaled(payee_state.path(), &fixture.payer_channel),
            (1..=nonce).map(|n| n * FORWARDED).collect::<Vec<_>>(),
            "crossing {nonce}: the payee's client-edge journal must record the payer's voucher \
             advanced by exactly {FORWARDED}. Journal was:\n{}",
            client_edge_journal(payee_state.path())
        );
    }

    // And the client leg was charged, in the payer's own journal.
    assert_eq!(
        journaled(fixture.payer_state.path(), &fixture.client_channel),
        vec![CLIENT_PRICE, 2 * CLIENT_PRICE],
        "the client leg must be charged on the client's channel. Journal was:\n{}",
        client_edge_journal(fixture.payer_state.path())
    );
}

/// **The amount a priced forwarded route will carry is bounded by its
/// price** (ADR 0028): a client paying `CLIENT_PRICE` and declaring more is
/// refused `F03` before its claim is ingested. One carriage suffices: the
/// rule lives in `over_carried_reject`, which both carriages share.
#[tokio::test]
async fn a_client_may_not_declare_more_than_the_forwarded_route_charges() {
    let Some(fixture) = PeerFixture::spawn().await else {
        return;
    };
    let payee_state = tempfile::tempdir().expect("temp payee state dir");
    let stub_app = spawn_stub_app();
    let (payee, _payee_config, _payee_key, _payee_settlement) =
        spawn_payee(&fixture, payee_state.path(), &stub_app.addr);
    let endpoint = format!("http://{}/ilp", payee.client_edge_addr);
    let (payer, _payer_config, _payer_key, _payer_settlement) =
        spawn_payer(&fixture, Carriage::Http, &endpoint, &endpoint);

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
    let accumulated_cost = response
        .headers()
        .get("toon-accumulated-cost")
        .expect("every REJECT this edge answers carries its running cost")
        .to_str()
        .expect("the accumulated-cost header is ASCII")
        .to_string();
    let body = response.bytes().await.expect("response body").to_vec();
    let reject = Reject::decode(&body).expect("an over-carried packet is refused, not fulfilled");
    assert_eq!(reject.code.as_str(), "F03", "{}", reject.message);
    assert_eq!(accumulated_cost, CLIENT_PRICE.to_string());
    assert!(
        client_edge_journal(fixture.payer_state.path()).is_empty(),
        "the claim must not be spent on a packet this connector refused to carry. Journal \
         was:\n{}",
        client_edge_journal(fixture.payer_state.path())
    );
    assert!(
        journaled(payee_state.path(), &fixture.payer_channel).is_empty(),
        "and nothing crossed the peering"
    );
}

/// **A packet whose forwarded amount is below the terminating route's price
/// is not paid for twice** (#1462). The payee greets the forward with its
/// terms; a retry changes the voucher and not the packet, so it could only
/// end `F03` after a second admitted voucher. The payer signs none: the
/// payee's journal holds the one voucher the forward rode.
#[tokio::test]
async fn a_forward_below_the_terminating_price_is_not_retried_over_btp() {
    let Some(fixture) = PeerFixture::spawn().await else {
        return;
    };
    let payee_state = tempfile::tempdir().expect("temp payee state dir");
    let stub_app = spawn_stub_app();
    let price = FORWARDED + 5;
    let (payee, _payee_config, _payee_key, _payee_settlement) =
        spawn_payee_priced(&fixture, payee_state.path(), &stub_app.addr, price);
    let payee_client_edge = format!("http://{}/ilp", payee.client_edge_addr);
    let (payer, _payer_config, _payer_key, _payer_settlement) = spawn_payer(
        &fixture,
        Carriage::Btp,
        &payee_endpoint(Carriage::Btp, &payee),
        &payee_client_edge,
    );

    let payee_identity = identity_from_key_seed(PAYEE_SIGNER_SEED);
    let (data, _shared) = sealed_prepare_data(b"too cheap", &payee_identity);
    let prepare = Prepare {
        amount: CLIENT_PRICE,
        ..sample_prepare(APP_PREFIX, data)
    };
    let (status, answer, _ack) = post_peer_request(
        &reqwest::Client::new(),
        &payer.client_edge_addr,
        Some(&fixture.client_claim(1)),
        &prepare,
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::OK);
    let reject = Reject::decode(&answer).expect("a packet below the price is refused");
    assert!(
        reject.message.contains(&price.to_string())
            && reject.message.contains(&FORWARDED.to_string()),
        "the reject must name the quoted price {price} and the amount {FORWARDED}: {}",
        reject.message
    );
    assert_eq!(
        journaled(payee_state.path(), &fixture.payer_channel),
        vec![FORWARDED],
        "the payee must hold one voucher, not a second for the price. Journal was:\n{}",
        client_edge_journal(payee_state.path())
    );
}

// ---------------------------------------------------------------------------
// A peer voucher is acknowledged, and re-acknowledged, over both carriages.
// ---------------------------------------------------------------------------

/// **A peer voucher is acknowledged, and a byte-identical resend is
/// acknowledged again, over `ws://` BTP.** Driven straight at the payee's
/// binary as a peer would -- §6.3's resend rule is a property of what the
/// payee answers, and a payer that never loses an ack never resends.
#[tokio::test]
async fn a_peer_voucher_is_acknowledged_over_btp() {
    a_peer_voucher_is_acknowledged(Carriage::Btp).await;
}

/// **The same over `http://`**, the ack riding `Toon-Claim-Ack` and the
/// status 200 regardless of the verdict (§6.2).
#[tokio::test]
async fn a_peer_voucher_is_acknowledged_over_http() {
    a_peer_voucher_is_acknowledged(Carriage::Http).await;
}

async fn a_peer_voucher_is_acknowledged(carriage: Carriage) {
    let Some(fixture) = PeerFixture::spawn().await else {
        return;
    };
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let stub_app = spawn_stub_app();
    let (payee, _config, _key, _settlement_key) =
        spawn_payee(&fixture, state_dir.path(), &stub_app.addr);
    let payee_identity = identity_from_key_seed(PAYEE_SIGNER_SEED);
    let client = reqwest::Client::new();

    // §6.3 is about bytes: the resend below reuses this exact string.
    let voucher = fixture.payer_voucher(u128::from(FORWARDED));

    let ack_of = |body: &'static [u8], claim: String| {
        let (data, _shared) = sealed_prepare_data(body, &payee_identity);
        let prepare = sample_prepare(APP_PREFIX, data);
        let addr = payee.client_edge_addr.clone();
        let client = client.clone();
        async move { present(carriage, &client, &addr, Some(&claim), &prepare).await }
    };

    // (1) The payer's voucher proves the peer role, is acked, and is
    // journaled on the payer's channel.
    let first = ack_of(b"first crossing", voucher.clone()).await;
    assert_eq!(
        first.as_deref(),
        Some(r#"{"result":"accepted"}"#),
        "§6.1: the ack rides the response that already answers the voucher-bearing frame"
    );
    assert_eq!(
        journaled(state_dir.path(), &fixture.payer_channel),
        vec![FORWARDED],
        "an accepted peer voucher is journaled at its cumulative amount"
    );

    // (2) The same bytes again: `accepted`, never a refusal, and nothing
    // new journaled.
    let resent = ack_of(b"retransmission", voucher).await;
    assert_eq!(
        resent.as_deref(),
        Some(r#"{"result":"accepted"}"#),
        "§6.3: a voucher byte-identical to the one at the watermark MUST be re-acked \
         `accepted` -- a lost ack and a lost voucher are indistinguishable at the payer"
    );
    assert_eq!(
        journaled(state_dir.path(), &fixture.payer_channel),
        vec![FORWARDED],
        "a resend advances nothing"
    );

    // (3) The narrowing is one voucher wide: a genuine voucher by the bound
    // key below the channel's watermark is still the peer's, and refused.
    let below = ack_of(
        b"below the watermark",
        fixture.payer_voucher(u128::from(FORWARDED) - 1),
    )
    .await
    .expect("a peer's voucher is acknowledged, whatever its verdict");
    assert!(
        below.contains(r#""rejected""#) && below.contains("amount_not_advancing"),
        "a voucher below the watermark is refused as not advancing: {below}"
    );
}
