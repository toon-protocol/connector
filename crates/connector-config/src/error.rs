use std::net::AddrParseError;
use std::path::PathBuf;

use connector_domain::{AssetChain, AssetId, AssetIdError, GuardError, Price, RateError};

use crate::settlement::SettlementChain;

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

    #[error("invalid client_edge_addr '{value}': {source}")]
    InvalidBindAddr {
        value: String,
        #[source]
        source: AddrParseError,
    },

    #[error("invalid {field} '{value}': not a valid ILP address")]
    InvalidAddress { field: &'static str, value: String },

    #[error(
        "invalid child name '{name}': must be a single ILP address label \
         (alphanumeric, '-', '_')"
    )]
    InvalidChildName { name: String },

    #[error("invalid handler_url '{value}' for route '{prefix}': {source}")]
    InvalidHandlerUrl {
        prefix: String,
        value: String,
        #[source]
        source: url::ParseError,
    },

    #[error("handler_url '{value}' for route '{prefix}' must be http or https")]
    UnsupportedUrlScheme { prefix: String, value: String },

    #[error("duplicate route prefix '{prefix}'")]
    DuplicatePrefix { prefix: String },

    #[error("children are configured but no apex is set: add a top-level 'apex' field")]
    MissingApex,

    #[error("signer config must set exactly one of 'key_file' or 'kms_key_id', but {reason}")]
    SignerLocationAmbiguous { reason: &'static str },

    #[error("signer key_file does not exist or is not a file: {0}")]
    SignerKeyFileNotFound(PathBuf),

    #[error("signer kms_key_id must not be empty")]
    SignerKmsIdEmpty,

    #[error(
        "the [operator] section is present but names no bearer token, or an empty one: the \
         operator surface would have no read authentication. Set exactly one of \
         'bearer_token_file = \"/app/data/…\"' (what a deployed node should use -- this \
         repository is public, so a committed config must not carry the literal) or \
         'bearer_token = \"…\"'"
    )]
    OperatorMissingBearerToken,

    #[error(
        "the [operator] section is present but names no write keys: the operator surface \
         would accept writes from no one. Set exactly one of 'write_keys_file = \
         \"/app/data/…\"' (the deployed form -- a file an operator can edit to revoke, which \
         a committed literal is not) or 'write_keys = [\"…\"]'"
    )]
    OperatorNoWriteKeys,

    #[error(
        "invalid operator write_keys entry '{value}': must be 64 hex characters \
         (a 32-byte ed25519 public key)"
    )]
    OperatorInvalidWriteKey { value: String },

    #[error(
        "the [operator] section sets both '{literal}' and '{file}': exactly one of them says \
         where this setting's value comes from, and two answers is not a merge -- it is an \
         unanswerable question about which one gates the operator surface, with the losing \
         value left in the file looking authoritative. Keep '{file}' (the deployed form -- \
         this repository is public, so a committed config must not carry the literal) and \
         delete '{literal}'; see docs/operators/key-rotation-runbook.md"
    )]
    OperatorSettingAmbiguous {
        literal: &'static str,
        file: &'static str,
    },

    #[error(
        "the [operator] section sets '{setting} = {path}', which does not exist or is not a \
         file: an operator surface that cannot read its own authentication authenticates \
         nobody, so this is refused at load rather than becoming a surface that is enabled \
         and quietly rejects every request. The path is resolved the same way '[signer] \
         key_file' is -- relative to the process's working directory, so write an absolute \
         path; see docs/operators/key-rotation-runbook.md"
    )]
    OperatorFileNotFound {
        setting: &'static str,
        path: PathBuf,
    },

    #[error(
        "the [operator] section sets '{setting} = {path}', which could not be read as text: \
         {source} -- check the file's permissions inside the container (a secret file is \
         usually mode 600 and must be owned by the uid the connector runs as); see \
         docs/operators/key-rotation-runbook.md"
    )]
    OperatorFileUnreadable {
        setting: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(
        "the [operator] section sets '{setting} = {path}', which carries nothing once \
         whitespace and comments are stripped: an empty file is the same unauthenticated \
         surface an empty literal would be, and a truncated or half-written file is the usual \
         cause -- rewrite it; see docs/operators/key-rotation-runbook.md"
    )]
    OperatorFileEmpty {
        setting: &'static str,
        path: PathBuf,
    },

    #[error(
        "invalid operator write_keys_file entry at {path}:{line}: '{value}' must be 64 hex \
         characters (a 32-byte ed25519 public key). One key per line, '#' starts a comment; \
         see docs/operators/key-rotation-runbook.md"
    )]
    OperatorWriteKeysFileInvalidKey {
        path: PathBuf,
        line: usize,
        value: String,
    },

    #[error(
        "route '{prefix}' must set exactly one of 'handler_url' or 'peer_id', but neither is set"
    )]
    RouteMissingTarget { prefix: String },

    #[error(
        "route '{prefix}' must set exactly one of 'handler_url' or 'peer_id', but both are set"
    )]
    RouteTargetAmbiguous { prefix: String },

    #[error("route '{prefix}' has an empty 'peer_id'")]
    RoutePeerIdEmpty { prefix: String },

    #[error(
        "route '{prefix}' terminates locally at '{handler_url}' but sets no 'price': a \
         terminated route is never silently free -- set 'price = 0' if that is deliberate"
    )]
    RouteMissingPrice { prefix: String, handler_url: String },

    /// ADR 0061's removed-field row. A fee attaches to a **peering**, not to
    /// a route: what this hop retains is the same number whichever prefix
    /// the packet was addressed to, so it belongs on the `[[peers]]` row
    /// that names the counterparty. Refused on **any** route, terminated or
    /// forwarded -- the narrower `TerminatedRouteHasFee` this replaces let a
    /// forwarded route keep writing one, which is exactly the spelling ADR
    /// 0061 moves.
    #[error(
        "route '{prefix}' sets 'fee', which moved to the '[[peers]]' row it was always about \
         (ADR 0061): a fee is what this hop retains for carrying one packet to a counterparty, \
         and that is the same number whichever prefix is addressed. Delete it here and write \
         'fee' on the '[[peers]]' entry this route's 'peer_id' names -- a terminating route \
         never had one to move, since an app's work is paid for by 'price'"
    )]
    RouteFeeRemoved { prefix: String },

    #[error(
        "route '{prefix}' forwards to peer '{peer_id}' but sets no 'price': a forwarded route \
         is never silently free either (ADR 0028) -- 'price' is what this connector's client \
         edge charges a client for a packet to this prefix, and the peering's own 'fee' is \
         only what this hop retains of it. Set 'price = 0' if free carriage is deliberate"
    )]
    PeerRouteMissingPrice { prefix: String, peer_id: String },

    #[error(
        "route '{prefix}' forwards to a peer and sets 'transport = \"{value}\"', which this \
         connector does not yet apply to a forwarded route (issue #701, ADR 0028) -- such a \
         route accepts both client transports. Remove the 'transport' field"
    )]
    PeerRouteHasTransport { prefix: String, value: String },

    #[error(
        "route '{prefix}' sets transport = '{value}', which this connector does not recognize \
         -- only 'http', 'btp' or 'both' are valid (issue #701). Omit the field for the \
         default ('both'), or set one of those three"
    )]
    InvalidTransportPolicy { prefix: String, value: String },

    #[error(
        "handler_url '{handler_url}' is priced inconsistently: route '{first_prefix}' charges \
         {first_price} but route '{second_prefix}' charges {second_price} -- an app cannot tell \
         which request arrived under which price, so the cheaper one would always win"
    )]
    ConflictingHandlerPrice {
        handler_url: String,
        first_prefix: String,
        first_price: Price,
        second_prefix: String,
        second_price: Price,
    },

    #[error(
        "route '{prefix}' forwards to peer_id '{peer_id}', which no '[[peers]]' entry configures"
    )]
    UnknownPeerId { prefix: String, peer_id: String },

    #[error("peer entry has an empty 'id'")]
    PeerIdEmpty,

    #[error(
        "duplicate peer id '{id}': two '[[peers]]' entries name it, so which endpoint, fee \
         and cap apply to it is unanswerable -- see \
         docs/operators/btp-peer-transport-bringup.md"
    )]
    DuplicatePeerId { id: String },

    // -- The peer carriage config surface (issue #677, peer-carriage-spec
    // §11). Every message names the bring-up doc, because a peering that
    // does not come up produces no other evidence an operator can read.
    #[error(
        "invalid peer_expose value '{value}': a connector exposes 'btp', 'http', 'both' or \
         'neither' peer carriage (peer-carriage-spec.md §2.1) -- 'neither' is the NAT'd \
         operator, who exposes nothing and only dials out. Omit the field for the default \
         ('neither'); see docs/operators/btp-peer-transport-bringup.md"
    )]
    InvalidPeerExposure { value: String },

    /// ADR 0042 (item 3): the forwarded-arrival migration knob, spelled
    /// wrong. Refused by name rather than falling through to the default,
    /// because the default here is the permissive one -- a typo meant as
    /// "enforce" would leave this peering carrying forwards for free.
    #[error(
        "invalid forwarded_claim_enforcement value '{value}' for peer '{id}': a peer sets \
         'observe' (admit an uncovered forwarded arrival and log it -- the default, because the \
         fleet's send halves are not live yet) or 'enforce' (refuse it with the x402 greeting, \
         ADR 0042's permanent rule). Omit the field for the default ('observe'); see \
         docs/operators/claim-policy-rollout.md"
    )]
    InvalidForwardedClaimEnforcement { id: String, value: String },

    /// ADR 0042's cap, written as `0`. A cap of zero refuses every packet
    /// this peering could carry, so it is a peering that silently does
    /// nothing -- and there is deliberately no spelling that turns the cap
    /// off, since the whole point of the rule is that a bound always
    /// exists.
    #[error(
        "max_packet_amount = 0 for peer '{id}': the cap is the largest amount this connector \
         will forward to a peer in ONE packet (ADR 0042), so zero refuses every packet that \
         peering could ever carry. Write a positive amount in the settlement asset's base \
         units, or omit the field for the default"
    )]
    PeerMaxPacketAmountZero { id: String },

    #[error(
        "invalid endpoint '{value}' for peer '{id}': {source} -- a peer endpoint is a URL \
         ('wss://host:port/path' for BTP, 'https://host:port/path' for ILP-over-HTTP), not a \
         host:port pair; see docs/operators/btp-peer-transport-bringup.md"
    )]
    InvalidPeerEndpoint {
        id: String,
        value: String,
        #[source]
        source: url::ParseError,
    },

    #[error(
        "endpoint '{value}' for peer '{id}' has scheme '{scheme}', which selects no peer \
         carriage: 'wss://' selects BTP and 'https://' selects ILP-over-HTTP, and there is no \
         third one (ADR 0027 deleted the raw-TCP transport). Both are TLS-only because a \
         peering carries signed balance proofs (ADR 0004), so 'ws://' and 'http://' are not \
         accepted either -- except at a host ending in '.onion' or '.anyone', which \
         authenticates the circuit with its own key and needs no certificate (ADR 0070), and \
         this host is not one; see docs/operators/btp-peer-transport-bringup.md"
    )]
    PeerEndpointScheme {
        id: String,
        value: String,
        scheme: String,
    },

    #[error(
        "peer '{id}' sets 'credential': the '{{peerId, secret}}' shared secret is deleted (ADR \
         0060). A peering is proven by a verified claim on one of its '[[peer_channels]]' \
         rows, not by a string both operators wrote into their own config files, and there is \
         no replacement key -- not renamed, not optional. Delete the 'credential' table from \
         this peer; see docs/protocol/peer-carriage-spec.md §1.2"
    )]
    PeerCredentialRemoved { id: String },

    #[error(
        "peer '{id}' has no '[[peer_channels]]' entry: a peer role needs a channel binding \
         and a verified claim on one of its channels (peer-carriage-spec.md §1.2 P2/P3), so \
         without one this peering can never take the peer role and its claims would be judged \
         as a stranger's. \
         This is the surface whose absence made ADR 0024 inert (issue #620); see \
         docs/operators/btp-peer-transport-bringup.md"
    )]
    PeerChannelUnbound { id: String },

    #[error(
        "'[[peer_channels]]' names peer_id '{peer_id}', which no '[[peers]]' entry configures \
         -- a channel bound to a peering that does not exist binds nothing; see \
         docs/operators/btp-peer-transport-bringup.md"
    )]
    PeerChannelOrphaned { peer_id: String },

    #[error(
        "channel '{value}' is configured in both '[[peer_channels]]' and \
         '[[client_channels]]': peer and client claim watermarks are separate records, so one \
         channel in both namespaces lets the same money be counted as credit twice \
         (peer-carriage-spec.md §1.8). Keep the namespaces disjoint -- see \
         docs/operators/btp-peer-transport-bringup.md"
    )]
    ChannelInBothNamespaces { value: String },

    // -- `[[pay_channels]]` (ADR 0042 item 2, issue #881): the channel this
    // node PAYS a next hop from, as an ordinary client of it. See
    // `crate::pay_channel`'s module header for why this is a third table
    // rather than a row in either of the two above.
    #[error(
        "'[[pay_channels]]' names peer_id '{peer_id}', which no '[[peers]]' entry configures -- \
         a channel that pays a peering that does not exist pays nobody. This table is how this \
         node covers every PREPARE it forwards to a hop (ADR 0042); the hop is named by the \
         same '[[peers]]' id a route's peer_id names"
    )]
    PayChannelOrphaned { peer_id: String },

    #[error(
        "'[[pay_channels]]' for peer '{peer_id}' names channel '{value}', which is not a \
         32-byte on-chain channel id: it must be 64 hex characters, optionally '0x'-prefixed. \
         This is the channel this node PAYS that hop from as an ordinary client of it"
    )]
    PayChannelInvalidId { peer_id: String, value: String },

    #[error(
        "'[[pay_channels]]' for peer '{peer_id}' has an invalid {field} '{value}': it must be a \
         20-byte EVM address, optionally '0x'-prefixed. It is half of the EIP-712 domain the \
         covering claim is signed under, and a claim signed under the wrong domain recovers to \
         a different address and is refused at the far gate"
    )]
    PayChannelInvalidAddress {
        peer_id: String,
        field: &'static str,
        value: String,
    },

    #[error(
        "'[[pay_channels]]' for peer '{peer_id}' has an unusable client_edge_url '{value}': \
         {source}. It is that hop's own 'POST /ilp' endpoint -- where this node arrives as an \
         ordinary buyer, and where 'POST /ilp/claim-state' (issue #693) answers where this \
         node's claims on the channel stand"
    )]
    PayChannelInvalidClientEdgeUrl {
        peer_id: String,
        value: String,
        #[source]
        source: url::ParseError,
    },

    #[error(
        "'[[pay_channels]]' for peer '{peer_id}' has client_edge_url '{value}', whose scheme \
         '{scheme}' is not one this node may ask a channel's claim state over: it must be \
         'https://' (or 'http://' with peer_allow_plaintext_endpoints, which is a loopback and \
         test setting). The ask carries a signed challenge -- an EIP-712 digest on EVM, an ed25519 \
         message on Solana, and on either chain a capability to read a channel's state -- so it is \
         TLS-only by default. A peering's own 'wss://' endpoint is \
         not this URL and is never turned into it by swapping scheme and appending a path \
         (ADR 0030)"
    )]
    PayChannelClientEdgeUrlScheme {
        peer_id: String,
        value: String,
        scheme: String,
    },

    #[error(
        "'[[pay_channels]]' names peer '{peer_id}' twice: the outbound client ledger keeps one \
         nonce line per next hop, so a second row would be a second channel for one line and \
         which one signed would depend on file order"
    )]
    PayChannelDuplicatePeer { peer_id: String },

    #[error(
        "channel '{value}' is named by two '[[pay_channels]]' rows: one channel paid from by \
         two next hops is one channel carrying two nonce lines, and the far gate resolves that \
         by refusing one of them as a replay"
    )]
    PayChannelDuplicate { value: String },

    #[error(
        "channel '{value}' is configured in both '[[pay_channels]]' and '[[client_channels]]': \
         '[[client_channels]]' is channels this node RECEIVES claims on and '[[pay_channels]]' \
         is one it PAYS from, so one channel in both roles is the same double-count \
         'ChannelInBothNamespaces' refuses between the peer and client books (ADR 0030, \
         peer-carriage-spec.md §1.8)"
    )]
    PayChannelIsAlsoAClientChannel { value: String },

    #[error(
        "'[[pay_channels]]' names peer '{peer_id}' but this node has no '[settlement.evm]' \
         table: a covering claim is an EIP-712 balance proof signed by the channel's on-chain \
         participant, which IS this node's settlement address -- the same key ADR 0024's \
         outbound peer claims use. There is no second key to configure and none is invented \
         (ADR 0030)"
    )]
    PayChannelWithoutEvmSettlement { peer_id: String },

    #[error(
        "'[[pay_channels]]' for peer '{peer_id}' has an invalid {field} '{value}': it must be a \
         base58-encoded 32-byte Solana address. It is the channel account every covering claim \
         on this row signs over (ADR 0053), and one that does not decode is a claim the far \
         gate verifies against a different account"
    )]
    PayChannelInvalidSolanaAccount {
        peer_id: String,
        field: &'static str,
        value: String,
    },

    #[error(
        "'[[pay_channels]]' for peer '{peer_id}' names a 'program_id', which this table does \
         not declare: the settlement program a covering claim is signed under is \
         '[settlement.solana] program_id' and nothing else -- the one program this node can \
         redeem through, and since ADR 0053 part of what every claim signs. Remove the key \
         (issue #1128's rule, and the same one '[[peer_channels]]' and '[[client_channels]]' \
         already hold)"
    )]
    PayChannelProgramIdNotDeclared { peer_id: String },

    #[error(
        "'[[pay_channels]]' names a Solana channel for peer '{peer_id}' but this node has no \
         '[settlement.solana]' table: a covering claim on Solana is an ed25519 balance proof \
         signed by the channel's on-chain participant -- '[settlement.solana.key]'s key -- \
         under '[settlement.solana] program_id', and neither exists to read. There is no \
         second key to configure and none is invented (ADR 0030)"
    )]
    PayChannelWithoutSolanaSettlement { peer_id: String },

    #[error(
        "'[[pay_channels]]' names a Solana channel for peer '{peer_id}', but '[settlement.solana] \
         program_id' is '{value}', which is not a base58-encoded 32-byte address. ADR 0053 signs \
         that program id into every claim on the channel, so it has to be a real address before \
         a claim can be minted at all -- not only when the settlement backend first dials a chain"
    )]
    PayChannelSolanaSettlementProgramIdInvalid { peer_id: String, value: String },

    #[error(
        "'[[pay_channels]]' names Solana channel '{value}' for peer '{peer_id}', which has no \
         Solana '[[peer_channels]]' row for that same peering. Unlike an EVM claim's optional \
         EIP-712 domain, a Solana claim's 'programId' is a REQUIRED wire field, and the peer \
         carriage renders it from that peering's Solana peer-channel row -- so this row would \
         mint claims that could not be put on the wire at all. Holding one channel in both \
         roles with one hop is the deployed shape (the peer role for what arrives, the client \
         role for what this node sends); add the matching '[[peer_channels]]' row"
    )]
    PayChannelSolanaWithoutPeerChannel { peer_id: String, value: String },

    #[error(
        "peer '{peer_id}' is the next hop of route '{prefix}' but has no '[[pay_channels]]' \
         entry: a connector covers every PREPARE it sends (ADR 0042), so a peering this node \
         FORWARDS to must name the channel it pays that hop from. There is no postpay \
         fallback any more -- ADR 0004's 'the claim covering crossing n rides crossing n + 1' \
         was deleted in issue #1145 -- so without this row every packet on that route would be \
         refused at packet time. Add:\n\
         \n\
             [[pay_channels]]\n\
             peer_id = \"{peer_id}\"\n\
             # EVM:    channel_id / chain_id / token_network\n\
             # Solana: channel_account (and a Solana '[[peer_channels]]' row for the same \
         channel)\n\
             client_edge_url = \"<that hop's own POST /ilp endpoint>\"\n\
         \n\
         This key is newly REQUIRED, which by ADR 0009 makes it a breaking deploy: land the \
         config before moving the image tag, never the other way round"
    )]
    PayChannelUnbound { prefix: String, peer_id: String },

    #[error(
        "'[[pay_channels]]' is configured but 'state_dir' is not: the outbound client ledger \
         keeps the highest nonce it has ever ISSUED to each next hop, and it has to outlive the \
         process -- a restart that reissued a nonce would fork this node's own outbound nonce \
         line and the far gate would refuse one of the two claims as a replay"
    )]
    PayChannelsWithoutStateDir,

    #[error(
        "route '{prefix}' forwards to peer '{peer_id}', which this connector can never \
         originate to: the peering configures no 'endpoint' (so this connector never dials it) \
         and peer_expose does not include 'btp' (so it can never be reached back over a dialed \
         session either) -- packets flow only in the dialing direction on HTTP \
         (peer-carriage-spec.md §6.4). Give the peer a 'wss://' or 'https://' endpoint, or set \
         peer_expose to include 'btp'; see docs/operators/btp-peer-transport-bringup.md"
    )]
    PeerRouteUndeliverable { prefix: String, peer_id: String },

    #[error(
        "peer '{id}' has no 'endpoint' and this connector's peer_expose is 'neither': it dials \
         nothing and accepts nothing, so this peering can never establish and no amount of \
         retrying changes that (peer-carriage-spec.md §2.2). Give it an endpoint, or expose a \
         carriage for it to dial; see docs/operators/btp-peer-transport-bringup.md"
    )]
    PeerUndialable { id: String },

    #[error(
        "invalid [[peer_channels]] channel_id '{value}': must be 64 hex characters \
         (an on-chain 32-byte channel identifier), optionally '0x'-prefixed"
    )]
    PeerChannelInvalidId { value: String },

    #[error(
        "invalid [[peer_channels]] {field} '{value}': must be 40 hex characters \
         (a 20-byte EVM address), optionally '0x'-prefixed"
    )]
    PeerChannelInvalidAddress { field: &'static str, value: String },

    #[error("[[peer_channels]] names channel '{value}' more than once")]
    PeerChannelDuplicate { value: String },

    /// An EVM `[[peer_channels]]` row on a node with no `[settlement.evm]`
    /// table (issue #1138) -- the EVM twin of
    /// [`ConfigError::PeerChannelWithoutSolanaSettlement`], under the one
    /// rule `crate::settlement::SettlementTables` states for all four
    /// channel tables.
    ///
    /// Not the same missing input the Solana row has: an EVM row declares
    /// its own EIP-712 domain, so the config is *complete* and the node
    /// happily verified inbound peer claims on it. What is missing is the
    /// node's EVM identity. `[settlement.evm.key]` is the address a
    /// channel names as this node's participant, and
    /// `TokenNetwork.claimFromChannel` refuses any caller that is not one
    /// -- so without the table there is no address this node could ever
    /// redeem the claim as, from this process or any other. It also signs
    /// no outbound covering claim (ADR 0024's peer claim identity is that
    /// same key), so the peering could only ever take and never pay.
    ///
    /// Refused rather than left loading, for the reason #1134 gave:
    /// `Config::load` already requires every peering to carry a row
    /// ([`ConfigError::PeerChannelUnbound`]), so a row that binds nothing
    /// leaves the peering bound on paper and unredeemable in fact.
    #[error(
        "peer '{peer_id}' names an EVM '[[peer_channels]]' row but this node has no \
         '[settlement.evm]' table: that table is where this node's EVM address comes from, and \
         a channel's claims are redeemed by its on-chain participant \
         ('TokenNetwork.claimFromChannel' reverts 'InvalidParticipant' for anyone else). With \
         no table there is no address to be that participant, so this node would verify the \
         peer's inbound claims, render carriage for them and never be able to collect -- and it \
         would sign no covering claim outbound either, since ADR 0024's peer claim is signed by \
         that same settlement key. Add '[settlement.evm]', or delete the row and peer over a \
         chain this node settles on"
    )]
    PeerChannelWithoutEvmSettlement { peer_id: String },

    #[error(
        "invalid [[peer_channels]] {field} '{value}': must be base58 encoding a 32-byte \
         Solana account"
    )]
    PeerChannelInvalidSolanaAccount { field: &'static str, value: String },

    /// The removed-key rejection for `[[peer_channels]] program_id` (issue
    /// #1128), in the shape [`ConfigError::PeerCeilingRemoved`] and
    /// [`ConfigError::PeerSaleRemoved`] already take: the key is still
    /// parsed, purely so it can be refused **by name** rather than silently
    /// ignored or lost in `#[serde(untagged)]`'s "matched no variant".
    ///
    /// It was a second declaration of a fact `[settlement.solana]` already
    /// states, and since ADR 0053 bound the settlement program into a
    /// Solana claim's signed message the two disagreeing is not a typo with
    /// a cosmetic symptom: the node verifies inbound peer claims under the
    /// row's program while its settlement backend redeems under
    /// `[settlement.solana]`'s, so it renders carriage for claims it can
    /// never cash. Removing the key is what makes that state unwritable --
    /// the same "no second declaration" rule #981/#1082 applied to
    /// `[[client_channels]]`.
    #[error(
        "peer '{peer_id}' sets 'program_id' on a Solana '[[peer_channels]]' row, which was \
         removed (issue #1128): a peer channel's settlement program is read from \
         '[settlement.solana] program_id', the one program this node can actually redeem a \
         claim under. Since ADR 0053 that program id is bound into a Solana claim's signed \
         message, so a row naming a different one made this node accept peer claims it could \
         never settle. Delete the key rather than replace it -- the value it should have held \
         is already in '[settlement.solana]'"
    )]
    PeerChannelProgramIdRemoved { peer_id: String },

    /// A Solana `[[peer_channels]]` row on a node with no
    /// `[settlement.solana]` table (issue #1128). The sibling of
    /// [`ConfigError::PayChannelWithoutEvmSettlement`], and refused for the
    /// same reason: with the per-row `program_id` gone there is no other
    /// place a program id could come from, and a node that cannot settle on
    /// Solana at all cannot redeem a Solana peer claim however correctly it
    /// was signed.
    ///
    /// Refused rather than skipped-with-a-warning (which is what the client
    /// edge does for the same shape today): `Config::load` already requires
    /// every peering to carry a `[[peer_channels]]` row
    /// ([`ConfigError::PeerChannelUnbound`]), so skipping this one would
    /// leave the peering bound on paper and unverifiable in fact.
    #[error(
        "peer '{peer_id}' names a Solana '[[peer_channels]]' row but this node has no \
         '[settlement.solana]' table: a peer claim on that channel is signed against the \
         settlement program's id (ADR 0053) and redeemed through that same program, and \
         without the table there is neither. Add '[settlement.solana]', or delete the row and \
         peer over a chain this node settles on"
    )]
    PeerChannelWithoutSolanaSettlement { peer_id: String },

    /// `[settlement.solana] program_id` is checked only for non-emptiness
    /// where it is resolved; a Solana `[[peer_channels]]` row now takes its
    /// program id from there, so the value has to be a real 32-byte address
    /// before any claim can be judged against it (issue #1128). Named
    /// against the row that needs it *and* the table that holds it, because
    /// the fix is in the table and the symptom is on the row.
    #[error(
        "peer '{peer_id}' names a Solana '[[peer_channels]]' row, whose settlement program is \
         read from '[settlement.solana] program_id' -- but that is '{value}', which is not \
         base58 encoding a 32-byte Solana program address. Since ADR 0053 a claim on this \
         channel signs that program id, so it must name a real deployed program"
    )]
    PeerChannelSolanaSettlementProgramIdInvalid { peer_id: String, value: String },

    #[error(
        "[[peer_channels]] is configured but 'state_dir' is not: this node would accept peer \
         claims and keep their replay watermarks only in memory, so every claim a peer has \
         already spent becomes spendable again the next time this process restarts (issue \
         #605). Set a top-level state_dir to a directory this node can write, and mount it so \
         it outlives the container"
    )]
    PeerChannelsWithoutStateDir,

    #[error(
        "peer '{id}' sets 'addr', which was removed with the raw-TCP transport (ADR 0027, \
         issue #679) -- a peer is reached by 'endpoint' (a wss:// or https:// URL) instead; \
         see docs/operators/btp-peer-transport-bringup.md"
    )]
    PeerAddrRemoved { id: String },

    /// §11's removed-field row, `ceiling` half (ADR 0033, issue #882):
    /// nothing tracks trailing exposure any more, so there is nothing for a
    /// ceiling to bound. Cites the record that removed the machinery, not the
    /// reasoning behind it (issue #1068) -- ADR 0042's covering-claim rule is
    /// a target record whose forwarded half is unbuilt, and a boot error must
    /// not depend on it.
    #[error(
        "peer '{id}' sets 'ceiling', which was removed when the exposure machinery was retired \
         (ADR 0033, issue #882) -- nothing tracks trailing exposure, so there is nothing for a \
         ceiling to bound. Delete the key rather than replace it; see \
         docs/operators/btp-peer-transport-bringup.md"
    )]
    PeerCeilingRemoved { id: String },

    /// §11's removed-field row, `flush_interval_ms` half (ADR 0033, issue
    /// #882): a claim no longer trails the fulfilment it covers, so there is
    /// nothing left to flush.
    #[error(
        "peer '{id}' sets 'flush_interval_ms', which was removed for the same reason 'ceiling' \
         was (ADR 0033, issue #882): nothing tracks trailing exposure, so there is no pending \
         claim for a timer to flush. Delete the key rather than replace it; see \
         docs/operators/btp-peer-transport-bringup.md"
    )]
    PeerFlushIntervalRemoved { id: String },

    /// §11's removed-field row, `claim_enforcement` (ADR 0042 item 4, issue
    /// #1077): the issue #883 migration ramp is gone, so `"observe"` names
    /// no mode and `"enforce"` names the only behaviour there is. Refused by
    /// name rather than ignored, because a config still writing `"observe"`
    /// was written by an operator who believes uncovered arrivals to a
    /// priced termination are admitted here, and none are.
    ///
    /// Not to be confused with `forwarded_claim_enforcement`, which is a
    /// live field and defaults the other way
    /// ([`ConfigError::InvalidForwardedClaimEnforcement`]).
    #[error(
        "peer '{id}' sets 'claim_enforcement', which was removed with the issue #883 migration \
         ramp (ADR 0042 item 4, issue #1077): an uncovered peer PREPARE to a priced termination \
         is now always refused, so 'observe' names a mode this build does not have and \
         'enforce' names the only behaviour there is. Delete the key rather than replace it -- \
         and note that 'forwarded_claim_enforcement', which governs forwarded arrivals, is a \
         different and still-live field; see docs/operators/claim-policy-rollout.md"
    )]
    PeerClaimEnforcementRemoved { id: String },

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

    #[error(
        "the [settlement] section names chain '{value}', which this connector does not \
         implement -- only 'evm' is recognized"
    )]
    SettlementUnknownChain { value: String },

    #[error("the [settlement] section's rpc_url is empty")]
    SettlementMissingRpcUrl,

    #[error("invalid settlement rpc_url '{value}': {source}")]
    SettlementInvalidRpcUrl {
        value: String,
        #[source]
        source: url::ParseError,
    },

    #[error("settlement rpc_url '{value}' must be http or https")]
    SettlementUnsupportedRpcScheme { value: String },

    #[error(
        "invalid settlement contract_address '{value}': must be 40 hex characters \
         (a 20-byte EVM address), optionally '0x'-prefixed"
    )]
    SettlementInvalidContractAddress { value: String },

    #[error(
        "invalid settlement token_address '{value}': must be 40 hex characters \
         (a 20-byte EVM address), optionally '0x'-prefixed"
    )]
    SettlementInvalidTokenAddress { value: String },

    #[error("the [settlement] section's decimals must not be zero")]
    SettlementZeroDecimals,

    #[error(
        "the [settlement] section's key must set exactly one of 'key_file' or 'kms_key_id', \
         but {reason}"
    )]
    SettlementKeyLocationAmbiguous { reason: &'static str },

    #[error("settlement key_file does not exist or is not a file: {0}")]
    SettlementKeyFileNotFound(PathBuf),

    #[error("settlement kms_key_id must not be empty")]
    SettlementKmsIdEmpty,

    #[error(
        "the [settlement] section is present but names no chain at all -- add an \
         [settlement.evm] and/or [settlement.solana] table, or remove the section"
    )]
    SettlementSectionEmpty,

    #[error("the [settlement.solana] section's program_id is empty")]
    SettlementMissingProgramId,

    #[error("the [settlement.solana] section's token_address is empty")]
    SettlementMissingSolanaTokenAddress,

    #[error(
        "the [settlement.evm] section's channel_index_confirmations is 0: applying a log at \
         chain head has nothing to fall back on if the head reorgs, and issue #661 deliberately \
         ships no unwind logic for that case -- a channel inside the confirmation window is \
         meant to fall through to a direct chain read instead. Omit the field for the default \
         confirmation depth, or set a depth of at least 1 block"
    )]
    SettlementChannelIndexConfirmationsZero,

    /// A settlement table asked for its RPC to ride the node's `socks_proxy`
    /// and the node has none (ADR 0073 decision 1). The key selects the one
    /// proxy ADR 0070 gives a node; it never names a second, and a dial that
    /// was meant for a circuit is never made direct instead.
    #[error(
        "[settlement.{table}] sets rpc_via_socks_proxy = true, but this node configures no \
         socks_proxy. The key selects the node's one proxy and never names a second, and a \
         settlement RPC meant for a circuit is never dialed direct instead (ADR 0073). Set \
         socks_proxy = \"socks5h://<host>:<port>\" at the top level, or remove \
         rpc_via_socks_proxy"
    )]
    SettlementRpcViaSocksProxyWithoutProxy { table: &'static str },

    /// A settlement table asked for its RPC to ride a circuit to a plain
    /// `http://` endpoint that is not an onion address (ADR 0073). The exit
    /// relay would be able to read and rewrite every answer.
    #[error(
        "[settlement.{table}] rpc_url '{value}' is plain http, and rpc_via_socks_proxy = true \
         would send it through an exit relay that can read and rewrite every answer (a \
         channel's deposit, a transaction's receipt). Use the endpoint's https URL. Plain http \
         over a circuit is accepted only for a .onion or .anyone host, whose address \
         authenticates the service itself (ADR 0070, ADR 0073)"
    )]
    SettlementRpcViaSocksProxyPlaintext { table: &'static str, value: String },

    #[error(
        "invalid [[client_channels]] channel_id '{value}': must be 64 hex characters \
         (an on-chain 32-byte channel identifier), optionally '0x'-prefixed"
    )]
    ClientChannelInvalidId { value: String },

    #[error(
        "invalid [[client_channels]] {field} '{value}': must be 40 hex characters \
         (a 20-byte EVM address), optionally '0x'-prefixed"
    )]
    ClientChannelInvalidAddress { field: &'static str, value: String },

    #[error("[[client_channels]] names channel '{value}' more than once")]
    ClientChannelDuplicate { value: String },

    #[error(
        "invalid [[client_channels]] {field} '{value}': must be base58 encoding a 32-byte \
         Solana account"
    )]
    ClientChannelInvalidSolanaAccount { field: &'static str, value: String },

    /// An EVM `[[client_channels]]` row on a node with no
    /// `[settlement.evm]` table (issue #1138). The client-edge case of the
    /// one rule `crate::settlement::SettlementTables` states, and the one
    /// the issue called the hard half -- so, explicitly: **the declared
    /// channel path's latitude does not reach this.**
    ///
    /// `connector_client_edge::DepositFloor::Unknown` exempts a declared
    /// channel from the collateral cap (issue #646) because how much
    /// unverified exposure to take on a channel is a *policy*, and an
    /// operator hand-declaring a channel is making it. That latitude is
    /// over how much may be spent on a channel this node is a participant
    /// of. It is not latitude over whether such a channel exists at all: a
    /// claim is redeemed by the channel's on-chain participant and this
    /// node's EVM participant address IS `[settlement.evm.key]`
    /// (ADR 0030's "no second key ... and none is invented", the same
    /// sentence [`ConfigError::PayChannelWithoutEvmSettlement`] makes).
    /// With no table there is no such address, so the row names a channel
    /// this node is not in and every write it buys is given away. That is
    /// a fact about the chain with exactly one answer -- the category
    /// issue #1136 put the EIP-712 domain in -- not a policy.
    ///
    /// The wire already agreed before this refusal existed: a
    /// settlement-less node's x402 greeting carries no `settlement` or
    /// `settlements` key at all, so no conforming client can even discover
    /// the domain to sign under, and this connector's own announce path
    /// refuses to pay such a node by name ("a node with no settlement
    /// backend cannot be paid by channel claim",
    /// `connector_cli::announce`'s `NoSettlementTerms`).
    #[error(
        "'[[client_channels]]' names EVM channel '{channel_id}' but this node has no \
         '[settlement.evm]' table: a client claim is an EIP-712 balance proof redeemed by the \
         channel's on-chain participant, which IS this node's settlement address -- there is no \
         second key to configure and none is invented (ADR 0030). With no table there is no \
         address to be that participant, so this node would accept the claim, serve the paid \
         write and never be able to collect. Declaring a channel is an operator's own credit \
         decision (issue #646) and stays one; being able to redeem it at all is not a policy \
         but a fact about the chain (issue #1136). Add '[settlement.evm]', or delete the row"
    )]
    ClientChannelWithoutEvmSettlement { channel_id: String },

    /// A Solana `[[client_channels]]` row on a node with no
    /// `[settlement.solana]` table (issue #1138), the twin of
    /// [`ConfigError::ClientChannelWithoutEvmSettlement`] and of
    /// [`ConfigError::PeerChannelWithoutSolanaSettlement`].
    ///
    /// Over-determined here, as it is on the peer table: besides having no
    /// Solana identity to be the channel's participant, the node has no
    /// program id to read, and since ADR 0053 that program id is part of
    /// what every Solana claim signs -- so the row could not even be given
    /// a verification domain.
    ///
    /// This replaces `connector-cli`'s warn-and-skip, which its own
    /// comment already called the worse answer: a skipped row is a
    /// configured channel that silently refuses every claim as unknown.
    #[error(
        "'[[client_channels]]' names Solana channel '{channel_account}' but this node has no \
         '[settlement.solana]' table: a claim on that channel signs the settlement program's id \
         (ADR 0053), which is read from that table, and is redeemed by the channel's on-chain \
         participant, which is that table's key. Without it there is neither a program to judge \
         the claim under nor an address to collect it at. Previously this row was skipped with \
         a warning and every claim on it refused as an unknown channel; it is refused by name \
         at load instead (issue #1138). Add '[settlement.solana]', or delete the row"
    )]
    ClientChannelWithoutSolanaSettlement { channel_account: String },

    /// `[settlement.solana] program_id` is checked only for non-emptiness
    /// where it is resolved, and a Solana `[[client_channels]]` row now
    /// carries it (issue #1138) exactly as the peer table has since #1128
    /// -- so the value has to be a real 32-byte address before any claim
    /// can be judged against it. The client-edge twin of
    /// [`ConfigError::PeerChannelSolanaSettlementProgramIdInvalid`], and
    /// it closes a real crash: `ClientChannelRegistry::record_solana`
    /// base58-decodes the program id and `connector-cli` `expect`s that
    /// decode, so a malformed one used to panic the boot with a message
    /// blaming the row's own fields.
    #[error(
        "'[[client_channels]]' names Solana channel '{channel_account}', whose settlement \
         program is read from '[settlement.solana] program_id' -- but that is '{value}', which \
         is not base58 encoding a 32-byte Solana program address. Since ADR 0053 a claim on \
         this channel signs that program id, so it must name a real deployed program"
    )]
    ClientChannelSolanaSettlementProgramIdInvalid {
        channel_account: String,
        value: String,
    },

    #[error("[[client_identities]] entry has an empty 'id'")]
    ClientIdentityIdEmpty,

    #[error("[[client_identities]] names identity '{id}' more than once")]
    DuplicateClientIdentityId { id: String },

    #[error(
        "[[client_channels]] is configured but 'state_dir' is not: this node would accept \
         claims and keep their replay watermarks only in memory, so every claim a client \
         has already spent becomes spendable again the next time this process restarts \
         (issue #605). Set a top-level state_dir to a directory this node can write, and \
         mount it so it outlives the container"
    )]
    ClientChannelsWithoutStateDir,

    /// A settlement table is what registers the `ClientChannelSource` that
    /// resolves an undeclared channel from chain (ADR 0052, CF-27), so a
    /// node configuring one accepts claims from strangers whether or not it
    /// declares a single `[[client_channels]]` row -- and therefore has
    /// watermarks to lose. Issue #1186: this arm did not exist, so the
    /// permissionless shape, which is the one an operator should be running,
    /// was the one shape that could boot with its watermarks in memory.
    #[error(
        "a settlement backend is configured but 'state_dir' is not: this node resolves an \
         undeclared channel from chain and accepts the claim (ADR 0052), so it takes payment \
         from senders it has never been configured for -- and would keep their replay \
         watermarks only in memory. Every claim any of them has already spent becomes \
         spendable again the next time this process restarts, and nothing in a log shows \
         that it did (issue #605, issue #1186). Set a top-level state_dir to a directory \
         this node can write, and mount it so it outlives the container"
    )]
    SettlementWithoutStateDir,

    #[error("state_dir '{path}' exists but is not a directory")]
    StateDirNotADirectory { path: PathBuf },

    #[error(
        "channel_liveness_ttl_secs is 0: this node would re-read the chain for every channel \
         on every packet rather than caching a resolution at all, which is a way to exhaust \
         an RPC endpoint's request budget and take the node's own paid writes down with it \
         (issue #649). Omit the field for the default, or set the number of seconds a \
         resolved channel's liveness may be believed for"
    )]
    ZeroChannelLivenessTtl,

    #[error(
        "channel_reattempt_interval_ms is 0: this node would put no floor at all on how often \
         one channel can make it read the chain, so a single client -- by sending packets, by \
         sending them at once, or by re-presenting one claim its channel cannot cover -- \
         becomes one RPC request each (issue #649). Omit the field for the default, or set the \
         milliseconds one channel must wait between lookups"
    )]
    ZeroChannelReattemptInterval,

    #[error(
        "channel_serve_stale_secs is {serve_stale_secs}s but channel_liveness_ttl_secs is \
         {ttl_secs}s: a resolved channel would stop being believed and stop being servable at \
         the same moment, so the stale window could never be used. Set it to at least the ttl, \
         or to 0 to never serve a reading this node could not confirm"
    )]
    ServeStaleShorterThanLivenessTtl {
        serve_stale_secs: u64,
        ttl_secs: u64,
    },

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

    // -- The `[node]` section (ADR 0050, issue #1080) --
    //
    // Every one of these refuses a value this node would otherwise publish
    // in its own self-description, which is read by strangers and acted on.
    // That is what makes them load errors rather than warnings.
    #[error(
        "[node] names no addresses: a section with no `addresses` describes no node at all. \
         Set `addresses = [\"g.your.node\"]` (primary first) -- see \
         docs/protocol/self-description-spec.md (ADR 0050, issue #1080)"
    )]
    NodeNoAddresses,

    /// Issue #1220: `btp_endpoint` is required when `peer_expose` opens a BTP
    /// peer listener; `http_endpoint` whenever it opens any -- a peer covering
    /// a forward asks this node's client edge for claim-state over HTTP
    /// whichever carriage the packet rides (issue #1217), so a peerable node
    /// with no HTTP endpoint is one nobody can pay.
    #[error(
        "[node] {field} is not set, but peer_expose = \"{peer_expose}\" makes this node \
         peerable and a peer needs it (btp_endpoint to dial over BTP; http_endpoint to ask \
         this node's client edge for claim-state over HTTP, whichever carriage a packet \
         rides): a node behind TLS termination cannot learn its own public name, so this is \
         an operator fact and there is deliberately no default. The retired announcer sidecar \
         DID default it, and its compiled-in fallback still names a `/rust/ilp` path that \
         answers 410 Gone on both devnet boxes -- a default here is how a node ends up \
         publishing a dead URL to whoever asks (ADR 0050, issue #1220)"
    )]
    NodeMissingEndpoint {
        field: &'static str,
        peer_expose: &'static str,
    },

    #[error("[node] {field} '{value}' is not a URL: {source}")]
    NodeInvalidUrl {
        field: &'static str,
        value: String,
        #[source]
        source: url::ParseError,
    },

    #[error(
        "[node] {field} '{value}' has scheme '{scheme}', but this field must name one of: \
         {allowed}. The two URLs a node publishes are where clients PAY it, over two different \
         carriages: `http_endpoint` is ILP-over-HTTP (`https://`) and `btp_endpoint` is BTP \
         (`wss://`). Conflating them publishes an address no client can reach (ADR 0050)"
    )]
    NodeEndpointScheme {
        field: &'static str,
        value: String,
        scheme: String,
        allowed: String,
    },

    /// A `[node]` (or a stale `[announce]`) key that died with the kind:10032
    /// announce (ADR 0046, issue #1074).
    ///
    /// The removed-key trap [`ConfigError::PeerWireAddrRemoved`] and
    /// [`ConfigError::PeerSaleRemoved`] already take, for the same reason: the
    /// devnet boxes bind-mount configs that lead the repo copies, so a stale
    /// file must stop the node **by name** rather than load with the key
    /// silently dropped (ADR 0009).
    ///
    /// One variant covers all fifteen, because the fix is always the same --
    /// delete the line. Nothing survives for any of them to be rewritten into:
    /// what a node publishes about itself is now either one of `[node]`'s three
    /// configured facts or derived from a settlement backend that proved it
    /// against a chain.
    #[error(
        "'[node] {field}' was removed with the kind:10032 announce (ADR 0046, issue #1074): a \
         connector answers when asked and never announces, so there is no event for this key to \
         ride on. `[node]` keeps exactly three fields -- `addresses`, `http_endpoint` and \
         `btp_endpoint` -- the facts a node cannot introspect about itself; everything else it \
         publishes is derived from the settlement backends and the route table (ADR 0050). \
         Delete the line. `relay_url` in particular described software BEHIND this connector, \
         which a self-description never carries, and `solana_chain_id` was a second declaration \
         of a fact `[settlement.solana]` already holds -- which is how a mainnet node came to \
         describe itself as devnet (issue #981)"
    )]
    AnnounceKeyRemoved { field: &'static str },

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

    /// A `[[tokens]]` row whose `asset` is not an asset at all (ADR 0071
    /// decision 3). Carries the domain's own words, because
    /// `AssetIdError` already says the three things that can be wrong with
    /// the text -- an unknown chain, no chain at all, no token -- and
    /// restating them here would be a second place for them to drift.
    #[error("invalid [[tokens]] asset '{value}': {source}")]
    TokenAssetInvalid {
        value: String,
        #[source]
        source: AssetIdError,
    },

    /// Two `[[tokens]]` rows for one token. Refused rather than merged: a
    /// rate table is keyed by token, so the second row is either a typo or
    /// a disagreement, and a disagreement resolved silently is the one a
    /// packet finds out about.
    #[error(
        "[[tokens]] declares '{asset}' more than once -- a token has one row, and two \
         spellings of one contract address (a checksummed one and a lowercase one) are one \
         token"
    )]
    DuplicateToken { asset: AssetId },

    /// Two `[[tokens]]` rows both set `numeraire = true` (ADR 0071
    /// decision 3). The boot refusal that keeps peg arithmetic out of the
    /// node: a cross rate is composed as `X -> Y = (X/numeraire) /
    /// (Y/numeraire)`, and composing one leg through USDC and the other
    /// through DAI silently prices the USDC/DAI peg at exactly 1 -- a peg
    /// nobody declared, which holds right up until it does not.
    #[error(
        "[[tokens]] names two numeraires, '{first}' and '{second}': a node declares exactly \
         one, because every cross rate composes through it. Mixing them would price the peg \
         between the two at 1 without anyone declaring it -- set 'numeraire = true' on one \
         row and declare the other pair's rate in [[rates]] if you deal it"
    )]
    MixedNumeraire { first: AssetId, second: AssetId },

    /// The numeraire's own row carries a `quote`. The numeraire is what
    /// every quote path ends at, so a quote on it is a path from a token to
    /// itself -- there is nothing it could read.
    #[error(
        "[[tokens]] gives the numeraire '{asset}' a quote: the numeraire is the token every \
         other token's quote path ends AT, so it has no price of its own to read. Remove the \
         'quote' from this row"
    )]
    NumeraireQuoted { asset: AssetId },

    /// `quote = []`. An empty path names no pool and is not a declaration
    /// of anything; a token priced by hand simply has no `quote` key.
    #[error(
        "[[tokens]] gives '{asset}' an empty quote: a quote is one or two pools, and a token \
         with no pool to read is declared without a 'quote' and priced by a [[rates]] row"
    )]
    TokenQuoteEmpty { asset: AssetId },

    /// More than two legs (ADR 0071 decision 3). Not a longer path -- a
    /// path nobody has reasoned about: each leg is an independent
    /// manipulation surface and the legs' errors compose.
    #[error(
        "[[tokens]] gives '{asset}' a quote of {legs} pools: ADR 0071 allows one (a \
         numeraire-quoted token) or two (a token whose only real venue is quoted in an \
         intermediate, e.g. ANYONE/WETH then WETH/USDC). Every extra leg is another \
         manipulation surface and another rounding, so a third is refused rather than \
         composed"
    )]
    TokenQuoteTooLong { asset: AssetId, legs: usize },

    /// A quote leg's `quote_token` is not an asset.
    #[error("invalid quote_token '{value}' in '{asset}'s quote: {source}")]
    TokenQuoteTokenInvalid {
        asset: AssetId,
        value: String,
        #[source]
        source: AssetIdError,
    },

    /// A quote leg quoting into a token on another chain. There is no
    /// cross-chain pool: a path that leaves the token's own settlement
    /// chain names a venue that does not exist.
    #[error(
        "'{asset}'s quote names '{quote_token}', which is on another chain: ADR 0071 puts a \
         token's quote on its OWN settlement chain -- the one chain the peering already \
         guarantees this node RPC for -- and no pool spans two of them. Price this pair with \
         a [[rates]] row instead"
    )]
    TokenQuoteOffChain {
        asset: AssetId,
        quote_token: AssetId,
    },

    /// A quote leg's pool is not an address on its chain.
    #[error(
        "invalid pool '{value}' in '{asset}'s quote: an EVM pool is 40 hex characters \
         (optionally '0x'-prefixed) and a Solana pool is a base58 32-byte account. A pool \
         this node cannot address is a rate it can never read"
    )]
    TokenQuotePoolInvalid { asset: AssetId, value: String },

    /// A quote leg with `twap_window_secs = 0`. A window of nothing is a
    /// spot read, and ADR 0071 decision 3 has none: the whole defence
    /// against a shoved pool is that manipulation has to be sustained for
    /// the length of the window.
    #[error(
        "'{asset}'s quote sets 'twap_window_secs = 0': a window of nothing is a spot read, \
         and a spot ratio is manipulable inside one block. Set a window long enough that \
         holding the pool at a false price across it costs an attacker more than the packets \
         it would misprice"
    )]
    TokenQuoteZeroWindow { asset: AssetId },

    /// A quote on a token whose chain this node has no `[settlement]` table
    /// for (ADR 0071 decision 3).
    #[error(
        "'{asset}' has a quote but this node has no [settlement.{chain}] table: a quote is \
         read over that table's own RPC endpoint, so a pool on a chain this node does not \
         settle on is one it can never take a reading from. Add the settlement table, or \
         drop the quote and price this token's pairs with [[rates]] rows"
    )]
    TokenQuoteWithoutSettlement { asset: AssetId, chain: AssetChain },

    /// A quote path with no numeraire to end at.
    #[error(
        "'{asset}' has a quote but no [[tokens]] row sets 'numeraire = true': a quote path \
         ends at the numeraire, and a node with none has nowhere for it to end"
    )]
    QuoteWithoutNumeraire { asset: AssetId },

    /// The last leg of a quote path lands somewhere other than the
    /// numeraire (ADR 0071 decision 3). The refusal that makes composition
    /// sound: every quote answers "how much numeraire is this token worth",
    /// and one that answers in WETH instead would be composed with the
    /// others as if it had answered in the numeraire.
    #[error(
        "'{asset}'s quote ends at '{ends_at}', not at the numeraire '{numeraire}': every \
         quote path ends at the numeraire, because a cross rate is composed as (X/numeraire) \
         / (Y/numeraire) and a leg that answers in a different unit is silently treated as \
         though it had answered in that one. Add the final leg that reaches the numeraire \
         (e.g. WETH/USDC after ANYONE/WETH)"
    )]
    TokenQuoteDoesNotEndAtNumeraire {
        asset: AssetId,
        ends_at: AssetId,
        numeraire: AssetId,
    },

    /// A `[[rates]]` row whose `from` or `to` is not an asset.
    #[error("invalid [[rates]] {field} '{value}': {source}")]
    RateRowAssetInvalid {
        field: &'static str,
        value: String,
        #[source]
        source: AssetIdError,
    },

    /// A `[[rates]]` row whose two sides are one token. A pair with itself
    /// is not a denomination boundary, so nothing would ever look it up.
    #[error(
        "[[rates]] declares '{asset}' against itself: a rate crosses a denomination \
         boundary, and a token is not a boundary with itself -- a forward whose two legs \
         hold the same token is not a conversion and takes the flat fee alone"
    )]
    RateRowSelfPair { asset: AssetId },

    /// A `[[rates]]` row naming a token no `[[tokens]]` row declares (ADR
    /// 0071 decision 3).
    #[error(
        "[[rates]] declares '{from}' -> '{to}', but '{unknown}' is not a token this node \
         deals: every side of a rate is a [[tokens]] row, so that a mistyped contract \
         address is a boot refusal rather than a rate filed under a key no forward ever \
         looks up"
    )]
    RateRowUnknownToken {
        from: AssetId,
        to: AssetId,
        unknown: AssetId,
    },

    /// Two `[[rates]]` rows for one ordered pair. The reverse pair is a
    /// different row and is not this: direction is the trade.
    #[error(
        "[[rates]] declares '{from}' -> '{to}' more than once: one ordered pair has one row. \
         (The reverse pair is a DIFFERENT row and may carry a different rate -- direction is \
         the trade.)"
    )]
    DuplicateRateRow { from: AssetId, to: AssetId },

    /// A `[[rates]]` row whose declared rate is not a rate -- a zero
    /// denominator, or the zero numerator ADR 0071 decision 7 refuses for
    /// the same reason. The check itself lives in `connector_domain::Rate`,
    /// which is where a rate is constructed at all; this variant exists so
    /// that the operator reading the refusal learns which ROW was wrong,
    /// which the domain's message on its own cannot say.
    #[error("invalid rate on [[rates]] '{from}' -> '{to}': {source}")]
    RateRowInvalid {
        from: AssetId,
        to: AssetId,
        #[source]
        source: RateError,
    },

    /// A declared guard that is not one (ADR 0071 decision 5): a spread at
    /// or above the whole mid, a fraction with no whole, a ttl of no time
    /// at all. The check is each guard's own constructor in
    /// `connector_domain` -- which is where a guard is made at all -- and
    /// this variant exists so the operator reading the refusal learns which
    /// row carried it, which the domain's message cannot say. `row` is
    /// `[rate_guards]`, or the ordered pair whose `[[rates]]` row overrode
    /// it.
    #[error("invalid guard on {row}: {source}")]
    RateGuardInvalid {
        row: String,
        #[source]
        source: GuardError,
    },

    /// A node that can produce a rate but declared no `[rate_guards]` (ADR
    /// 0071 decision 5). Required exactly when something can produce a
    /// rate, and only then: declaring tokens alone -- which is what a
    /// same-asset cross-chain hop does -- produces none and needs none.
    #[error(
        "this node declares a rate (a [[rates]] row, or a [[tokens]] quote) but no \
         [rate_guards] table: the three guards have no safe default. A defaulted spread \
         would be zero -- dealing at mid and donating the risk -- and a defaulted 'ttl' or \
         'max_move' would be a number nobody wrote being enforced on an operator's book. \
         Add '[rate_guards]' with 'spread', 'ttl_secs' and 'max_move'; a [[rates]] row may \
         override any of the three for its own pair"
    )]
    RateGuardsMissing,

    /// A peering whose channels hold a token no `[[tokens]]` row declares
    /// (ADR 0071 decision 1, issue #1292).
    ///
    /// Only ever reached on a node that declares tokens: one that declares
    /// none resolves no peering at all and is not held to this rule, which
    /// is what makes ADR 0071 free for every operator who deals nothing.
    /// On a node that does deal, the alternative to this refusal is a
    /// forward discovering mid-packet that it cannot say what unit it is
    /// carrying -- and the packet is already paid for by then.
    #[error(
        "peering '{peer_id}' is denominated in '{asset}', which is not a token this node \
         deals: a node that declares [[tokens]] must be able to say which declared token \
         every peering holds, because a forward between two peerings is a conversion exactly \
         when their tokens differ. The token comes from the [settlement] table that peering's \
         channels settle through -- declare it with a [[tokens]] row, or remove the [[tokens]] \
         table if this node deals nothing"
    )]
    PeeringTokenNotDeclared { peer_id: String, asset: AssetId },

    /// One peering whose channel rows sit on two chains, and therefore in
    /// two tokens (ADR 0071 decision 1, issue #1292). A packet's amount is
    /// denominated by the channel it rides, so a peering with two units has
    /// no unit at all: which one a forward was converting into would depend
    /// on which of the two tables the reader consulted.
    #[error(
        "peering '{peer_id}' holds channels in two tokens, '{first}' and '{second}': a \
         peering is one denomination -- a packet's amount is denominated by the channel it \
         rides, and a forward out of this one has no single unit to convert into. Give each \
         chain its own [[peers]] row, so that every peering's [[peer_channels]] and \
         [[pay_channels]] rows settle through one [settlement] table"
    )]
    PeeringTokenAmbiguous {
        peer_id: String,
        first: AssetId,
        second: AssetId,
    },

    /// A chain whose client-edge channels hold a token no `[[tokens]]` row
    /// declares (ADR 0071 decision 1, issue #1301) -- the client edge's
    /// sibling of [`ConfigError::PeeringTokenNotDeclared`], and refused for
    /// the stronger reason.
    ///
    /// Named by CHAIN rather than by channel, because the channel that
    /// forces this refusal is the one no `[[client_channels]]` row names: a
    /// settlement table registers the `ClientChannelSource` that admits a
    /// buyer this operator has never heard of (ADR 0052, issue #502), so
    /// every chain with a settlement table can carry an arrival whose
    /// denomination a forward must know. Left unresolved on a dealing node,
    /// such an arrival would forward across a real boundary at an implied
    /// 1:1 -- decision 2's failure arriving by the one door the absence rule
    /// does not cover.
    ///
    /// Only ever reached on a node that declares tokens; one that declares
    /// none resolves no client channel at all and is not held to this rule.
    #[error(
        "client-edge channels on the '{chain}' chain are denominated in '{asset}', which is \
         not a token this node deals: a node that declares [[tokens]] must be able to say \
         which declared token every channel it accepts a claim on holds, because a forward \
         out of a client arrival is a conversion exactly when that token and the outgoing \
         peering's differ. This applies to every channel on the chain and not only to a \
         declared [[client_channels]] row -- a [settlement.{chain}] table is what lets this \
         node accept a claim on a channel it has never been configured for (ADR 0052). The \
         token comes from that table's own 'token_address' -- declare it with a [[tokens]] \
         row, remove the [settlement.{chain}] table, or remove the [[tokens]] table if this \
         node deals nothing"
    )]
    ClientChannelTokenNotDeclared {
        chain: SettlementChain,
        asset: AssetId,
    },
}
