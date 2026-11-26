use std::net::AddrParseError;
use std::path::PathBuf;

use connector_domain::AssetId;

use crate::batch_settlement::BatchSettlementRefusal;
use crate::client_channel_asset::ClientChannelAssetRefusal;
use crate::controller_keys::ControllerKeysRefusal;
use crate::denomination::DenominationRefusal;
use crate::identity::ClientIdentityRefusal;
use crate::node::NodeRefusal;
use crate::operator::OperatorRefusal;
use crate::pay_channel::PayChannelRefusal;
use crate::peer::PeerRefusal;
use crate::peer_channel::PeerChannelRefusal;
use crate::peering_asset::PeeringAssetRefusal;
use crate::route::RouteRefusal;
use crate::secret::SecretRefusal;
use crate::settlement::SettlementRefusal;
use crate::zk_note::ZkNoteRefusal;

use thiserror::Error;

/// Every way [`crate::Config::load`] can fail.
///
/// Each variant names the offending field and, where useful, the value the
/// operator wrote, so the error is actionable without opening the source.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("config file at {path} is not valid TOML: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },

    /// A refusal of the `[settlement]` tables, defined beside them in
    /// `settlement.rs`. Each table's refusals compose into this one type the
    /// same way (issue #1690), so the binary still handles one error.
    #[error(transparent)]
    Settlement(#[from] SettlementRefusal),

    /// A refusal of the `batch settlement` rules, defined beside them in `batch_settlement.rs`.
    #[error(transparent)]
    BatchSettlement(#[from] BatchSettlementRefusal),

    /// A refusal of the `client channel asset` rules, defined beside them in `client_channel_asset.rs`.
    #[error(transparent)]
    ClientChannelAsset(#[from] ClientChannelAssetRefusal),

    /// A refusal of the `controller keys` rules, defined beside them in `controller_keys.rs`.
    #[error(transparent)]
    ControllerKeys(#[from] ControllerKeysRefusal),

    /// A refusal of the `denomination` rules, defined beside them in `denomination.rs`.
    #[error(transparent)]
    Denomination(#[from] DenominationRefusal),

    /// A refusal of the `identity` rules, defined beside them in `identity.rs`.
    #[error(transparent)]
    ClientIdentity(#[from] ClientIdentityRefusal),

    /// A refusal of the `node` rules, defined beside them in `node.rs`.
    #[error(transparent)]
    Node(#[from] NodeRefusal),

    /// A refusal of the `operator` rules, defined beside them in `operator.rs`.
    #[error(transparent)]
    Operator(#[from] OperatorRefusal),

    /// A refusal of the `pay channel` rules, defined beside them in `pay_channel.rs`.
    #[error(transparent)]
    PayChannel(#[from] PayChannelRefusal),

    /// A refusal of the `peer channel` rules, defined beside them in `peer_channel.rs`.
    #[error(transparent)]
    PeerChannel(#[from] PeerChannelRefusal),

    /// A refusal of the `peering asset` rules, defined beside them in `peering_asset.rs`.
    #[error(transparent)]
    PeeringAsset(#[from] PeeringAssetRefusal),

    /// A refusal of the `peer` rules, defined beside them in `peer.rs`.
    #[error(transparent)]
    Peer(#[from] PeerRefusal),

    /// A refusal of the `route` rules, defined beside them in `route.rs`.
    #[error(transparent)]
    Route(#[from] RouteRefusal),

    /// A refusal of the `secret` rules, defined beside them in `secret.rs`.
    #[error(transparent)]
    Secret(#[from] SecretRefusal),

    /// A refusal of the `zk note` rules, defined beside them in `zk_note.rs`.
    #[error(transparent)]
    ZkNote(#[from] ZkNoteRefusal),

    #[error("invalid client_edge_addr '{value}': {source}")]
    InvalidBindAddr {
        value: String,
        #[source]
        source: AddrParseError,
    },

    #[error("invalid {field} '{value}': not a valid ILP address")]
    InvalidAddress { field: &'static str, value: String },

    #[error(
        "route '{prefix}' forwards to peer_id '{peer_id}', which no '[[peers]]' entry configures"
    )]
    UnknownPeerId { prefix: String, peer_id: String },

    #[error(
        "peer '{id}' has no '[[peer_channels]]' entry: a peer role is proven by a voucher on a \
         channel whose voucher signer is bound to the peering, or by the claim-state challenge \
         that signer signs (peer-carriage-spec.md §1.2, ADR 0075 decision 5), and a \
         config-declared peering's signer is named by its '[[peer_channels]]' row -- so without \
         one this peering can never take the peer role and its vouchers would be judged as a \
         stranger's. Add:\n\
         \n\
             [[peer_channels]]\n\
             peer_id = \"{id}\"\n\
             voucher_signer = \"<the peer's settlement address (EVM) or key (Solana)>\"\n\
         \n\
         see docs/protocol/configuration-spec.md"
    )]
    PeerChannelUnbound { id: String },

    #[error(
        "'[[peer_channels]]' names peer_id '{peer_id}', which no '[[peers]]' entry configures \
         -- a voucher signer bound to a peering that does not exist binds nothing"
    )]
    PeerChannelOrphaned { peer_id: String },

    /// A config peering's `[[peer_channels]]` row and its `[[pay_channels]]`
    /// row are on two networks of one chain (ADR 0086 decision 11): a
    /// peering is on one network, both of its channels. A node that moves
    /// value between networks does it with two peerings and a forward.
    #[error(
        "peering '{peer_id}' has its '[[peer_channels]]' row on {inbound} and its \
         '[[pay_channels]]' row on {outbound}: both channels of a peering are on one network \
         (ADR 0086 decision 11) -- put both rows on the same network, or make two peerings"
    )]
    PeeringNetworksDiffer {
        peer_id: String,
        inbound: String,
        outbound: String,
    },

    #[error(
        "'[[peer_channels]]' is configured but 'state_dir' is not: the vouchers a peer pays this \
         node with are journaled beside a client's, and their watermark is the replay defence -- \
         kept only in memory, every voucher a peer has already spent becomes spendable again the \
         next time this process restarts (issue #605). Set a top-level state_dir to a directory \
         this node can write, and mount it so it outlives the container"
    )]
    PeerChannelsWithoutStateDir,

    // -- `[[pay_channels]]` (ADR 0042 item 2, as ADR 0075 decision 6 amends
    // it): this node's own outbound x402 channel toward a next hop. See
    // `crate::pay_channel`'s module header.
    #[error(
        "'[[pay_channels]]' names peer_id '{peer_id}', which no '[[peers]]' entry configures -- \
         a channel that pays a peering that does not exist pays nobody. This table is how this \
         node covers every PREPARE it forwards to a hop (ADR 0042); the hop is named by the \
         same '[[peers]]' id a route's peer_id names"
    )]
    PayChannelOrphaned { peer_id: String },

    #[error(
        "'[[pay_channels]]' for peer '{peer_id}' names an outbound channel on {chain}, but this \
         node has no '{heading}' table, so there is no paying half to sign a voucher \
         with (ADR 0075). Add the table, or pay over a chain this node settles on",
        heading = crate::settlement::settlement_heading(.chain)
    )]
    PayChannelWithoutX402 {
        peer_id: String,
        chain: &'static str,
    },

    #[error(
        "channel '{value}' is both a '[[peer_channels]]' inbound_channel and a \
         '[[pay_channels]]' outbound_channel: an x402 channel moves value one way, so this node \
         is either its payer or its receiver, never both (ADR 0075)"
    )]
    ChannelInBothDirections { value: String },

    /// A runtime write took a peer id or route prefix the config file owns
    /// (ADR 0034). `peering_admission` owns the rule; a file's own rows are
    /// all config, so load cannot reach it -- it is named so that the two
    /// surfaces map every refusal.
    #[error("'{value}' is defined in this node's config file and cannot be changed at runtime")]
    PeeringOwnedByConfig { value: String },

    #[error(
        "peer '{peer_id}' is the next hop of route '{prefix}' but has no '[[pay_channels]]' \
         entry: a connector covers every PREPARE it sends (ADR 0042), so a peering this node \
         FORWARDS to must name the channel it pays that hop on. Open one with 'POST /channels' \
         and add:\n\
         \n\
             [[pay_channels]]\n\
             peer_id = \"{peer_id}\"\n\
             outbound_channel = \"<the x402 channel id (EVM) or account (Solana) it opened>\"\n\
             client_edge_url = \"<that hop's own POST /ilp endpoint>\"\n\
         \n\
         This key is REQUIRED, which by ADR 0009 makes it a breaking deploy: land the config \
         before moving the image tag, never the other way round"
    )]
    PayChannelUnbound { prefix: String, peer_id: String },

    #[error(
        "'[[pay_channels]]' is configured but 'state_dir' is not: the channel a row names is \
         held in this node's outbound-channel journal under state_dir, which is also where every \
         voucher signed on it is recorded before it is handed out (ADR 0075 decision 8)"
    )]
    PayChannelsWithoutStateDir,

    /// The other half of §11's removed-field row, spelled and worded
    /// exactly as PR #718 (`feat/delete-peer-role`) spells it.
    ///
    /// **Defined here, constructed there.** #718 owns the deletion of the
    /// raw-TCP transport itself (issue #679), including the top-level
    /// `peer_wire_addr` field, the listener `connector-cli` binds from it
    /// and the infra configs that still name it. This branch does not
    /// touch that listener, so on this branch alone the field still binds
    /// one; when the two land, #718's `if raw.peer_wire_addr.is_some()`
    /// meets this variant and the pair is complete.
    #[error(
        "'peer_wire_addr' was removed with the raw-TCP transport (ADR 0027, issue #679) -- \
         peer carriages are exposed on the connector's own listeners, not a separate socket; \
         see docs/operators/btp-peer-transport-bringup.md"
    )]
    PeerWireAddrRemoved,

    /// `[[client_channels]]`: the channels a `toon-channel` client claim could
    /// be paid on, declared with their counterparty (issue #558). Deleted
    /// with that claim scheme (ADR 0075 decision 9, issue #1384).
    #[error(
        "'[[client_channels]]' was removed with the toon-channel claim (ADR 0075, issue #1384): \
         every claim is an x402 'batch-settlement' voucher, whose channel is read from the chain \
         when the voucher presents it, so no channel is declared in config. Delete the table. A \
         client pays on an x402 channel it opens toward this node; a toon-channel channel still \
         open is drained on the last release that supports toon-channel claims"
    )]
    ClientChannelsRemoved,

    /// A tuning key of the client edge's `toon-channel` channel registry's
    /// liveness memo (issue #649), deleted with the registry (ADR 0075,
    /// issue #1384).
    #[error(
        "'{field}' was removed with the toon-channel client channel registry it tuned (ADR 0075, \
         issue #1384): every claim is an x402 voucher, whose channel is read from the settlement \
         backend with no liveness memo in between. Delete the line"
    )]
    ChannelLivenessKeyRemoved { field: &'static str },

    /// A settlement table is what lets this node admit a voucher on a
    /// channel it has never been configured for (ADR 0052, CF-27), so a node
    /// configuring one accepts claims from strangers -- and therefore has
    /// watermarks to lose. Issue #1186: this arm did not exist, so the
    /// permissionless shape, which is the one an operator should be running,
    /// was the one shape that could boot with its watermarks in memory.
    #[error(
        "a settlement backend is configured but 'state_dir' is not: this node resolves an \
         undeclared channel from chain and accepts its voucher (ADR 0052), so it takes payment \
         from senders it has never been configured for -- and would keep their replay \
         watermarks only in memory. Every claim any of them has already spent becomes \
         spendable again the next time this process restarts, and nothing in a log shows \
         that it did (issue #605, issue #1186). Set a top-level state_dir to a directory \
         this node can write, and mount it so it outlives the container"
    )]
    SettlementWithoutStateDir,

    /// A spending or deposit limit is counted in a journal under `state_dir` (ADR 0078
    /// decision 8); without one the count would not survive a restart.
    #[error(
        "controller key '{entry}' states a {limit} but 'state_dir' is not set: the day's \
         count is journaled under state_dir, and a count held only in memory would reset the \
         day at every restart. Set a top-level state_dir to a directory this node can write"
    )]
    LimitWithoutStateDir { entry: String, limit: &'static str },

    #[error("state_dir '{path}' exists but is not a directory")]
    StateDirNotADirectory { path: PathBuf },

    #[error(
        "unresolvable_lookup_budget_{field} is 0: this node would never resolve a channel it \
         was not explicitly configured with, so an unaffiliated buyer who opened a channel on \
         chain could not pay it at all -- which is the registration-free path issue #611 exists \
         to provide, switched off by a number that reads as a tightening (issue #613). Omit the \
         field for the default, or set how many lookups for channels that do not resolve are \
         allowed per window"
    )]
    ZeroUnresolvableLookupBudget { field: &'static str },

    #[error(
        "unresolvable_lookup_budget_window_secs is 0: a zero-length window restarts on every \
         request, so both allowances are spendable in full by every request and the budget \
         bounds nothing at all while appearing to be configured (issue #613). Omit the field for \
         the default, or set the number of seconds the allowances are counted over"
    )]
    ZeroUnresolvableLookupWindow,

    #[error(
        "btp_session_window is 0: a BTP session's first paid frame would wait forever for an \
         in-flight slot that does not exist, hanging every client on connect (issue #688). Omit \
         the field for the default, set 1 for the original lockstep session, or set how many of \
         one session's frames may be past claim admission at once"
    )]
    ZeroBtpSessionWindow,

    #[error(
        "unresolvable_lookup_budget_per_signer is {per_signer} but \
         unresolvable_lookup_budget_total is {total}: the node-wide allowance would refuse first \
         every time, so the per-signer number could never be reached and means nothing. Set it \
         to at most the total (issue #613)"
    )]
    UnresolvableLookupPerSignerAboveTotal { per_signer: u32, total: u32 },

    #[error(
        "unresolvable_lookup_budget_max_wait_ms is 0: this node would refuse a lookup outright \
         the moment its discovery drain saturated, rather than holding it for a slot. That hands \
         any sender able to sustain unresolvable_lookup_budget_total requests per window a switch \
         that turns the registration-free path of issue #611 off for every new buyer -- a worse \
         failure than the RPC spend the bound exists to prevent (issue #613). Omit the field for \
         the default, or set the milliseconds a lookup may wait for its turn"
    )]
    ZeroUnresolvableLookupMaxWait,

    #[error(
        "unresolvable_lookup_budget_max_wait_ms is {max_wait_ms} but \
         unresolvable_lookup_budget_window_secs is {window_secs} (either as written or by \
         default): the wait ceiling is the size of the waiting room, not just a timeout -- a \
         room drained at the configured rate and holding a lookup for {max_wait_ms} ms parks \
         more than a whole window's worth of them, which is more memory than the bound is worth \
         and a wait no packet's own deadline would survive (issue #613). Set it to at most the \
         window"
    )]
    UnresolvableLookupMaxWaitAboveWindow { max_wait_ms: u64, window_secs: u64 },

    #[error(
        "unresolvable_lookup_budget_window_secs is {window_secs}, above the {max_secs} s this \
         node will honour: a rate limit whose window outlives the process running it is not a \
         rate limit, and the arithmetic over it stops fitting an instant (issue #613)"
    )]
    UnresolvableLookupWindowTooLong { window_secs: u64, max_secs: u64 },

    /// The section itself, renamed rather than removed (ADR 0050).
    ///
    /// Two of `[announce]`'s fields feed the packet path -- they ride the x402
    /// greeting so a client with a stale genesis seed can bootstrap -- so this
    /// is a rename, not a deletion, and the error says so: an operator whose
    /// file still writes the old heading needs the new one, not a eulogy.
    #[error(
        "'[announce]' was renamed to '[node]' (ADR 0050, issue #1080): the section holds the \
         facts a node cannot introspect about itself, and it is named for what they are rather \
         than for a verb this connector no longer has (ADR 0046 removed the announce). Rename \
         the heading and keep `addresses`, `http_endpoint` and `btp_endpoint`; every other key \
         it used to carry is gone"
    )]
    AnnounceSectionRenamed,

    /// The one `socks_proxy` value is not a URL at all (ADR 0070 decision 3).
    ///
    /// Carries the text as written, because the usual cause is a bare
    /// `host:port` with no scheme -- which is exactly the shape every other
    /// SOCKS-taking tool accepts -- and an operator reading the message has
    /// to see that the scheme is what is missing.
    #[error("socks_proxy '{value}' is not a URL: {source}")]
    SocksProxyInvalidUrl {
        value: String,
        #[source]
        source: url::ParseError,
    },

    /// The `socks_proxy` URL names no host (ADR 0070 decision 3) --
    /// `socks5h://` on its own, or a `socks5h:9050` that looks like a
    /// `host:port` and is not one. `socks5h` is not a *special* scheme in
    /// the URL standard, so unlike `https://` it parses happily with an
    /// empty host; without this check such a value would load, the node
    /// would come up clean, and every onion dial would then fail on a proxy
    /// address that is not an address. A loaded `Config` needs no further
    /// validation anywhere downstream (ADR 0009), and that includes this.
    #[error(
        "socks_proxy '{value}' names no host to reach the proxy at -- write it as \
         'socks5h://<host>:<port>', e.g. 'socks5h://127.0.0.1:9050'. ('socks5h' is not a \
         special URL scheme, so a missing host parses rather than failing, which is why this \
         is checked by name)"
    )]
    SocksProxyNoHost { value: String },

    /// The `socks_proxy` URL names a scheme other than `socks5h` (ADR 0070
    /// decision 3) -- in practice `socks5`, which every SOCKS-taking tool
    /// spells that way and which is wrong here for a reason no operator can
    /// be expected to already know. Hence the length of the message: the
    /// `h` is the whole point of the key.
    #[error(
        "socks_proxy '{value}' has scheme '{scheme}', but it must be 'socks5h'. The 'h' is not \
         a preference: a 'socks5://' proxy resolves the hostname LOCALLY and dials the address \
         it gets back, and no local resolver can resolve a '.onion' or '.anyone' name -- so a \
         node that started with one would come up clean and then fail every onion peering at \
         dial time, for a reason nothing in its log explains. Resolution has to happen AT the proxy, which \
         is what 'socks5h://' asks for (ADR 0070)"
    )]
    SocksProxyScheme { value: String, scheme: String },

    /// The removed-section trap for purchasable peering (ADR 0043), the
    /// same shape [`ConfigError::PeerWireAddrRemoved`] and
    /// [`ConfigError::PeerCeilingRemoved`] already take: the section is
    /// still parsed so a stale config naming it stops the node by name
    /// rather than being silently dropped by `deny_unknown_fields`.
    ///
    /// One variant covers the whole table -- `prefix`, `price`,
    /// `lease_seconds` and every abuse bound -- because there is no
    /// surviving `[peer_sale]` for any of them to be written into: the
    /// fix is always to delete the section, never to correct a field.
    #[error(
        "'[peer_sale]' was removed with purchasable peering (ADR 0043) -- a peering cannot be \
         bought at all, so 'prefix', 'price', 'lease_seconds', 'max_purchased_rows', \
         'max_routes_per_payer', 'max_prefix_length', 'purchase_rate_limit' and \
         'purchase_rate_window_seconds' have nothing left to configure. Delete the whole \
         section; an operator still adds a peer and its route directly, over the operator \
         surface (POST /peers, POST /routes/peers) or in the config file"
    )]
    PeerSaleRemoved,

    /// A zk edge with nowhere to journal its spent nullifiers or keep its evidence.
    #[error(
        "[zk_note] is set but 'state_dir' is not: a zk edge journals every spent nullifier and \
         keeps its evidence file under state_dir, and one held only in memory would let a note \
         open a second allowance from a state it already spent after a restart. Set a \
         top-level state_dir to a directory this node can write"
    )]
    ZkNoteWithoutStateDir,

    /// A zk edge that forwards in a settlement asset has no declared rate out of MINA.
    #[error(
        "[zk_note] is set and this node pays its next hops in '{to}', but no [[rates]] row \
         declares a rate for '{from}' -> '{to}': no TWAP source quotes MINA, so the rate a \
         forward crosses at must be declared (ADR 0080 decision 7, ADR 0071)"
    )]
    ZkNoteRateMissing { from: AssetId, to: String },
}

