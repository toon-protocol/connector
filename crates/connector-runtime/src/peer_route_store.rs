//! Durable storage for issue #884's runtime-mutable peer/route table:
//! peer rows and peer-forwarding route rows added, updated or removed at
//! runtime over the operator surface (`Connector::upsert_runtime_peer`,
//! `Connector::upsert_runtime_peer_route` and their `remove_*` twins),
//! surviving a restart -- unlike a leased route (issue #427, ADR 0006),
//! which is deliberately memory-only and lapses on a TTL instead of being
//! written down.
//!
//! A whole-table JSON snapshot, not an append-only log like
//! [`crate::Journal`] or `OutboundClientLedger`: this table supports
//! removal, which an append-only log cannot express without a compaction
//! pass it has no use for anywhere else. Every write here is on the rare,
//! operator-initiated path (ADR 0015's cold-path exception, not the
//! per-packet one), so the O(n) whole-table rewrite this costs is paid by
//! nothing that runs per packet.
//!
//! Every write goes to a temp file beside `path` and is renamed over it --
//! `rename` is atomic on the same filesystem on every platform this node
//! ships on -- so a crash mid-write leaves the previous, still-valid
//! snapshot in place rather than a half-written file. An operator inspects
//! the current table at any time with `cat`/`jq` directly on `path`.
//!
//! # What a peer row holds, and why it holds it
//!
//! Until ADR 0058 a runtime peer row was *a name*: an id a route might
//! legally reference, with no endpoint, no carriage and no channel
//! binding. That is what made the row hollow -- a peering could not be
//! added to a running node at all, because the four config tables a
//! peering needs could only be edited with the process stopped.
//!
//! [`RuntimePeering`] is that row grown into the thing ADR 0034's rules
//! were always about: the endpoint to reach the counterparty on, the edge
//! identity a payload is sealed to, the operator's own fee and cap, and
//! the payment channel this peering's claims are judged against. Every one
//! of those but the fee and the cap is read from the counterparty's own
//! self-description (ADR 0050) or derived from it (ADR 0059); the fee and
//! the cap are the operator's policy and no document can supply them
//! (ADR 0006, ADR 0049, ADR 0061).
//!
//! **Nothing here is pinned, verified or attested.** The identity in a row
//! is whatever the URL the operator named served at the moment they named
//! it -- trust-on-first-use, which ADR 0058 states plainly and declines to
//! strengthen.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use connector_config::PeerCarriage;
use connector_domain::Price;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use crate::route::PeerRoute;

#[derive(Debug, Error)]
pub enum PeerRouteStoreError {
    #[error("runtime peer/route table I/O error at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("corrupt runtime peer/route table at {path}: {source}")]
    Corrupt {
        path: PathBuf,
        source: serde_json::Error,
    },
}

/// One runtime peering's binding to one payment channel, by chain.
///
/// The runtime twin of a `[[peer_channels]]` row plus its `[[pay_channels]]`
/// counterpart: the same channel holds both roles with one hop -- the peer
/// role for what arrives, the client role for what this node sends -- which
/// is the deployed shape `connector_config::pay_channel`'s own header
/// describes.
///
/// `counterparty_key` is **the counterparty's settlement address on this
/// entry's own chain**, and is what the channel was derived from (ADR
/// 0059). It is never the peer's edge identity: `TokenNetwork` recovers a
/// balance proof's signer and requires it to *be* a channel participant, so
/// a secp256k1 edge key in this field would name a participant no chain
/// holds. On Solana the two could not even be confused -- an ed25519 public
/// key and a secp256k1 one are different values on different curves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "chain", rename_all = "lowercase")]
pub enum RuntimePeerChannel {
    Evm {
        /// The `TokenNetwork` channel id, `0x`-prefixed lowercase hex.
        channel_id: String,
        /// The peer's 20-byte EVM settlement address, `0x`-prefixed hex.
        counterparty_key: String,
        /// Half of the EIP-712 domain this channel's claims are signed
        /// under (ADR 0024).
        chain_id: u64,
        /// The other half: the `TokenNetwork` that verifies a claim on
        /// redemption, `0x`-prefixed hex.
        token_network: String,
    },
    Solana {
        /// The channel PDA, base58.
        channel_account: String,
        /// The peer's 32-byte ed25519 settlement public key, base58.
        counterparty_key: String,
        /// The deployed `payment-channel` program a claim on this channel
        /// binds its domain to (ADR 0053), base58.
        program_id: String,
    },
}

