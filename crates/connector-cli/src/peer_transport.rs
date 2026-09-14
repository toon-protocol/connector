//! **The dial side, built from configuration** (issue #678's gap 2,
//! `docs/protocol/peer-carriage-spec.md` §2).
//!
//! `BtpPeerTransport` and `HttpPeerTransport` each implement
//! [`PeerTransport`] over the peerings they dial, and each is deliberately
//! blind to the other. A `[[peers]]` table may hold both, though, so
//! something has to hand a packet to the right one -- and that something is
//! [`ConfiguredPeerTransport`], which is one more [`PeerTransport`] and
//! nothing else: it adds no policy, no fee, no ceiling and no claim
//! handling, because every one of those lives above this port (spec I5).
//!
//! # The carriage is the endpoint's scheme, and nothing else (§2.1)
//!
//! `wss://` is BTP, `https://` is ILP-over-HTTP, and -- for a node that
//! opted into `peer_allow_plaintext_endpoints` (issue #678 gap 3) -- `ws://`
//! and `http://` are the same two carriages without TLS. `Config::load` has
//! already decided which, so this module never re-derives it: each
//! carriage's own `PeerRelation::from_config` filters by
//! [`PeerConfig::dial`], and this map is built from the same answer.
//!
//! # A peer with no carriage still answers `T01`
//!
//! An accept-only peering (no `endpoint`) is one this connector never
//! dials: it dials in. A packet routed to one -- which config load already
//! refuses where it can see it (`PeerRouteUndeliverable`) -- must still be
//! **rejected `T01` with the peer named, never `T00` and never a silent
//! drop** (§2.2). That is exactly what an empty [`InProcessPeerTransport`]
//! answers, so it is what an unmapped peer id falls through to, rather than
//! this module minting a second copy of the same reject.
//!
//! # It is no longer only built from configuration (ADR 0058)
//!
//! This module's title stopped being the whole truth when a peering could
//! be established over the operator surface. `build_peer_transport` running
//! once at boot from `config.peers()` is exactly what made a runtime peer
//! row a name with nothing behind it, so [`ConfiguredPeerTransport`] is
//! also a [`PeerRegistrar`]: a peering added while the process serves gets
//! its carriage here, and a peering removed loses it.
//!
//! Two consequences follow, and both are deliberate. **Both carriages are
//! always built**, because a runtime peering's endpoint may select the one
//! this node's config never named. And a registered relation reads its
//! claim bindings off the durable row rather than off `[[peer_channels]]`,
//! which is the same two maps `PeerRelation::from_config` builds, from the
//! other source.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use connector_config::{Config, PeerCarriage, DEFAULT_PEER_TIMEOUT_MS};
use connector_domain::Prepare;
use connector_peer_btp::claim_json::canonical_evm_channel_id;
use connector_peer_btp::{BtpPeerTransport, PeerClaimDomain, TungsteniteDialer};
use connector_peer_http::{HttpPeerTransport, ReqwestPeerClient};
use connector_runtime::{
    ClaimAckOutcome, Clock, InProcessPeerTransport, PeerForward, PeerRegistrar, PeerTransport,
    RuntimePeerChannel, RuntimePeering, WireClaim,
};

/// One [`PeerTransport`] over both carriages, dispatching by peer id --
/// and, since ADR 0058, one [`PeerRegistrar`] as well.
pub(crate) struct ConfiguredPeerTransport {
    btp: BtpPeerTransport,
    http: HttpPeerTransport,
    /// Peer id → the carriage its endpoint's scheme selected. A peer id
    /// absent from this map is one this connector cannot dial at all.
    ///
    /// Copy-on-write behind an [`ArcSwap`], for the same reason each
    /// carriage's own relation map is: the packet path reads it, and an
    /// operator write changes it.
    carriage: ArcSwap<HashMap<String, PeerCarriage>>,
    /// Registered with no peers, so every unmapped peer id gets §2.2's
    /// `T01` from the one place that already produces it.
    unreachable: InProcessPeerTransport,
    /// The node's own `peer_allow_plaintext_endpoints`, so a peering
    /// registered at runtime picks its carriage by exactly the rule a
    /// config-file peering did.
    allow_plaintext: bool,
}

