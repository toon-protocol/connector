//! The two roles and what each one grants.
//!
//! There is no session binding here, and there used to be one. A
//! `SessionRoleBinding` bound a BTP session's role once, at the `auth`
//! frame, and held it for the socket's lifetime — which was right while a
//! per-session credential decided role. ADR 0060 deleted that credential,
//! and §1.5 inverted with it: role is a property of the **frame**, decided
//! from the claim that frame carries, so there is nothing left to bind and
//! no second bind to refuse.
//!
//! §1.7 states the containment as an enumeration "because 'peer trust' and
//! 'client trust' are otherwise undefined, and undefined trust is what
//! leaks". It is an enumeration here too, for the same reason: [`Capability`]
//! is a closed enum matched exhaustively by [`SessionRole::grants`], so
//! adding a capability without deciding which roles hold it does not
//! compile.

use std::fmt;

/// The role of one interaction (§1.1).
///
/// **A property of the frame, not of the session** (§1.5, as amended by
/// #868): each frame stands on the claim it carries, and a frame carrying
/// no claim that satisfies P2 and P3 is a client frame however many peer
/// frames preceded it on the same socket. A per-session credential fixed a
/// per-session role; a per-packet claim fixes a per-packet one.
///
/// Two variants, and there will not be a third. §1.2 admits no `Unknown`,
/// no unroled state and no degraded peer: "if either fails, for any
/// reason, the interaction has role `client`". An `Unknown` variant would
/// be a state every downstream match has to handle, and the handling it
/// would get is the fallthrough §1.2 forbids.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SessionRole {
    /// Everything that is not a proven peer, including a frame that named
    /// a configured peer channel and failed to prove it (§1.6). The
    /// default, because §1.5 starts every interaction here.
    #[default]
    Client,
    /// A proven peering: the frame carried a voucher, or a peer-role
    /// challenge, whose signature verified against the voucher signer the
    /// chain records for its channel, and that signer is bound to this
    /// peering (ADR 0075 decision 5).
    Peer {
        /// The **bound** peer id — the peering this node bound the signer to,
        /// never a string the interaction asserted. The evidence names a
        /// channel, the chain names its signer and this node's binding names
        /// the relation, which is what lets everything downstream treat this
        /// as an identifier rather than as input.
        peer_id: String,
    },
}

impl SessionRole {
    /// A proven peering with `peer_id`.
    #[must_use]
    pub fn peer(peer_id: impl Into<String>) -> Self {
        SessionRole::Peer {
            peer_id: peer_id.into(),
        }
    }

    /// Whether this is a proven peering.
    #[must_use]
    pub fn is_peer(&self) -> bool {
        matches!(self, SessionRole::Peer { .. })
    }

    /// Whether this is a client interaction.
    #[must_use]
    pub fn is_client(&self) -> bool {
        matches!(self, SessionRole::Client)
    }

    /// The proven peer id, or `None` for a client.
    #[must_use]
    pub fn peer_id(&self) -> Option<&str> {
        match self {
            SessionRole::Client => None,
            SessionRole::Peer { peer_id } => Some(peer_id),
        }
    }

    /// The name this role is metered and logged under (ADR 0014).
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            SessionRole::Client => "client",
            SessionRole::Peer { .. } => "peer",
        }
    }

    /// Whether this role holds `capability` — §1.7's enumeration, as one
    /// exhaustive match.
    #[must_use]
    pub fn grants(&self, capability: Capability) -> bool {
        match capability {
            // Peer grants, "and only these" (§1.7).
            Capability::AdvancePeerWatermark
            | Capability::AppendToPeerClaimLedger
            | Capability::EmitClaimAck
            | Capability::AcceptFlush
            | Capability::BeARouteNextHop
            | Capability::CountTowardPeeringExposure => self.is_peer(),

            // §1.7's "peer role does NOT grant". Held by neither role, so
            // a caller reaching for one has no role to reach it with.
            Capability::FreeCarriage
            | Capability::RouteNotInRoutingTable
            | Capability::OperatorSurface
            | Capability::ExemptionFromSealing
            | Capability::SetRoutePrice
            | Capability::OpenForwardedPayload => false,
        }
    }
}