impl RuntimePeerChannel {
    /// How a claim on this channel names it on the wire -- an EVM
    /// `channelId` or a Solana `channelAccount`. The one spelling every
    /// cross-table check compares by, matching
    /// `connector_config::PayChannelConfig::channel`.
    #[must_use]
    pub fn channel(&self) -> &str {
        match self {
            RuntimePeerChannel::Evm { channel_id, .. } => channel_id,
            RuntimePeerChannel::Solana {
                channel_account, ..
            } => channel_account,
        }
    }

    /// The counterparty's settlement address on this entry's chain, in
    /// that chain's own spelling.
    #[must_use]
    pub fn counterparty_key(&self) -> &str {
        match self {
            RuntimePeerChannel::Evm {
                counterparty_key, ..
            }
            | RuntimePeerChannel::Solana {
                counterparty_key, ..
            } => counterparty_key,
        }
    }
}

/// A peering held in the runtime table: everything ADR 0058's one operator
/// write establishes.
///
/// `endpoint` and `edge_identity` come from the counterparty's
/// self-description; `channels` are derived from the two settlement
/// addresses and read (or opened) on chain; `fee` and `max_packet_amount`
/// are the operator's.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimePeering {
    /// ADR 0061's flat per-packet fee: what this connector retains for
    /// carrying one packet to this counterparty, whichever prefix it was
    /// addressed to.
    pub fee: u64,
    /// ADR 0049's cap: the largest amount this connector will forward to
    /// this counterparty in one packet. Zero means "this row states none",
    /// and the peering keeps `DEFAULT_MAX_PACKET_AMOUNT` -- there is no
    /// call anywhere that removes a bound.
    pub max_packet_amount: u64,
    /// Where this connector dials the counterparty, from its
    /// self-description's `btpEndpoint` or `httpEndpoint`. `None` for a
    /// peering that predates ADR 0058 or for one this node only ever
    /// accepts on.
    pub endpoint: Option<String>,
    /// The counterparty's edge identity -- the secp256k1 key a payload is
    /// sealed to (ADR 0018) -- as the `0x`-prefixed uncompressed public
    /// key its self-description published. **Not** a settlement address
    /// and never usable as one.
    pub edge_identity: Option<String>,
    /// The counterparty's own client edge (`POST /ilp`), read from its
    /// self-description's `httpEndpoint`. What
    /// `POST /ilp/claim-state` is asked on when this node covers a packet
    /// it forwards to this peering.
    pub client_edge_url: Option<String>,
    /// The payment channels this peering's claims are judged against, one
    /// per chain the two nodes share. A peering with none is refused at
    /// write time (the runtime twin of `ConfigError::PeerChannelUnbound`).
    pub channels: Vec<RuntimePeerChannel>,
}

impl RuntimePeering {
    /// Which carriage this connector dials this peering on, decided
    /// **solely** by the endpoint (`peer-carriage-spec.md` §2.1) -- the
    /// same rule `connector_config::PeerConfig::dial` applies to a
    /// config-file peering, read from the same place rather than restated.
    /// That includes ADR 0070's host clause: a `.onion` endpoint decides
    /// its carriage here exactly as it does in the config file, because
    /// both ask `PeerCarriage::for_endpoint`.
    ///
    /// `None` for a peering with no endpoint, and for one whose endpoint
    /// names a scheme this node will not dial.
    #[must_use]
    pub fn dial(&self, allow_plaintext: bool) -> Option<PeerCarriage> {
        let url = self.endpoint_url()?;
        PeerCarriage::for_endpoint(&url, allow_plaintext)
    }