/// The transport a validated [`Config`] describes.
///
/// `signer_address` is this node's own EVM address -- the `senderId` /
/// `signerAddress` of every EVM claim it emits (§4). `signer_solana_public_key`
/// is the Solana counterpart (issue #732/#998) -- the raw ed25519 public key
/// rendered as `senderId`/`signerPublicKey` on a Solana claim -- `None` for a
/// node with no `[settlement.solana]` table, exactly mirroring how
/// `signer_address` is all-zero and unused on a node with no
/// `[settlement.evm]` table, which never produces an EVM claim to render
/// either. Without it, a dial side that DID sign a Solana claim
/// (`ClaimBook::record_fulfillment`, once a `[[peer_channels]]` Solana row is
/// wired) would panic trying to render one -- see `claim_json::encode`'s own
/// doc. `clock` is the one the rest of the node reads, so a claim's
/// `timestamp` and a fulfilment's agree.
///
/// A node with no dialable peering registers no carriage at all, and every
/// peer-routed packet falls through to the bare [`InProcessPeerTransport`]
/// this holds: `T01 peer unreachable`, with the peer named.
pub(crate) fn build_peer_transport(
    config: &Config,
    signer_address: [u8; 20],
    signer_solana_public_key: Option<[u8; 32]>,
    clock: Arc<dyn Clock>,
) -> Arc<ConfiguredPeerTransport> {
    let mut carriage = HashMap::new();
    for peer in config.peers() {
        if let Some(dial) = peer.dial() {
            carriage.insert(peer.id().to_string(), dial);
        }
    }

    // ADR 0070 decision 3: the one `socks_proxy` this node dials onion
    // endpoints through, read here because here is where the carriages are
    // built and here is the only place it is needed. It is handed to the
    // thing that owns the *socket* on each carriage -- the HTTP client and
    // the websocket dialer -- never to the transport, and each selects per
    // endpoint by host. So a peering registered at runtime onto either of
    // these same two transports (ADR 0058, `PeerRegistrar` below) dials
    // through the proxy for free, with no second decision anywhere that
    // could disagree with this one. Nothing else on this node is proxied:
    // settlement RPC and the app's `handler_url` hold their own clients
    // (decision 4).
    let socks_proxy = config.socks_proxy();

    // Ask-only (`TungsteniteDialer::new`) rather than symmetric
    // (`::serving`): §2.3's inbound half of a *dialed* session needs a
    // `PeerCarriageState`, which needs the `Connector` this transport is
    // about to be handed to. Answers still correlate; what a dialed session
    // cannot yet do is serve a request the far side originates on it, which
    // no gate of issue #678 exercises -- the far side of a `wss://` peering
    // reaches this node on its own listener like everybody else.
    let mut btp = BtpPeerTransport::new(
        Arc::new(TungsteniteDialer::new().through_socks_proxy(socks_proxy)),
        signer_address,
        Arc::clone(&clock),
    );
    let http_client = match socks_proxy {
        Some(proxy) => ReqwestPeerClient::through_socks_proxy(proxy),
        None => ReqwestPeerClient::default(),
    };
    let mut http = HttpPeerTransport::new(Arc::new(http_client), signer_address, clock);
    if let Some(public_key) = signer_solana_public_key {
        btp.set_solana_signer_public_key(public_key);
        http.set_solana_signer_public_key(public_key);
    }
    btp.add_peers_from_config(config.peers(), config.peer_channels());
    http.add_peers_from_config(config.peers(), config.peer_channels());

    Arc::new(ConfiguredPeerTransport {
        btp,
        http,
        carriage: ArcSwap::from_pointee(carriage),
        unreachable: InProcessPeerTransport::new(),
        allow_plaintext: config.peer_allow_plaintext_endpoints(),
    })
}

