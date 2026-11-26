//! [`Peerings`]: how a peering comes to exist (spec #1686, ticket #1693).
//!
//! A peering is several pieces of state that must agree: the **voucher hop**
//! every forward to the peer is paid on (ADR 0075 decision 6), the **voucher
//! signer binding** that makes the peer's inbound channel the peer's (decision
//! 4), the **carriage** the node dials it over (ADR 0058), the **runtime peer
//! table row** and its **durable store**, and the peering's **cap** and
//! **fee**. This module owns all of them; [`crate::Connector`] holds one
//! [`Peerings`] and delegates to it.
//!
//! # One way in, one way out
//!
//! * [`Peerings::admit`] runs the peering-admission rule set
//!   (`connector_domain::peering_admission`, #1691 and #1692), then wires, in
//!   this order every time: voucher hop, signer binding, carriage, table
//!   row, durable store. If a step fails it undoes what the earlier steps
//!   wired, so a peering is never left half-wired. `POST /peers` is its
//!   caller today; boot replay (#1694) and the config file (#1695) call the
//!   same operation.
//! * [`Peerings::remove`] undoes exactly what admit set up.
//!
//! What a refusal does is the caller's choice (`POST /peers` answers with an
//! error); what counts as one is not.
//!
//! # The table row, then the store
//!
//! The row and its store entry are one write under the table lock: the new
//! table is persisted, and only then published, so the durable copy and the
//! running one can never disagree about which write is current. A failure of
//! the store therefore leaves no row either.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;
use connector_config::{SettlementChain, DEFAULT_MAX_PACKET_AMOUNT};
use connector_domain::peering_admission::{
    self, ChannelFact, PeerRouteFacts, PeeringFacts, PeeringSet,
};
use connector_domain::{is_valid_ilp_address, Price};
use thiserror::Error;

use crate::connector::PeerRouteTableError;
use crate::operator_view::{PeerRouteView, PeerView, RouteSource};
use crate::outbound_voucher::{HttpVoucherState, UnreachableVoucherState, VoucherStateSource};
use crate::peer_route_store::{PeerRouteStore, RuntimePeerChannel, RuntimePeering, RuntimePeers};
use crate::peer_transport::PeerRegistrar;
use crate::route::PeerRoute;
use crate::voucher_binding::{VoucherBindingError, VoucherSigner, VoucherSignerBindings};

/// One next hop this connector pays over its own outbound x402 channel
/// (ADR 0075 decisions 4 and 6): a runtime EVM peering.
#[derive(Clone)]
pub(crate) struct VoucherHop {
    /// This node's outbound channel toward the hop, as
    /// [`crate::OutboundChannels`] keys it.
    pub(crate) channel_id: String,
    /// The hop's own `POST /ilp/claim-state`: the watermark authority on
    /// restore (decision 6).
    pub(crate) claim_state: Arc<dyn VoucherStateSource>,
    /// Whether this process has asked `claim_state` where the channel
    /// stands since it last had cause to -- once per process, and again
    /// after a voucher the hop did not accept.
    pub(crate) synced: Arc<AtomicBool>,
    /// The highest voucher amount a forward rode and was then rejected on
    /// since the hop last answered `claim_state`, or 0 for none (issue
    /// #1446): the figure the next sync may lower the signed watermark
    /// from, and only while nothing later has been signed above it.
    pub(crate) rejected_through: Arc<Mutex<u128>>,
    /// Serialises reading the signed watermark and signing above it, so two
    /// concurrent forwards never both sign the same next amount.
    pub(crate) signing: Arc<tokio::sync::Mutex<()>>,
}