    /// This peering's endpoint as a parsed URL, or `None` when it has none
    /// (or holds something that is not a URL, which only a hand-edited
    /// snapshot can produce).
    #[must_use]
    pub fn endpoint_url(&self) -> Option<Url> {
        Url::parse(self.endpoint.as_deref()?).ok()
    }
}

/// The on-disk shapes a stored peer has ever had, all folded into what a
/// peer row is now.
///
/// Issue #884 wrote `peers` as a bare `Vec<String>`; #886 grew each entry
/// into a struct so it could carry a peer-sale lease's `expires_at`; ADR
/// 0043 removed purchasable peering and with it the lease, so that field is
/// read and dropped rather than refused; ADR 0061 gave the row a `fee`; ADR
/// 0058 gave it the rest of a peering. `state_dir` is a persistent volume
/// that outlives image upgrades (deploy/connector-rust/README.md) and a
/// snapshot format has no version field to migrate on, so a box holding any
/// older shape must still boot: refusing to parse one is a crash loop with
/// no migration path.
///
/// A row from an older shape replays with no endpoint and no channel --
/// which is exactly what it was, since the table could hold neither. Such
/// a row is still readable and still removable; what it cannot do is be
/// *written* again, because [`RuntimePeering::channels`] being empty is
/// refused at write time now.
#[derive(Deserialize)]
#[serde(untagged)]
enum StoredPeerCompat {
    Bare(String),
    Full {
        id: String,
        /// A removed peer-sale lease (ADR 0038, removed by ADR 0043).
        /// Parsed so an older snapshot still opens, then discarded: a
        /// runtime peer row has no expiry of any kind again.
        #[serde(default)]
        #[allow(dead_code)]
        expires_at: Option<DateTime<Utc>>,
        /// ADR 0061's fee, absent from every snapshot written before it.
        #[serde(default)]
        fee: Option<u64>,
        /// Everything ADR 0058 added, absent from every snapshot written
        /// before it.
        #[serde(default)]
        max_packet_amount: Option<u64>,
        #[serde(default)]
        endpoint: Option<String>,
        #[serde(default)]
        edge_identity: Option<String>,
        #[serde(default)]
        client_edge_url: Option<String>,
        #[serde(default)]
        channels: Vec<RuntimePeerChannel>,
    },
}

/// One row as it comes back out of the compat layer: an id and the
/// peering it names, whichever of the four on-disk shapes it was written
/// in.
struct ReadPeer {
    id: String,
    peering: RuntimePeering,
}

impl From<StoredPeerCompat> for ReadPeer {
    fn from(compat: StoredPeerCompat) -> ReadPeer {
        match compat {
            StoredPeerCompat::Bare(id) => ReadPeer {
                id,
                peering: RuntimePeering::default(),
            },
            StoredPeerCompat::Full {
                id,
                fee,
                max_packet_amount,
                endpoint,
                edge_identity,
                client_edge_url,
                channels,
                ..
            } => ReadPeer {
                id,
                peering: RuntimePeering {
                    fee: fee.unwrap_or(0),
                    max_packet_amount: max_packet_amount.unwrap_or(0),
                    endpoint,
                    edge_identity,
                    client_edge_url,
                    channels,
                },
            },
        }
    }
}

/// A peer row as written: the id, and the peering it names.
///
/// Flattened rather than nested so the JSON an operator reads with `jq` is
/// one object per peering, and so the compat layer above can keep reading
/// the shapes that had no nesting either.
#[derive(Debug, Clone, Serialize)]
struct StoredPeer {
    id: String,
    #[serde(flatten)]
    peering: StoredPeeringFields,
}

