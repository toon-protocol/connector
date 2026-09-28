//! Role-by-authentication: whether an interaction is a **peer** or a
//! **client**, decided from the claim it carries and configuration alone
//! (`docs/protocol/peer-carriage-spec.md` §1, issue #726).
//!
//! ADR 0026 could prove role by construction — peers spoke a different
//! protocol on a different listener, so no client trust could leak onto a
//! peer session and no peer trust onto a client one. ADR 0027 spent that
//! proof: peers now ride the same two carriages clients do, and what
//! replaces the proof is this crate. Everything here is a stop-ship
//! invariant.
//!
//! # The rule (§1.2)
//!
//! An interaction has role `peer` **if and only if both** hold:
//!
//! * **P2 — a channel binding.** The frame's claim names a `channel_id`
//!   that one of a configured peering's `[[peer_channels]]` rows binds.
//! * **P3 — a verified claim signature.** That claim's signature verifies
//!   against **the counterparty key that row configures** — never against
//!   anything the claim declares about itself.
//!
//! If either fails, for any reason, the role is `client`. There is no
//! fallthrough, no degraded peer, no third state: [`SessionRole`] has two
//! variants and no `Unknown`.
//!
//! **There is no P1, and no bearer credential anywhere in this crate.**
//! Until ADR 0060 a `{peerId, secret}` shared secret decided role, and the
//! weaker check gated the stronger one: a peering whose secret was stale
//! was downgraded to `client` on the strength of a shared string, while the
//! signature that actually proves who it is was never consulted. That
//! credential is deleted rather than renamed, demoted to a label or kept as
//! an optional discriminator — a second identifier for a relation the claim
//! already names is the fault ADR 0060 removes, and rebuilding it smaller
//! would be the same fault.
//!
//! # Why the decision cannot see the transport (§1.3)
//!
//! §1.3 forbids inferring role from the carriage, the listener, the port,
//! the bind address, the source address, the TLS SNI name, a client
//! certificate, the `btp` subprotocol, an endpoint appearing in
//! `[[peers]]`, the shape of what was sent, or anything the interaction —
//! or another interaction from the same address — did earlier.
//!
//! That is enforced here structurally rather than by review, in three
//! layers:
//!
//! 1. **The signature.** [`decide_role`] takes exactly
//!    `(Option<PresentedClaim>, &PeerAuthPolicy)` and returns a
//!    [`RoleDecision`]. A [`PresentedClaim`] holds a channel id and a
//!    [`ClaimVerification`] and nothing else; a [`PeerAuthPolicy`] is built
//!    from configuration and nothing else. Neither type has a field a port
//!    number, a peer address or a carriage could be smuggled in, so a
//!    caller wanting to weight one has nowhere to put it.
//! 2. **The dependency graph.** This crate depends on `connector-config`
//!    and nothing else. It depends on no async runtime, no HTTP or
//!    websocket stack, and no other connector crate — so it cannot name a
//!    socket, a request, a session or a frame even privately.
//!    [`tests::the_decision_crate_cannot_name_a_transport`] asserts that
//!    against the manifest, so a future dependency has to argue with a
//!    failing test. It is also why the signature *verdict* arrives as a
//!    [`ClaimVerification`] rather than being computed here: verifying one
//!    needs the counterparty key the claim book holds, and reaching for it
//!    would mean depending on the runtime.
//! 3. **No I/O and no clock.** Nothing here reads a socket, a file or the
//!    time. Every function is pure, which is what makes the whole surface
//!    testable without one (ADR 0007: fakes yes, mocks no) — the rate limit
//!    on the operator event takes `now_ms` as an argument rather than
//!    reading a clock.
//!
//! There is deliberately **no** "trusted network" or "loopback is a peer"
//! escape hatch. Every such shortcut is transport inference wearing a
//! different hat, and §1.3 does not have an exception for the convenient
//! ones. The one escape hatch ADR 0027 does name — a dedicated peer
//! listener (§1.10) — is defence in depth that changes *nothing* here: role
//! is still decided by P2 and P3, and the listener only changes what a
//! carriage does with a `client` verdict on it.
//!
//! # What this crate is not
//!
//! It is not a carriage. The BTP peer carriage (issue #727) and the
//! ILP-over-HTTP one (issue #728) own sessions, frames, headers, requests
//! and responses, and they own decoding a claim off their own wire. What
//! reaches here is the two facts §1.2 reads.
//!
//! # Layout
//!
//! | Module | What it holds |
//! | ------ | -------------- |
//! | [`policy`] | the configured side: which channel belongs to which peering |
//! | [`decision`] | [`decide_role`], its verdict, and the `peer_auth_refused` operator event |
//! | [`role`] | the two roles and the containment enumeration |