impl fmt::Display for SessionRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// §1.7's containment enumeration, spelled as values so it can be asserted
/// rather than remembered.
///
/// Two kinds of entry live here. The first six are the peer role's
/// grants: things a peer interaction may do and a client interaction MUST
/// be refused **even when it presents bytes that look like them**. The
/// last six are §1.7's "peer role does NOT grant" list — held by *neither*
/// role, recorded here because "peer trust" left undefined is exactly the
/// undefined trust that leaks, and because a reader looking for the limits
/// of a peering should find them beside its powers.
///
/// One §1.7 line is deliberately absent: `accumulatedCost` relayed with
/// this hop's own fee added. It is a peer grant in §1.7's list, but it is
/// not role-*discriminating* — a client interaction accumulates cost on a
/// reject too (ADR 0011), so a `Capability` returning `false` for a client
/// would assert something untrue. It is named in §1.7 because a peer hop
/// adds its own fee to a figure it received, not because the field is
/// refused to a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    /// Judge a voucher as a peering's payment and answer it in a
    /// `claim-ack`. Peer only in its verdict's destination: the voucher is
    /// held to its channel's **one** watermark whichever role it arrives
    /// under (§1.8, ADR 0075 decision 6), and a client interaction
    /// presenting a voucher from a bound signer's channel is still a client
    /// unless the voucher itself proved the role -- §1.3 forbids inferring
    /// it from "a channel that happens to be bound".
    AdvancePeerWatermark,
    /// Append to the peer claim ledger.
    AppendToPeerClaimLedger,
    /// Emit a `claim-ack` / `Toon-Claim-Ack` on a response. A connector
    /// MUST NOT emit one on a client interaction. See
    /// [`claim_ack_to_emit`].
    EmitClaimAck,
    /// Accept a FLUSH (§6). No peering sends one since ADR 0075 (#1380):
    /// a voucher rides the PREPARE it covers.
    AcceptFlush,
    /// Be a route's next hop: this peering relation may appear in the
    /// routing table as somewhere to forward to. Not the same as having
    /// one's packets forwarded, which a client's are — the discriminating
    /// half is being a *destination* the table can name.
    BeARouteNextHop,
    /// Be eligible for §6.4's flush prompt (`Toon-Flush-Requested`). A
    /// client interaction is never treated as a peering relation for flush
    /// purposes. Named for the credit-window exposure/ceiling accounting
    /// this capability originally gated too; that machinery is retired
    /// (ADR 0031, ADR 0033, issue #882), and the prompt itself is no longer
    /// emitted since ADR 0075 (#1380).
    CountTowardPeeringExposure,

    /// Carriage without paying for it. Granted to neither role: a peering
    /// is not a discount.
    FreeCarriage,
    /// A route the routing table does not have. Neither role conjures one.
    RouteNotInRoutingTable,
    /// Any operator or admin surface (ADR 0008). A peering is a money
    /// relationship, not an administrative one.
    OperatorSurface,
    /// Exemption from sealing (§8). A peer's packets are sealed to the
    /// terminating connector exactly as a client's are (ADR 0018).
    ExemptionFromSealing,
    /// A say in this connector's fees or a route's price (ADR 0020).
    SetRoutePrice,
    /// Opening the payload of a packet it forwards (ADR 0016).
    OpenForwardedPayload,
}