impl ConfiguredPeerTransport {
    /// The carriage `peer_id` is dialed on, as a transport. `None` for a
    /// peer this connector does not dial.
    fn transport_for(&self, peer_id: &str) -> Option<&dyn PeerTransport> {
        match self.carriage.load().get(peer_id)? {
            PeerCarriage::Btp => Some(&self.btp as &dyn PeerTransport),
            PeerCarriage::Http => Some(&self.http as &dyn PeerTransport),
        }
    }

    /// Replace the peer-id → carriage map with a copy that has `change`
    /// applied.
    fn rebind(&self, change: impl FnOnce(&mut HashMap<String, PeerCarriage>)) {
        let mut next = (**self.carriage.load()).clone();
        change(&mut next);
        self.carriage.store(Arc::new(next));
    }
}

/// ADR 0058: a peering established over the operator surface becomes
/// dialable **while this process serves**, and a removed one stops being
/// dialed -- no restart in either direction.
///
/// The relation is registered on whichever carriage the peering's endpoint
/// scheme selects, and on that one only: §2.1's rule, applied through
/// `connector_config::PeerCarriage` rather than restated here.
impl PeerRegistrar for ConfiguredPeerTransport {
    fn register(&self, peer_id: &str, peering: &RuntimePeering) {
        let (Some(endpoint), Some(carriage)) =
            (peering.endpoint_url(), peering.dial(self.allow_plaintext))
        else {
            // No endpoint, or one whose scheme selects no carriage this
            // node dials. Deregister rather than leave a stale mapping in
            // place: the peering's answer is then §2.2's `T01` with the
            // peer named, which is what an unmapped id already falls
            // through to.
            self.deregister(peer_id);
            return;
        };
        let (domains, programs) = claim_bindings(peering);
        let answer_timeout = Duration::from_millis(DEFAULT_PEER_TIMEOUT_MS);
        match carriage {
            PeerCarriage::Btp => self.btp.add_peer(connector_peer_btp::PeerRelation::new(
                peer_id,
                endpoint,
                domains,
                programs,
                answer_timeout,
                answer_timeout,
            )),
            PeerCarriage::Http => self.http.add_peer(connector_peer_http::PeerRelation::new(
                peer_id,
                endpoint,
                domains,
                programs,
                answer_timeout,
                answer_timeout,
            )),
        }
        self.rebind(|map| {
            map.insert(peer_id.to_string(), carriage);
        });
    }

    fn deregister(&self, peer_id: &str) {
        // Removed from both, not from whichever the map says: a peering
        // re-registered onto the other carriage would otherwise leave a
        // relation behind on the first.
        self.btp.remove_peer(peer_id);
        self.http.remove_peer(peer_id);
        self.rebind(|map| {
            map.remove(peer_id);
        });
    }
}

/// The EIP-712 domains a runtime peering's EVM channels sign under, and the
/// programs its Solana channels bind to (ADR 0053) -- the same two maps
/// `PeerRelation::from_config` builds out of `[[peer_channels]]`, built
/// instead out of the durable row.
///
/// A binding whose `token_network` is not a readable address is skipped
/// rather than defaulted: a claim signed under a zero `verifyingContract`
/// verifies nowhere, and producing no claim at all is what a channel with
/// no domain has always done.
fn claim_bindings(
    peering: &RuntimePeering,
) -> (HashMap<String, PeerClaimDomain>, HashMap<String, String>) {
    let mut domains = HashMap::new();
    let mut programs = HashMap::new();
    for binding in &peering.channels {
        match binding {
            RuntimePeerChannel::Evm {
                channel_id,
                chain_id,
                token_network,
                ..
            } => {
                let Some(token_network) = parse_evm_address(token_network) else {
                    continue;
                };
                domains.insert(
                    canonical_evm_channel_id(channel_id),
                    PeerClaimDomain {
                        chain_id: *chain_id,
                        token_network,
                    },
                );
            }
            RuntimePeerChannel::Solana {
                channel_account,
                program_id,
                ..
            } => {
                programs.insert(channel_account.clone(), program_id.clone());
            }
        }
    }
    (domains, programs)
}