/// The wire form of [`RuntimePeering`], with every field that a
/// still-default peering would write as noise skipped.
#[derive(Debug, Clone, Serialize)]
struct StoredPeeringFields {
    fee: u64,
    max_packet_amount: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    edge_identity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_edge_url: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    channels: Vec<RuntimePeerChannel>,
}

impl From<RuntimePeering> for StoredPeeringFields {
    fn from(peering: RuntimePeering) -> StoredPeeringFields {
        StoredPeeringFields {
            fee: peering.fee,
            max_packet_amount: peering.max_packet_amount,
            endpoint: peering.endpoint,
            edge_identity: peering.edge_identity,
            client_edge_url: peering.client_edge_url,
            channels: peering.channels,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredRoute {
    prefix: String,
    peer_id: String,
    /// A route's fee, moved to the peer row by ADR 0061. Parsed so a
    /// snapshot written before that still opens, then discarded -- the
    /// same read-and-drop the removed peer-sale lease above gets, and for
    /// the same reason: a `state_dir` outliving an image upgrade must not
    /// crash-loop on its own durable table. Never written back: a rewritten
    /// snapshot drops the key for good, the way the removed lease is
    /// dropped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)]
    fee: Option<u64>,
    /// The schedule this route charges (ADR 0065). A snapshot written
    /// before schedules existed carries a bare integer here, which is what
    /// a flat [`Price`] both reads and writes -- so an image upgrade opens
    /// an old table unchanged, and a downgrade opens any table whose routes
    /// are all flat.
    price: Price,
}

#[derive(Debug, Default, Serialize)]
struct Snapshot {
    peers: Vec<StoredPeer>,
    routes: Vec<StoredRoute>,
}

/// The read half of [`Snapshot`], which goes through the compat layer.
#[derive(Default, Deserialize)]
struct StoredSnapshot {
    #[serde(default)]
    peers: Vec<StoredPeerCompat>,
    #[serde(default)]
    routes: Vec<StoredRoute>,
}

/// The runtime peerings this node holds (issue #884, ADR 0058), each id
/// mapped to the peering it names -- endpoint, edge identity, channel
/// bindings, fee and cap.
pub type RuntimePeers = HashMap<String, RuntimePeering>;

/// What opening a [`PeerRouteStore`] replays: the store itself, plus
/// whatever peerings and peer-forwarding routes its file already held
/// (empty for a fresh file).
pub type PeerRouteTable = (PeerRouteStore, RuntimePeers, HashMap<String, PeerRoute>);

/// The `state_dir`-scoped file backing issue #884's runtime peer/route
/// table. Opening one replays whatever it already held (empty for a fresh
/// file, matching every other `state_dir`-scoped store's "no state yet"
/// degrade); `persist` overwrites it wholesale with the table's current
/// contents.
#[derive(Debug)]
pub struct PeerRouteStore {
    path: PathBuf,
}

impl PeerRouteStore {
    /// Open (or, if absent, note the future location of) the table file at
    /// `path`, returning the store alongside whatever peerings and routes
    /// it already held.
    pub fn open(path: &Path) -> Result<PeerRouteTable, PeerRouteStoreError> {
        let store = PeerRouteStore {
            path: path.to_path_buf(),
        };
        if !path.exists() {
            return Ok((store, RuntimePeers::new(), HashMap::new()));
        }
        let text = fs::read_to_string(path).map_err(|source| PeerRouteStoreError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if text.trim().is_empty() {
            return Ok((store, RuntimePeers::new(), HashMap::new()));
        }
        let snapshot: StoredSnapshot =
            serde_json::from_str(&text).map_err(|source| PeerRouteStoreError::Corrupt {
                path: path.to_path_buf(),
                source,
            })?;
        let peers = snapshot
            .peers
            .into_iter()
            .map(|peer| {
                let peer = ReadPeer::from(peer);
                (peer.id, peer.peering)
            })
            .collect();
        let routes = snapshot
            .routes
            .into_iter()
            .map(|route| {
                (
                    route.prefix.clone(),
                    PeerRoute::new_scheduled(route.prefix, route.peer_id, route.price),
                )
            })
            .collect();
        Ok((store, peers, routes))
    }

    /// Overwrite this store's file with `peers` and `routes` in full.
    /// Sorted before serializing so two writes of the same logical table
    /// produce byte-identical output -- an operator diffing the file
    /// across two mutations sees only the actual change, not a HashMap's
    /// unspecified iteration order.
    pub fn persist(
        &self,
        peers: &RuntimePeers,
        routes: &HashMap<String, PeerRoute>,
    ) -> Result<(), PeerRouteStoreError> {
        let mut stored_peers: Vec<StoredPeer> = peers
            .iter()
            .map(|(id, peering)| StoredPeer {
                id: id.clone(),
                peering: peering.clone().into(),
            })
            .collect();
        stored_peers.sort_by(|a, b| a.id.cmp(&b.id));
        let mut stored_routes: Vec<StoredRoute> = routes
            .values()
            .map(|route| StoredRoute {
                prefix: route.prefix().to_string(),
                peer_id: route.peer_id().to_string(),
                fee: None,
                price: route.price(),
            })
            .collect();
        stored_routes.sort_by(|a, b| a.prefix.cmp(&b.prefix));
        let snapshot = Snapshot {
            peers: stored_peers,
            routes: stored_routes,
        };
        let text = serde_json::to_string_pretty(&snapshot)
            .expect("a runtime peer/route snapshot always serializes to JSON");

        let tmp_path = self.path.with_extension("json.tmp");
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|source| PeerRouteStoreError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let mut file = fs::File::create(&tmp_path).map_err(|source| PeerRouteStoreError::Io {
            path: tmp_path.clone(),
            source,
        })?;
        file.write_all(text.as_bytes())
            .map_err(|source| PeerRouteStoreError::Io {
                path: tmp_path.clone(),
                source,
            })?;
        file.sync_all().map_err(|source| PeerRouteStoreError::Io {
            path: tmp_path.clone(),
            source,
        })?;
        fs::rename(&tmp_path, &self.path).map_err(|source| PeerRouteStoreError::Io {
            path: self.path.clone(),
            source,
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peering(fee: u64) -> RuntimePeering {
        RuntimePeering {
            fee,
            max_packet_amount: 5_000,
            endpoint: Some("https://peer.example/ilp".to_string()),
            edge_identity: Some("0x04ab".to_string()),
            client_edge_url: Some("https://peer.example/ilp".to_string()),
            channels: vec![RuntimePeerChannel::Evm {
                channel_id: format!("0x{}", "ab".repeat(32)),
                counterparty_key: "0x00000000000000000000000000000000000000aa".to_string(),
                chain_id: 31337,
                token_network: "0x00000000000000000000000000000000000000bb".to_string(),
            }],
        }
    }

    #[test]
    fn opening_a_path_that_does_not_exist_yet_yields_an_empty_table() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("runtime_peers.json");

        let (_store, peers, routes) = PeerRouteStore::open(&path).expect("open");
        assert!(peers.is_empty());
        assert!(routes.is_empty());
    }

    #[test]
    fn a_persisted_table_is_read_back_identically() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("runtime_peers.json");
        let (store, _, _) = PeerRouteStore::open(&path).expect("open");

        let mut peers = RuntimePeers::new();
        peers.insert("apex-relay-2".to_string(), peering(3));
        let mut routes = HashMap::new();
        routes.insert(
            "g.example.relay2".to_string(),
            PeerRoute::new_priced("g.example.relay2", "apex-relay-2", 25),
        );
        store.persist(&peers, &routes).expect("persist");

        let (_store, read_peers, read_routes) = PeerRouteStore::open(&path).expect("re-open");
        assert_eq!(read_peers, peers);
        assert_eq!(read_routes, routes);
    }

    /// ADR 0058: the endpoint, the edge identity and the channel binding
    /// survive the restart they are written for. Without the endpoint the
    /// node comes back unable to dial the counterparty it was peered with;
    /// without the channel it comes back unable to judge a claim from one.
    #[test]
    fn a_peerings_endpoint_identity_and_channel_survive_a_restart() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("runtime_peers.json");
        let (store, _, _) = PeerRouteStore::open(&path).expect("open");

        let mut peers = RuntimePeers::new();
        peers.insert("apex-relay-2".to_string(), peering(100));
        store.persist(&peers, &HashMap::new()).expect("persist");

        let (_store, read_peers, _) = PeerRouteStore::open(&path).expect("re-open");
        let read = &read_peers["apex-relay-2"];
        assert_eq!(read.fee, 100);
        assert_eq!(read.max_packet_amount, 5_000);
        assert_eq!(
            read.endpoint.as_deref(),
            Some("https://peer.example/ilp"),
            "a peering with no endpoint is one this node cannot dial"
        );
        assert_eq!(read.edge_identity.as_deref(), Some("0x04ab"));
        assert_eq!(read.channels.len(), 1);
        assert_eq!(
            read.channels[0].channel(),
            &format!("0x{}", "ab".repeat(32))
        );
    }

    /// The carriage is the endpoint's scheme and nothing else -- read
    /// through `connector_config`'s own rule rather than restated here.
    #[test]
    fn the_carriage_is_the_endpoints_scheme() {
        let mut wss = peering(0);
        wss.endpoint = Some("wss://peer.example/ilp/btp".to_string());
        assert_eq!(wss.dial(false), Some(PeerCarriage::Btp));
        assert_eq!(peering(0).dial(false), Some(PeerCarriage::Http));

        let mut plaintext = peering(0);
        plaintext.endpoint = Some("http://127.0.0.1:4000/ilp".to_string());
        assert_eq!(
            plaintext.dial(false),
            None,
            "a plaintext endpoint selects no carriage unless the node opted in"
        );
        assert_eq!(plaintext.dial(true), Some(PeerCarriage::Http));

        let accept_only = RuntimePeering::default();
        assert_eq!(accept_only.dial(true), None);
    }

    /// The exact bytes issue #884's format wrote -- `peers` as bare
    /// strings, and a `fee` on each route -- still replay: `state_dir`
    /// outlives image upgrades, so a node that added a runtime peer before
    /// #886 grew each entry into a struct boots with this very file on
    /// disk, and refusing to parse it is a crash loop with no migration
    /// path. The route's `fee` is read and dropped (ADR 0061); the peering
    /// it belonged to replays at zero, with no endpoint and no channel --
    /// which is precisely what it was.
    #[test]
    fn a_bare_string_peer_snapshot_still_replays() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("runtime_peers.json");
        fs::write(
            &path,
            r#"{"peers":["apex-relay-2"],"routes":[{"prefix":"g.example.relay2","peer_id":"apex-relay-2","fee":3,"price":25}]}"#,
        )
        .expect("write the #884-format file");

        let (store, peers, routes) = PeerRouteStore::open(&path).expect("the old format parses");
        assert_eq!(peers["apex-relay-2"], RuntimePeering::default());
        assert_eq!(routes.len(), 1);
        assert_eq!(routes["g.example.relay2"].price(), Price::flat(25));

        // And persisting rewrites it in the current form, which the next
        // open reads back identically.
        store.persist(&peers, &routes).expect("persist");
        let (_store, read_peers, read_routes) = PeerRouteStore::open(&path).expect("re-open");
        assert_eq!(read_peers, peers);
        assert_eq!(read_routes, routes);
    }

    /// A snapshot written while purchasable peering still existed (ADR
    /// 0038's `expires_at` on a peer row, removed by ADR 0043) still opens:
    /// the field is read and dropped, never refused. The alternative is a
    /// box whose `state_dir` outlived the image upgrade crash-looping on
    /// its own durable table.
    #[test]
    fn a_snapshot_carrying_a_removed_lease_field_still_replays() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("runtime_peers.json");
        fs::write(
            &path,
            r#"{"peers":[{"id":"evm:1","expires_at":"2030-01-01T00:00:00Z"},{"id":"apex-relay-2"}],"routes":[]}"#,
        )
        .expect("write the #886-format file");

        let (store, peers, _) = PeerRouteStore::open(&path).expect("the lease format parses");
        assert!(peers.contains_key("evm:1"));
        assert!(peers.contains_key("apex-relay-2"));

        // And rewriting drops the field for good, rather than carrying a
        // lease nothing reads any more.
        store.persist(&peers, &HashMap::new()).expect("persist");
        let rewritten = fs::read_to_string(&path).expect("read back");
        assert!(
            !rewritten.contains("expires_at"),
            "a rewritten snapshot must not carry the removed lease: {rewritten}"
        );
    }

    /// A snapshot written by the ADR 0061 shape -- an id and a fee, and
    /// nothing else a peering needs -- still replays, at its own fee.
    #[test]
    fn a_fee_only_snapshot_still_replays_at_that_fee() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("runtime_peers.json");
        fs::write(
            &path,
            r#"{"peers":[{"id":"apex-relay-2","fee":100}],"routes":[]}"#,
        )
        .expect("write the ADR 0061-format file");

        let (_store, peers, _) = PeerRouteStore::open(&path).expect("the fee format parses");
        assert_eq!(peers["apex-relay-2"].fee, 100);
        assert!(peers["apex-relay-2"].channels.is_empty());
    }

    #[test]
    fn persisting_again_overwrites_rather_than_appends() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("runtime_peers.json");
        let (store, _, _) = PeerRouteStore::open(&path).expect("open");

        let mut peers = RuntimePeers::new();
        peers.insert("peer-a".to_string(), peering(0));
        store
            .persist(&peers, &HashMap::new())
            .expect("first persist");

        peers.remove("peer-a");
        peers.insert("peer-b".to_string(), peering(0));
        store
            .persist(&peers, &HashMap::new())
            .expect("second persist");

        let (_store, read_peers, _) = PeerRouteStore::open(&path).expect("re-open");
        assert_eq!(read_peers, peers);
        assert!(!read_peers.contains_key("peer-a"));
    }