impl Capability {
    /// Every capability, so a test can assert the whole table rather than
    /// the rows someone remembered to write.
    pub const ALL: &'static [Capability] = &[
        Capability::AdvancePeerWatermark,
        Capability::AppendToPeerClaimLedger,
        Capability::EmitClaimAck,
        Capability::AcceptFlush,
        Capability::BeARouteNextHop,
        Capability::CountTowardPeeringExposure,
        Capability::FreeCarriage,
        Capability::RouteNotInRoutingTable,
        Capability::OperatorSurface,
        Capability::ExemptionFromSealing,
        Capability::SetRoutePrice,
        Capability::OpenForwardedPayload,
    ];
}

/// The claim acknowledgement this interaction's response may carry.
///
/// `None` for a client, always: §1.7 forbids a `claim-ack` /
/// `Toon-Claim-Ack` on a client response. Routing the ack through this
/// function makes "a client response never carries a claim-ack" a thing
/// the code does rather than a thing the code remembers.
#[must_use]
pub fn claim_ack_to_emit<T>(role: &SessionRole, acknowledgement: Option<T>) -> Option<T> {
    if role.grants(Capability::EmitClaimAck) {
        acknowledgement
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peer_holds_every_peer_grant_and_a_client_holds_none_of_them() {
        let peer = SessionRole::peer("store-box");
        let client = SessionRole::Client;

        for capability in [
            Capability::AdvancePeerWatermark,
            Capability::AppendToPeerClaimLedger,
            Capability::EmitClaimAck,
            Capability::AcceptFlush,
            Capability::BeARouteNextHop,
            Capability::CountTowardPeeringExposure,
        ] {
            assert!(peer.grants(capability), "peer should grant {capability:?}");
            assert!(
                !client.grants(capability),
                "client must be refused {capability:?} (peer-carriage-spec.md §1.7)"
            );
        }
    }

    #[test]
    fn the_capabilities_no_role_grants_are_granted_to_no_role() {
        let peer = SessionRole::peer("store-box");
        let client = SessionRole::Client;

        for capability in [
            Capability::FreeCarriage,
            Capability::RouteNotInRoutingTable,
            Capability::OperatorSurface,
            Capability::ExemptionFromSealing,
            Capability::SetRoutePrice,
            Capability::OpenForwardedPayload,
        ] {
            assert!(
                !peer.grants(capability),
                "peer role does NOT grant {capability:?} (peer-carriage-spec.md §1.7)"
            );
            assert!(!client.grants(capability));
        }
    }

    /// The table has to cover every capability, or a capability added
    /// without a decision would be untested rather than undecided.
    #[test]
    fn every_capability_is_covered_by_the_two_tables_above() {
        let covered = [
            Capability::AdvancePeerWatermark,
            Capability::AppendToPeerClaimLedger,
            Capability::EmitClaimAck,
            Capability::AcceptFlush,
            Capability::BeARouteNextHop,
            Capability::CountTowardPeeringExposure,
            Capability::FreeCarriage,
            Capability::RouteNotInRoutingTable,
            Capability::OperatorSurface,
            Capability::ExemptionFromSealing,
            Capability::SetRoutePrice,
            Capability::OpenForwardedPayload,
        ];

        assert_eq!(covered.len(), Capability::ALL.len());
        for capability in Capability::ALL {
            assert!(covered.contains(capability), "uncovered: {capability:?}");
        }
    }

    #[test]
    fn a_client_response_never_carries_a_claim_ack() {
        assert_eq!(
            claim_ack_to_emit(&SessionRole::Client, Some("accepted")),
            None
        );
        assert_eq!(
            claim_ack_to_emit(&SessionRole::peer("store-box"), Some("accepted")),
            Some("accepted")
        );
    }

    #[test]
    fn a_role_names_itself_for_metrics_and_logs() {
        assert_eq!(SessionRole::Client.name(), "client");
        assert_eq!(SessionRole::peer("store-box").name(), "peer");
        assert_eq!(SessionRole::peer("store-box").peer_id(), Some("store-box"));
        assert_eq!(SessionRole::Client.peer_id(), None);
        assert!(SessionRole::Client.is_client());
        assert!(!SessionRole::Client.is_peer());
    }
}