pub mod decision;
pub mod policy;
pub mod role;

pub use decision::{
    decide_role, decide_voucher_role, ClaimVerification, PeerAuthRefusal, PeerAuthRefusalLog,
    PeerAuthRefusalReport, PresentedClaim, PresentedVoucher, RoleDecision, UnmetRequirement,
    VoucherVerification, PEER_AUTH_REFUSED_EVENT,
};
pub use policy::PeerAuthPolicy;
pub use role::{claim_ack_to_emit, Capability, SessionRole};

#[cfg(test)]
mod tests {
    /// Crates whose presence in this crate's `[dependencies]` would mean
    /// the role decision *could* see something §1.3 forbids it from
    /// weighing. The list is the transports and runtimes this workspace
    /// actually uses, plus the connector crates that own a wire or the
    /// claim book.
    ///
    /// This is the mechanical half of the property; the other half is
    /// [`crate::decide_role`]'s signature, which has nowhere to put a
    /// transport fact even if one were reachable.
    const TRANSPORT_CRATES: &[&str] = &[
        "tokio",
        "axum",
        "hyper",
        "reqwest",
        "tungstenite",
        "tower",
        "h2",
        "rustls",
        "socket2",
        "mio",
        "connector-btp",
        "connector-client-edge",
        "connector-runtime",
        "connector-operator",
    ];

    /// The `[dependencies]` section only. There are no `[dev-dependencies]`
    /// left to exempt — the one that existed named `connector-btp` so the
    /// credential's protocolData entry could not fork from the frame
    /// grammar's spelling, and ADR 0060 deleted the credential.
    fn declared_dependencies() -> String {
        let manifest = include_str!("../Cargo.toml");
        let mut inside = false;
        let mut collected = String::new();
        for line in manifest.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                inside = trimmed == "[dependencies]";
                continue;
            }
            // Comments are prose about the dependencies, not dependencies;
            // this test would otherwise forbid explaining itself.
            if inside && !trimmed.starts_with('#') {
                collected.push_str(trimmed);
                collected.push('\n');
            }
        }
        collected
    }

    #[test]
    fn the_decision_crate_cannot_name_a_transport() {
        let dependencies = declared_dependencies();

        for forbidden in super::tests::TRANSPORT_CRATES {
            assert!(
                !dependencies.contains(forbidden),
                "connector-peer-auth grew a dependency on `{forbidden}`. Role is decided by \
                 the frame's verified claim and the config, never by the transport \
                 (peer-carriage-spec.md §1.3): a crate that can name a socket, a session or a \
                 frame is a crate where someone can weight one. Dependencies were:\n\
                 {dependencies}"
            );
        }
    }

    /// Guards the guard: a manifest whose `[dependencies]` section this
    /// parser fails to find would pass the assertion above vacuously.
    #[test]
    fn the_transport_guard_reads_a_non_empty_dependency_section() {
        let dependencies = declared_dependencies();

        assert!(
            dependencies.contains("connector-config"),
            "the dependency-section parser found nothing, so the transport guard proves \
             nothing; got:\n{dependencies}"
        );
    }
}