/// A 20-byte EVM address from its hex spelling, or `None` -- never a padded
/// or truncated one.
fn parse_evm_address(value: &str) -> Option<[u8; 20]> {
    let hex = value.strip_prefix("0x").unwrap_or(value);
    if hex.len() != 40 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut address = [0u8; 20];
    for (i, byte) in address.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(address)
}

#[async_trait]
impl PeerTransport for ConfiguredPeerTransport {
    async fn forward(
        &self,
        peer_id: &str,
        prepare: Prepare,
        claim: Option<WireClaim>,
    ) -> PeerForward {
        match self.transport_for(peer_id) {
            Some(transport) => transport.forward(peer_id, prepare, claim).await,
            None => self.unreachable.forward(peer_id, prepare, claim).await,
        }
    }

    async fn flush(&self, peer_id: &str, claim: WireClaim) -> ClaimAckOutcome {
        match self.transport_for(peer_id) {
            Some(transport) => transport.flush(peer_id, claim).await,
            None => self.unreachable.flush(peer_id, claim).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    use chrono::{TimeZone, Utc};
    use connector_domain::{PacketResponse, RejectCode};
    use connector_runtime::SystemClock;

    /// A config with `top` (top-level keys, which TOML requires before any
    /// table) and `peers` spliced in, loaded through the real loader --
    /// `PeerConfig` is constructible no other way, and a hand-built one
    /// would be a shape a node can never hold.
    fn config(top: &str, peers: &str) -> (Config, tempfile::TempDir, tempfile::NamedTempFile) {
        let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
        key_file.write_all(&[7u8; 32]).expect("write key file");
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let mut config_file = tempfile::NamedTempFile::new().expect("temp config file");
        write!(
            config_file,
            r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"
peer_allow_plaintext_endpoints = true
{top}

[signer]
key_file = "{key_file}"

# An EVM `[[peer_channels]]` row needs `[settlement.evm]` (issue #1138):
# that table is where this node's EVM address comes from, and a peer claim
# is redeemed by the channel's on-chain participant.
[settlement.evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "0x1234567890123456789012345678901234567890"
token_address = "0x49beE1Bca5d15Fb0963117923403F9498119a9Ce"
decimals = 6

[settlement.evm.key]
key_file = "{key_file}"
{peers}
"#,
            state_dir = state_dir.path().display(),
            key_file = key_file.path().display(),
        )
        .expect("write config file");
        let config = Config::load(config_file.path()).expect("load the peering config");
        (config, state_dir, key_file)
    }

    /// One peering, with a `channel_id` derived from `tag` so two peerings
    /// in one config do not collide (`PeerChannelDuplicate`).
    fn peer_block(id: &str, endpoint: &str, tag: &str) -> String {
        format!(
            r#"
[[peers]]
id = "{id}"
endpoint = "{endpoint}"


[[peer_channels]]
peer_id = "{id}"
channel_id = "0x{channel}"
counterparty_key = "0x00000000000000000000000000000000000000aa"
chain_id = 31337
token_network = "0x00000000000000000000000000000000000000bb"
"#,
            channel = tag.repeat(64),
        )
    }

    fn prepare(destination: &str) -> Prepare {
        Prepare {
            amount: 100,
            expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
            greeting: false,
            destination: destination.to_string(),
            data: Vec::new(),
        }
    }

    fn t01(response: &PacketResponse) -> bool {
        matches!(response, PacketResponse::Reject(reject)
            if reject.code.as_str() == RejectCode::t01_peer_unreachable().as_str())
    }

    /// §2.1: the carriage is the endpoint's scheme, so a `[[peers]]` table
    /// naming both is one transport over two -- dispatched by peer id, with
    /// nothing above the port able to tell which answered.
    #[tokio::test]
    async fn a_config_naming_both_schemes_builds_both_carriages() {
        let (config, _state, _key) = config(
            "",
            &format!(
                "{}{}",
                peer_block("over-btp", "ws://127.0.0.1:1/ilp/btp", "a"),
                peer_block("over-http", "http://127.0.0.1:1/ilp", "b"),
            ),
        );

        let transport = build_peer_transport(&config, [0u8; 20], None, Arc::new(SystemClock));

        // Nothing is listening on port 1, so both answer §2.2's `T01` --
        // the point being that each was *dialed*, on its own carriage,
        // rather than falling through to the unmapped path.
        for peer_id in ["over-btp", "over-http"] {
            let PeerForward {
                response,
                ack,
                reached_peer: reached,
                ..
            } = transport
                .forward(peer_id, prepare("g.example.app"), None)
                .await;
            assert!(t01(&response), "{peer_id}: {response:?}");
            assert_eq!(ack, ClaimAckOutcome::NotSent);
            assert!(!reached, "{peer_id} was never actually reached");
        }
    }

    /// §2.2: a peer id this connector cannot dial rejects **`T01` with the
    /// peer named, never `T00` and never a silent drop** -- the behaviour a
    /// node with no carriage at all has always had, kept for a node that
    /// has one and simply does not dial *this* peer.
    #[tokio::test]
    async fn a_peer_this_connector_never_dials_is_still_answered_t01() {
        let (config, _state, _key) =
            config("", &peer_block("dialed", "ws://127.0.0.1:1/ilp/btp", "a"));
        let transport = build_peer_transport(&config, [0u8; 20], None, Arc::new(SystemClock));

        let PeerForward {
            response,
            reached_peer: reached,
            ..
        } = transport
            .forward("never-configured", prepare("g.example.app"), None)
            .await;

        match response {
            PacketResponse::Reject(reject) => {
                assert_eq!(reject.code.as_str(), "T01");
                assert!(
                    reject.message.contains("never-configured"),
                    "a dial failure names the peer: {}",
                    reject.message
                );
            }
            other => panic!("expected a T01 reject, got {other:?}"),
        }
        assert!(!reached);
    }

    /// A node with only accept-only peerings dials nothing, and holds
    /// exactly what it held before this module existed.
    #[tokio::test]
    async fn a_node_with_nothing_to_dial_answers_t01_from_an_empty_transport() {
        let (config, _state, _key) = config(
            r#"peer_expose = "btp""#,
            r#"
[[peers]]
id = "dials-in"


[[peer_channels]]
peer_id = "dials-in"
channel_id = "0xaaaabbbbccccddddeeeeffff00001111aaaabbbbccccddddeeeeffff00001111"
counterparty_key = "0x00000000000000000000000000000000000000aa"
chain_id = 31337
token_network = "0x00000000000000000000000000000000000000bb"
"#,
        );

        let transport = build_peer_transport(&config, [0u8; 20], None, Arc::new(SystemClock));

        let PeerForward {
            response,
            reached_peer: reached,
            ..
        } = transport
            .forward("dials-in", prepare("g.example.app"), None)
            .await;

        assert!(t01(&response), "{response:?}");
        assert!(!reached);
    }

    /// A [`RuntimePeering`] as `POST /peers` writes one, endpoint and all.
    fn runtime_peering(endpoint: &str) -> RuntimePeering {
        RuntimePeering {
            fee: 100,
            max_packet_amount: 5_000,
            endpoint: Some(endpoint.to_string()),
            edge_identity: Some("0x04ab".to_string()),
            client_edge_url: Some(endpoint.to_string()),
            channels: vec![RuntimePeerChannel::Evm {
                channel_id: format!("0x{}", "ab".repeat(32)),
                counterparty_key: "0x00000000000000000000000000000000000000aa".to_string(),
                chain_id: 31337,
                token_network: "0x00000000000000000000000000000000000000bb".to_string(),
            }],
        }
    }

    /// ADR 0058: **`build_peer_transport` adds and removes a carriage
    /// while the process serves.** Running it once at boot is what made a
    /// runtime peer row hollow.
    ///
    /// The proof is the change in what a forward to one peer id does. It
    /// is unmapped and falls through to §2.2's `T01` from an empty
    /// transport; then it is registered and the forward is genuinely
    /// dialed -- still `T01`, because nothing is listening on port 1, but
    /// `reached_peer` and the dial attempt are the difference; then it is
    /// deregistered and it falls through again. Same transport value
    /// throughout, with no rebuild and no restart.
    #[tokio::test]
    async fn a_carriage_is_added_and_removed_while_the_transport_serves() {
        // A config with no `[[peers]]` at all: everything below is
        // established over the operator surface.
        let (config, _state, _key) = config("", "");
        let transport = build_peer_transport(&config, [0u8; 20], None, Arc::new(SystemClock));

        let forward = |transport: Arc<ConfiguredPeerTransport>| async move {
            transport
                .forward("added-at-runtime", prepare("g.example.app"), None)
                .await
        };

        // Before: unmapped, and answered by the empty in-process
        // transport rather than by a dial.
        let PeerForward { response, .. } = forward(Arc::clone(&transport)).await;
        match response {
            PacketResponse::Reject(reject) => {
                assert_eq!(reject.code.as_str(), "T01");
                assert!(reject.message.contains("added-at-runtime"));
            }
            other => panic!("expected T01, got {other:?}"),
        }

        // Register. `http://` selects ILP-over-HTTP because this config
        // opted into plaintext endpoints -- §2.1's rule, read through
        // `connector_config` rather than restated here.
        transport.register(
            "added-at-runtime",
            &runtime_peering("http://127.0.0.1:1/ilp"),
        );
        let PeerForward {
            response,
            reached_peer,
            ..
        } = forward(Arc::clone(&transport)).await;
        assert!(t01(&response), "nothing listens on port 1: {response:?}");
        assert!(!reached_peer, "the dial failed, which is the point");

        // A `wss://` endpoint re-registers the same id onto the OTHER
        // carriage. Nothing is left behind on the first: the carriage is
        // the endpoint's scheme, and re-registering must not leave a
        // relation this node would still dial.
        transport.register(
            "added-at-runtime",
            &runtime_peering("ws://127.0.0.1:1/ilp/btp"),
        );
        assert!(transport.http.flush_hints("added-at-runtime").is_empty());

        // Deregister -- the other half of ADR 0060's kill switch, which is
        // only "immediate" if the carriage goes with the durable row.
        transport.deregister("added-at-runtime");
        let PeerForward { response, .. } = forward(Arc::clone(&transport)).await;
        match response {
            PacketResponse::Reject(reject) => {
                assert_eq!(reject.code.as_str(), "T01");
                assert!(reject.message.contains("added-at-runtime"));
            }
            other => panic!("expected T01 after deregistration, got {other:?}"),
        }
    }

    /// A peering whose endpoint selects no carriage this node dials --
    /// here a plaintext one on a node that did not opt in -- registers
    /// nothing, and the peer id stays unmapped. Its answer is §2.2's `T01`
    /// with the peer named, which is what an unmapped id already falls
    /// through to; a stale mapping would instead dial nowhere.
    #[tokio::test]
    async fn a_peering_whose_scheme_selects_no_carriage_registers_nothing() {
        let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
        key_file.write_all(&[7u8; 32]).expect("write key file");
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let mut config_file = tempfile::NamedTempFile::new().expect("temp config file");
        write!(
            config_file,
            r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"
"#,
            state_dir = state_dir.path().display(),
            key_file = key_file.path().display(),
        )
        .expect("write config file");
        let config = Config::load(config_file.path()).expect("load a node with no peering");
        assert!(!config.peer_allow_plaintext_endpoints());

        let transport = build_peer_transport(&config, [0u8; 20], None, Arc::new(SystemClock));
        transport.register("plaintext", &runtime_peering("http://127.0.0.1:1/ilp"));

        let PeerForward {
            response,
            reached_peer,
            ..
        } = transport
            .forward("plaintext", prepare("g.example.app"), None)
            .await;
        assert!(t01(&response), "{response:?}");
        assert!(!reached_peer);
    }
}