/// Why a peering was not admitted.
#[derive(Debug, Error)]
pub(crate) enum AdmitError {
    /// A rule of the peering-admission check, or the durable write.
    #[error(transparent)]
    Table(#[from] PeerRouteTableError),
    /// The peer's voucher signer could not be bound: it already proves
    /// another peering (one signer, one relation).
    #[error(transparent)]
    Binding(#[from] VoucherBindingError),
}

/// The state of every peering this node holds, and the one way a new one
/// comes to exist. See the module docs.
pub(crate) struct Peerings {
    /// Peer ids the config file names, reserved so a runtime write can never
    /// add, change or remove one (ADR 0034).
    config_peer_ids: HashSet<String>,
    /// ADR 0042's per-packet cap, by peer id, from the config file's rows.
    /// A peer with no entry is capped at `DEFAULT_MAX_PACKET_AMOUNT`.
    packet_caps: HashMap<String, u64>,
    /// ADR 0010's flat per-packet fee, by peer id, from the config file's
    /// rows. A peer with no entry charges nothing.
    fees: HashMap<String, u64>,
    /// Adds and removes a dial carriage while this process serves (ADR 0058).
    registrar: Option<Arc<dyn PeerRegistrar>>,
    /// The node's one root-level `socks_proxy` (ADR 0070): what a runtime
    /// peering's claim-state ask leaves on when the peer's client edge is an
    /// onion host.
    socks_proxy: Option<url::Url>,
    /// The next hops this node pays over its own outbound x402 channel,
    /// keyed by peer id: a runtime peering, or a `[[pay_channels]]` row.
    /// Copy-on-write: an operator write registers and removes one while the
    /// packet path reads it lock-free.
    hops: ArcSwap<HashMap<String, VoucherHop>>,
    /// The voucher signers bound to a peering (ADR 0075 decision 4).
    voucher_bindings: VoucherSignerBindings,
    /// Peers added at runtime over the operator surface (issue #884).
    runtime_peers: ArcSwap<RuntimePeers>,
    /// Peer-forwarding routes added at runtime (issue #884).
    runtime_peer_routes: ArcSwap<HashMap<String, PeerRoute>>,
    /// Serializes every write of the runtime table (never taken by a read),
    /// so persisting and publishing happen exactly once per write.
    table_lock: Mutex<()>,
    /// Where the runtime table is written durably. `None` on a node with no
    /// `state_dir`: still mutable, not durable.
    store: Option<PeerRouteStore>,
}

/// What admit captured before it wired anything, to undo a failed write.
struct Prior {
    hop: Option<VoucherHop>,
    signers: Vec<VoucherSigner>,
    peering: Option<RuntimePeering>,
}

impl Peerings {
    pub(crate) fn new() -> Self {
        Self {
            config_peer_ids: HashSet::new(),
            packet_caps: HashMap::new(),
            fees: HashMap::new(),
            registrar: None,
            socks_proxy: None,
            hops: ArcSwap::from_pointee(HashMap::new()),
            voucher_bindings: VoucherSignerBindings::new(),
            runtime_peers: ArcSwap::from_pointee(RuntimePeers::new()),
            runtime_peer_routes: ArcSwap::from_pointee(HashMap::new()),
            table_lock: Mutex::new(()),
            store: None,
        }
    }

    // -- configuration, set once while the connector is built ---------------

    pub(crate) fn set_config_peer_ids(&mut self, ids: impl IntoIterator<Item = String>) {
        self.config_peer_ids = ids.into_iter().collect();
    }

    pub(crate) fn set_packet_caps(&mut self, caps: impl IntoIterator<Item = (String, u64)>) {
        self.packet_caps = caps.into_iter().collect();
    }

    pub(crate) fn set_fees(&mut self, fees: impl IntoIterator<Item = (String, u64)>) {
        self.fees = fees.into_iter().collect();
    }

    pub(crate) fn set_registrar(&mut self, registrar: Arc<dyn PeerRegistrar>) {
        self.registrar = Some(registrar);
    }

    pub(crate) fn set_socks_proxy(&mut self, proxy: Option<url::Url>) {
        self.socks_proxy = proxy;
    }

    // -- reads ---------------------------------------------------------------

    pub(crate) fn config_peer_ids(&self) -> &HashSet<String> {
        &self.config_peer_ids
    }

    pub(crate) fn is_config_peer(&self, id: &str) -> bool {
        self.config_peer_ids.contains(id)
    }

    /// The current runtime-peer table, snapshotted as of this call (ADR
    /// 0015): a single atomic `Arc` clone, no lock.
    pub(crate) fn runtime_peers(&self) -> Arc<RuntimePeers> {
        self.runtime_peers.load_full()
    }

    pub(crate) fn runtime_peer_routes(&self) -> Arc<HashMap<String, PeerRoute>> {
        self.runtime_peer_routes.load_full()
    }

    pub(crate) fn runtime_peering(&self, id: &str) -> Option<RuntimePeering> {
        self.runtime_peers.load().get(id).cloned()
    }

    pub(crate) fn hop(&self, peer_id: &str) -> Option<VoucherHop> {
        self.hops.load().get(peer_id).cloned()
    }

    pub(crate) fn voucher_bindings(&self) -> &VoucherSignerBindings {
        &self.voucher_bindings
    }

    /// The most this connector will forward to `peer_id` in one packet: its
    /// configured cap, or the runtime row's, or the default (ADR 0042).
    ///
    /// A stated cap of zero is "this row states none", not "forward
    /// nothing": a peering silently capped at zero would go dark on a
    /// default rather than on a decision.
    pub(crate) fn packet_cap_for(&self, peer_id: &str) -> u64 {
        if let Some(cap) = self.packet_caps.get(peer_id) {
            return *cap;
        }
        match self.runtime_peers().get(peer_id) {
            Some(peering) => crate::peering::stated_cap(peering.max_packet_amount),
            None => DEFAULT_MAX_PACKET_AMOUNT,
        }
    }

    /// What this connector retains for carrying one packet to `peer_id`
    /// (ADR 0010, ADR 0061), zero for a peering that configured none. The
    /// runtime snapshot is loaded only on a config miss.
    pub(crate) fn fee_for(&self, peer_id: &str) -> u64 {
        match self.fees.get(peer_id) {
            Some(fee) => *fee,
            None => self
                .runtime_peers()
                .get(peer_id)
                .map_or(0, |peering| peering.fee),
        }
    }

    // -- admit ---------------------------------------------------------------

    /// Refuse a peering write that could never land, **before** any
    /// outbound request is made: an empty id, or one the config file owns
    /// (ADR 0034).
    pub(crate) fn refuse_unlandable(&self, id: &str) -> Result<(), PeerRouteTableError> {
        if id.trim().is_empty() {
            return Err(PeerRouteTableError::InvalidPeerId);
        }
        self.refuse_config_owned_peer_id(id)
    }

    /// ADR 0034 for a write naming peer `id` alone.
    fn refuse_config_owned_peer_id(&self, id: &str) -> Result<(), PeerRouteTableError> {
        peering_admission::check_config_ownership(&PeeringSet {
            peerings: vec![Self::runtime_peering_facts(id)],
            config_peer_ids: self.config_peer_ids.iter().cloned().collect(),
            ..PeeringSet::default()
        })?;
        Ok(())
    }

    /// A peering written at runtime, as `peering_admission` sees it.
    fn runtime_peering_facts(id: &str) -> PeeringFacts {
        PeeringFacts {
            source: peering_admission::Source::Runtime,
            ..PeeringFacts::new(id)
        }
    }

    /// The peering-admission check for a runtime peering: an id, not owned
    /// by the config file (ADR 0034), with a channel bound (ADR 0058).
    pub(crate) fn check(
        &self,
        id: &str,
        peering: &RuntimePeering,
    ) -> Result<(), PeerRouteTableError> {
        if id.trim().is_empty() {
            return Err(PeerRouteTableError::InvalidPeerId);
        }
        peering_admission::admit(&PeeringSet {
            peerings: vec![Self::runtime_peering_facts(id)],
            inbound: peering
                .channels
                .iter()
                .map(|_| ChannelFact::new(id))
                .collect(),
            config_peer_ids: self.config_peer_ids.iter().cloned().collect(),
            ..PeeringSet::default()
        })?;
        Ok(())
    }

    /// Admit `peering` as `id`: the one way a peering comes to exist.
    ///
    /// Runs the admission check, then wires the voucher hop, the signer
    /// binding, the carriage, the table row and the durable store, in that
    /// order every time. A step that fails undoes what the earlier ones
    /// wired, restoring whatever an earlier peering of the same id had, so
    /// the peering is never left half-wired.
    ///
    /// The hop is wired from `peering.client_edge_url` (a peering with none
    /// has no claim-state to restore a watermark from and is accept-only);
    /// the signer binding and the carriage come from the peering's own
    /// channels and endpoint.
    ///
    /// # Errors
    ///
    /// [`AdmitError::Table`] for a refused rule or a failed store write, and
    /// [`AdmitError::Binding`] for a voucher signer already proving another
    /// peering.
    pub(crate) fn admit(&self, id: &str, peering: RuntimePeering) -> Result<PeerView, AdmitError> {
        self.check(id, &peering)?;
        let prior = Prior {
            hop: self.hop(id),
            signers: self.voucher_bindings.signers_of(id),
            peering: self.runtime_peering(id),
        };

        // 1. The voucher hop every forward to the peer is covered on.
        if let Some(client_edge_url) = &peering.client_edge_url {
            for binding in &peering.channels {
                self.register_voucher_hop(id, binding, client_edge_url);
            }
        }
        // 2. The signer binding: what makes the peer's channel the peer's.
        for binding in &peering.channels {
            if let Err(error) = self.bind_runtime_voucher_signer(id, binding) {
                self.undo(id, &prior);
                return Err(error.into());
            }
        }
        // 3. The carriage.
        self.register_carriage(id, &peering);
        // 4 and 5. The table row and the store.
        match self.write_row(id, peering) {
            Ok(view) => Ok(view),
            Err(error) => {
                self.undo(id, &prior);
                Err(error.into())
            }
        }
    }

    /// Put back what [`Self::admit`] found: the hop, the signers and the
    /// carriage an earlier peering of this id had, or none.
    fn undo(&self, id: &str, prior: &Prior) {
        self.set_hop(id, prior.hop.clone());
        for signer in self.voucher_bindings.signers_of(id) {
            if !prior.signers.contains(&signer) {
                self.voucher_bindings.unbind_signer(id, &signer);
            }
        }
        match (&prior.peering, &self.registrar) {
            (Some(peering), Some(registrar)) => registrar.register(id, peering),
            (None, Some(registrar)) => registrar.deregister(id),
            _ => {}
        }
    }

    /// The table row and its durable store entry: one write, persisted
    /// before it is published. Never wires anything.
    pub(crate) fn write_row(
        &self,
        id: &str,
        peering: RuntimePeering,
    ) -> Result<PeerView, PeerRouteTableError> {
        let view = PeerView {
            id: id.to_string(),
            fee: peering.fee,
            max_packet_amount: crate::peering::stated_cap(peering.max_packet_amount),
            source: RouteSource::Runtime,
        };
        let _write_guard = self.lock_table();
        let mut peers = (*self.runtime_peers()).clone();
        peers.insert(id.to_string(), peering);
        self.persist(&peers, &self.runtime_peer_routes())?;
        self.runtime_peers.store(Arc::new(peers));
        Ok(view)
    }

    /// Make a runtime peering dialable, if this node holds a registrar.
    fn register_carriage(&self, peer_id: &str, peering: &RuntimePeering) {
        if let Some(registrar) = &self.registrar {
            registrar.register(peer_id, peering);
        }
    }

    // -- remove --------------------------------------------------------------

    /// Remove a runtime peering (`DELETE /peers/:id`), undoing exactly what
    /// [`Self::admit`] set up. Refused when `id` belongs to the config file,
    /// when no such runtime peer exists, or when a runtime route still
    /// forwards to it.
    pub(crate) fn remove(&self, id: &str) -> Result<(), PeerRouteTableError> {
        self.refuse_config_owned_peer_id(id)?;
        let _write_guard = self.lock_table();
        let peers = self.runtime_peers();
        if !peers.contains_key(id) {
            return Err(PeerRouteTableError::PeerNotFound(id.to_string()));
        }
        let routes = self.runtime_peer_routes();
        if routes.values().any(|route| route.peer_id() == id) {
            return Err(PeerRouteTableError::PeerInUse(id.to_string()));
        }
        let mut next_peers = (*peers).clone();
        next_peers.remove(id);
        self.persist(&next_peers, &routes)?;
        self.runtime_peers.store(Arc::new(next_peers));
        // The same kill switch covers the peer role an x402 peering proves
        // with its vouchers (ADR 0075 decision 5): a signer left bound would
        // keep deciding `peer` for a relation that no longer exists.
        self.voucher_bindings.unbind_peer(id);
        // ADR 0075 decision 4: removing a peering stops signing on its
        // outbound channel, which stays journaled and open for the operator
        // to withdraw (`POST /channels/:id/withdraw`).
        self.set_hop(id, None);
        // ADR 0060 named `DELETE /peers` as the kill switch: "immediate,
        // does not require a restart". It is only immediate if the carriage
        // goes with the row.
        if let Some(registrar) = &self.registrar {
            registrar.deregister(id);
        }
        Ok(())
    }

    // -- the runtime route table, written under the same lock ---------------

    /// Add or update a runtime peer-forwarding route. `config_prefixes` is
    /// every prefix the config file serves, which a runtime write may never
    /// take (ADR 0034).
    pub(crate) fn upsert_route(
        &self,
        prefix: String,
        peer_id: String,
        price: Price,
        request: Option<serde_json::Value>,
        config_prefixes: BTreeSet<String>,
    ) -> Result<PeerRouteView, PeerRouteTableError> {
        if !is_valid_ilp_address(&prefix) {
            return Err(PeerRouteTableError::InvalidPrefix(prefix));
        }
        let _write_guard = self.lock_table();
        // Every cross-table rule of a route -- config ownership of the prefix
        // (ADR 0034), the peer it names, a channel to pay that peer from --
        // is `connector_domain::peering_admission`'s, the same code
        // `Config::load` runs (issues #1691, #1692). A config peering was
        // admitted at load, so it is stated as the facts load established; a
        // runtime peering is stated as the runtime knows it, its paying hop.
        let peering = if self.is_config_peer(&peer_id) {
            Some((
                PeeringFacts::new(&peer_id),
                vec![ChannelFact::new(&peer_id)],
                vec![ChannelFact::new(&peer_id)],
            ))
        } else {
            self.runtime_peers().get(&peer_id).map(|_| {
                let pay = if self.hops.load().contains_key(&peer_id) {
                    vec![ChannelFact::new(&peer_id)]
                } else {
                    Vec::new()
                };
                (
                    Self::runtime_peering_facts(&peer_id),
                    // `admit` refused a peering with no binding when it was
                    // written, and this write is not that rule's to
                    // re-judge: a peering replayed from a pre-ADR-0058
                    // snapshot reaches the pay-channel rule.
                    vec![ChannelFact::new(&peer_id)],
                    pay,
                )
            })
        };
        let (peerings, inbound, pay) = match peering {
            Some((peering, inbound, pay)) => (vec![peering], inbound, pay),
            None => (Vec::new(), Vec::new(), Vec::new()),
        };
        peering_admission::admit(&PeeringSet {
            peerings,
            inbound,
            pay,
            routes: vec![PeerRouteFacts {
                source: peering_admission::Source::Runtime,
                ..PeerRouteFacts::new(&prefix, &peer_id)
            }],
            config_peer_ids: self.config_peer_ids.iter().cloned().collect(),
            config_prefixes,
            ..PeeringSet::default()
        })?;
        if request.as_ref().is_some_and(|value| !value.is_object()) {
            return Err(PeerRouteTableError::InvalidRequest(prefix));
        }
        let route = PeerRoute::new_scheduled(prefix.clone(), peer_id.clone(), price)
            .with_request(request.clone());
        let mut routes = (*self.runtime_peer_routes()).clone();
        routes.insert(prefix.clone(), route);
        self.persist(&self.runtime_peers(), &routes)?;
        self.runtime_peer_routes.store(Arc::new(routes));
        Ok(PeerRouteView {
            prefix,
            peer_id,
            price,
            source: RouteSource::Runtime,
            request,
        })
    }

    /// Remove a runtime peer-forwarding route. Refused when `prefix` is
    /// defined by the config file, or when no such runtime route exists.
    pub(crate) fn remove_route(
        &self,
        prefix: &str,
        config_prefixes: BTreeSet<String>,
    ) -> Result<(), PeerRouteTableError> {
        // ADR 0034 is `peering_admission`'s, the same code the route write
        // runs; the route's peer is not this rule's concern.
        peering_admission::check_config_ownership(&PeeringSet {
            routes: vec![PeerRouteFacts {
                source: peering_admission::Source::Runtime,
                ..PeerRouteFacts::new(prefix, "")
            }],
            config_prefixes,
            ..PeeringSet::default()
        })?;
        let _write_guard = self.lock_table();
        let mut routes = (*self.runtime_peer_routes()).clone();
        if routes.remove(prefix).is_none() {
            return Err(PeerRouteTableError::RouteNotFound(prefix.to_string()));
        }
        self.persist(&self.runtime_peers(), &routes)?;
        self.runtime_peer_routes.store(Arc::new(routes));
        Ok(())
    }

    // -- boot ----------------------------------------------------------------

    /// Replay a durable runtime table into this node and arm it to persist
    /// future writes: every stored peering goes through [`Self::admit`], the
    /// same operation `POST /peers` uses, so a replayed peering is wired
    /// exactly as establishing it wired it (spec #1686, ticket #1694).
    ///
    /// **A stored peering that fails is skipped whole.** One that admit
    /// refuses, or that fails while being wired (its voucher signer already
    /// proves another peering, say), leaves no hop, binding, carriage or
    /// table row, and the routes that name it are not replayed either. Each
    /// is logged at error level with the peering's id and the reason, and
    /// the node starts and serves its other peerings. Peerings replay in id
    /// order, so which of two peerings that claim one signer keeps it does
    /// not depend on the hash order of the stored table.
    ///
    /// Nothing is written back to the store while replaying: a skipped
    /// peering stays on disk until the table's next write, which persists
    /// the table as it now stands.
    ///
    /// Admit reads the config file's peer ids, the registrar and the socks
    /// proxy; the only caller, [`crate::PeeringWiring`], is reachable only
    /// once all three are set.
    pub(crate) fn replay(
        &mut self,
        store: PeerRouteStore,
        peers: RuntimePeers,
        routes: HashMap<String, PeerRoute>,
    ) {
        // Admit persists through `self.store`, which is not armed yet: the
        // table is rebuilt in memory, and the durable copy is not rewritten
        // half-replayed.
        self.store = None;
        let mut ids: Vec<&String> = peers.keys().collect();
        ids.sort();
        let mut skipped: BTreeSet<&str> = BTreeSet::new();
        for id in ids {
            if let Err(error) = self.admit(id, peers[id].clone()) {
                tracing::error!(
                    peer_id = %id,
                    %error,
                    "stored peering skipped: it was not admitted at boot, so none of it is \
                     wired and the routes naming it are dropped"
                );
                skipped.insert(id);
            }
        }
        let routes: HashMap<String, PeerRoute> = routes
            .into_iter()
            .filter(|(prefix, route)| {
                let kept = !skipped.contains(route.peer_id());
                if !kept {
                    tracing::error!(
                        prefix = %prefix,
                        peer_id = %route.peer_id(),
                        "stored route dropped: its peering was skipped at boot"
                    );
                }
                kept
            })
            .collect();
        self.runtime_peer_routes = ArcSwap::from_pointee(routes);
        self.store = Some(store);
    }

    // -- voucher hops and signer bindings -----------------------------------

    /// Register the paying hop for a runtime x402 peering's channel: its
    /// claim-state ask goes to `client_edge_url`. Any other binding
    /// registers nothing.
    ///
    /// The claim-state ask leaves on the node's `socks_proxy` when the
    /// peer's client edge is an onion host (ADR 0070), by the one host rule
    /// `connector_config::is_onion_endpoint`.
    pub(crate) fn register_voucher_hop(
        &self,
        peer_id: &str,
        binding: &RuntimePeerChannel,
        client_edge_url: &str,
    ) {
        let (RuntimePeerChannel::EvmVoucher {
            outbound_channel_id,
            ..
        }
        | RuntimePeerChannel::SolanaVoucher {
            outbound_channel_id,
            ..
        }
        | RuntimePeerChannel::XrplVoucher {
            outbound_channel_id,
            ..
        }) = binding
        else {
            return;
        };
        self.register_voucher_hop_at(
            peer_id,
            outbound_channel_id,
            client_edge_url,
            std::time::Duration::from_millis(connector_config::DEFAULT_PEER_TIMEOUT_MS),
        );
    }

    /// Pay `peer_id` on this node's outbound x402 channel `outbound_channel_id`,
    /// asking `client_edge_url`'s `POST /ilp/claim-state` where it stands --
    /// the one registration both a runtime peering and a `[[pay_channels]]`
    /// row make. `answer_timeout` bounds each claim-state ask.
    pub(crate) fn register_voucher_hop_at(
        &self,
        peer_id: &str,
        outbound_channel_id: &str,
        client_edge_url: &str,
        answer_timeout: std::time::Duration,
    ) {
        let builder = reqwest::Client::builder().timeout(answer_timeout);
        let onion = url::Url::parse(client_edge_url)
            .is_ok_and(|url| connector_config::is_onion_endpoint(&url));
        let client = if onion {
            // Fail closed: an onion name must never reach the local
            // resolver, so with no usable proxy the ask is refused by name
            // before any dial, and the journaled watermark stands.
            let proxied = match self.socks_proxy.as_ref() {
                None => Err(crate::peer_transport::NO_SOCKS_PROXY.to_string()),
                Some(proxy) => reqwest::Proxy::all(proxy.as_str())
                    .and_then(|socks| builder.proxy(socks).build())
                    .map_err(|error| format!("socks_proxy '{proxy}' could not be used: {error}")),
            };
            match proxied {
                Ok(client) => client,
                Err(reason) => {
                    self.insert_voucher_hop(
                        peer_id,
                        outbound_channel_id,
                        Arc::new(UnreachableVoucherState(format!(
                            "{client_edge_url} is an onion host: {reason}"
                        ))),
                    );
                    return;
                }
            }
        } else {
            builder
                .build()
                .expect("a reqwest client with only a timeout set always builds")
        };
        self.insert_voucher_hop(
            peer_id,
            outbound_channel_id,
            Arc::new(HttpVoucherState::new(client, client_edge_url)),
        );
    }

    /// Pay `peer_id` over this node's outbound x402 channel `channel_id`,
    /// asking `claim_state` where it stands (ADR 0075 decision 6).
    ///
    /// The id is kept as given: [`crate::OutboundChannels`] canonicalises an
    /// EVM id itself, and a Solana channel account is base58, which is
    /// case-sensitive.
    pub(crate) fn insert_voucher_hop(
        &self,
        peer_id: &str,
        channel_id: &str,
        claim_state: Arc<dyn VoucherStateSource>,
    ) {
        self.set_hop(
            peer_id,
            Some(VoucherHop {
                channel_id: channel_id.to_string(),
                claim_state,
                synced: Arc::new(AtomicBool::new(false)),
                rejected_through: Arc::new(Mutex::new(0)),
                signing: Arc::new(tokio::sync::Mutex::new(())),
            }),
        );
    }

    /// Set, or with `None` remove, the hop paying `peer_id`.
    fn set_hop(&self, peer_id: &str, hop: Option<VoucherHop>) {
        self.hops.rcu(|current| {
            let mut next = (**current).clone();
            match &hop {
                Some(hop) => next.insert(peer_id.to_string(), hop.clone()),
                None => next.remove(peer_id),
            };
            next
        });
    }

    /// Bind the voucher signer an x402 peering's row names to `peer_id`
    /// (ADR 0075 decision 4), so the peer's channel toward this node is the
    /// peer's when it shows up. Nothing for any other binding.
    pub(crate) fn bind_runtime_voucher_signer(
        &self,
        peer_id: &str,
        binding: &RuntimePeerChannel,
    ) -> Result<(), VoucherBindingError> {
        let (chain, voucher_signer, network) = match binding {
            RuntimePeerChannel::EvmVoucher {
                voucher_signer,
                network,
                ..
            } => (SettlementChain::Evm, voucher_signer, network),
            RuntimePeerChannel::SolanaVoucher {
                voucher_signer,
                network,
                ..
            } => (SettlementChain::Solana, voucher_signer, network),
            RuntimePeerChannel::XrplVoucher {
                voucher_signer,
                network,
                ..
            } => (SettlementChain::Xrpl, voucher_signer, network),
            RuntimePeerChannel::Evm { .. } | RuntimePeerChannel::Solana { .. } => return Ok(()),
        };
        let Some(signer) = crate::peering::parse_chain_key(chain, voucher_signer) else {
            tracing::warn!(
                peer_id,
                voucher_signer,
                "peering names a voucher signer this node cannot read; its inbound channel is \
                 not bound"
            );
            return Ok(());
        };
        // ADR 0086 decision 11: the peer's half is the peering's only on the
        // peering's own network. A row whose network does not parse (none
        // written since #1599 lacks one) binds on any, as before.
        let network = network.parse::<connector_domain::Network>().ok();
        self.voucher_bindings
            .bind_on_network(peer_id, signer, network.as_ref(), None)
    }

    // -- the store -----------------------------------------------------------

    fn lock_table(&self) -> std::sync::MutexGuard<'_, ()> {
        self.table_lock
            .lock()
            .expect("runtime peer/route table lock poisoned")
    }

    /// Persist `peers`/`routes` if this node has a store, otherwise a no-op.
    /// Called with the table lock held, before the matching `ArcSwap` is
    /// published.
    fn persist(
        &self,
        peers: &RuntimePeers,
        routes: &HashMap<String, PeerRoute>,
    ) -> Result<(), PeerRouteTableError> {
        match &self.store {
            Some(store) => store
                .persist(peers, routes)
                .map_err(PeerRouteTableError::from),
            None => Ok(()),
        }
    }
}