    #[test]
    fn an_empty_file_reads_back_as_an_empty_table() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("runtime_peers.json");
        fs::write(&path, "").expect("write empty file");

        let (_store, peers, routes) = PeerRouteStore::open(&path).expect("open");
        assert!(peers.is_empty());
        assert!(routes.is_empty());
    }

    /// A Solana peering round-trips through its own chain shape: a channel
    /// account and a program id, never an EVM channel id and a chain id.
    #[test]
    fn a_solana_binding_round_trips_in_its_own_shape() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("runtime_peers.json");
        let (store, _, _) = PeerRouteStore::open(&path).expect("open");

        let solana = RuntimePeerChannel::Solana {
            channel_account: "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi".to_string(),
            counterparty_key: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
            program_id: "Toon11111111111111111111111111111111111111".to_string(),
        };
        let mut peers = RuntimePeers::new();
        peers.insert(
            "solana-hop".to_string(),
            RuntimePeering {
                channels: vec![solana.clone()],
                ..RuntimePeering::default()
            },
        );
        store.persist(&peers, &HashMap::new()).expect("persist");

        let (_store, read_peers, _) = PeerRouteStore::open(&path).expect("re-open");
        assert_eq!(read_peers["solana-hop"].channels, vec![solana]);
    }

    #[test]
    fn corrupt_json_is_a_named_error_not_a_silent_empty_table() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("runtime_peers.json");
        fs::write(&path, "{not json").expect("write garbage");

        let error = PeerRouteStore::open(&path).expect_err("garbage must not open");
        assert!(matches!(error, PeerRouteStoreError::Corrupt { .. }));
    }
}