impl From<connector_domain::peering_admission::PeeringRefusal> for ConfigError {
    /// The one place a domain refusal becomes the named refusal an operator
    /// reads at boot (ADR 0009). No wildcard arm: a new rule does not compile
    /// until it is named here (issue #1692).
    fn from(refusal: connector_domain::peering_admission::PeeringRefusal) -> Self {
        use connector_domain::peering_admission::PeeringRefusal;
        match refusal {
            PeeringRefusal::PeerIdOwnedByConfig(value)
            | PeeringRefusal::PrefixOwnedByConfig(value) => {
                ConfigError::PeeringOwnedByConfig { value }
            }
            PeeringRefusal::UnknownPeer { prefix, peer_id } => {
                ConfigError::UnknownPeerId { prefix, peer_id }
            }
            PeeringRefusal::PeerChannelOrphaned { peer_id } => {
                ConfigError::PeerChannelOrphaned { peer_id }
            }
            PeeringRefusal::PeeringWithoutChannel(id) => ConfigError::PeerChannelUnbound { id },
            PeeringRefusal::PayChannelOrphaned { peer_id } => {
                ConfigError::PayChannelOrphaned { peer_id }
            }
            PeeringRefusal::PayChannelOnUnsettledChain { peer_id, chain } => {
                ConfigError::PayChannelWithoutX402 { peer_id, chain }
            }
            PeeringRefusal::ChannelInBothDirections { channel } => {
                ConfigError::ChannelInBothDirections { value: channel }
            }
            PeeringRefusal::PeeringNetworksDiffer {
                peer_id,
                inbound,
                outbound,
            } => ConfigError::PeeringNetworksDiffer {
                peer_id,
                inbound,
                outbound,
            },
            PeeringRefusal::RouteWithoutPayChannel { prefix, peer_id } => {
                ConfigError::PayChannelUnbound { prefix, peer_id }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every table's refusals compose transparently (issue #1690): wrapping a
    /// refusal in the public error changes neither its message nor what it
    /// is, so the table module owns the message and the binary sees one type.
    macro_rules! transparent {
        ($($name:ident: $wrapper:ident($refusal:expr);)*) => {$(
            #[test]
            fn $name() {
                let refusal = $refusal;
                let message = refusal.to_string();
                let error = ConfigError::from(refusal);
                assert_eq!(error.to_string(), message);
                assert!(matches!(error, ConfigError::$wrapper(_)));
            }
        )*};
    }

    transparent! {
        a_batch_settlement_refusal_is_the_config_error_it_is_wrapped_in:
            BatchSettlement(BatchSettlementRefusal::BatchSettlementZeroMinimumSponsoredDeposit);
        a_controller_keys_refusal_is_the_config_error_it_is_wrapped_in:
            ControllerKeys(ControllerKeysRefusal::ControllerKeysFileInvalid {
                path: PathBuf::from("keys.toml"),
                message: "bad".to_owned(),
            });
        a_denomination_refusal_is_the_config_error_it_is_wrapped_in:
            Denomination(DenominationRefusal::RateGuardsMissing);
        a_client_identity_refusal_is_the_config_error_it_is_wrapped_in:
            ClientIdentity(ClientIdentityRefusal::ClientIdentityIdEmpty);
        a_node_refusal_is_the_config_error_it_is_wrapped_in:
            Node(NodeRefusal::NodeNoAddresses);
        an_operator_refusal_is_the_config_error_it_is_wrapped_in:
            Operator(OperatorRefusal::OperatorNoWriteKeys);
        a_pay_channel_refusal_is_the_config_error_it_is_wrapped_in:
            PayChannel(PayChannelRefusal::PayChannelDuplicatePeer { peer_id: "p".to_owned() });
        a_peer_channel_refusal_is_the_config_error_it_is_wrapped_in:
            PeerChannel(PeerChannelRefusal::PeerChannelVoucherSignerMissing {
                peer_id: "p".to_owned(),
            });
        a_peering_asset_refusal_is_the_config_error_it_is_wrapped_in:
            PeeringAsset(PeeringAssetRefusal::PeeringTokenNotDeclared {
                peer_id: "p".to_owned(),
                asset: "a".to_owned(),
            });
        a_peer_refusal_is_the_config_error_it_is_wrapped_in:
            Peer(PeerRefusal::PeerIdEmpty);
        a_route_refusal_is_the_config_error_it_is_wrapped_in:
            Route(RouteRefusal::MissingApex);
        a_secret_refusal_is_the_config_error_it_is_wrapped_in:
            Secret(SecretRefusal::SignerKmsIdEmpty);
        a_zk_note_refusal_is_the_config_error_it_is_wrapped_in:
            ZkNote(ZkNoteRefusal::ZkNoteViaSocksProxyWithoutProxy);
    }
}
