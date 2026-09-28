//! The **ILP-over-HTTP peer carriage** (`docs/protocol/peer-carriage-spec.md`,
//! ADR 0027, issue #728): peering with another connector by `POST` over
//! `https://` -- [`dial`] a peer's endpoint, [`accept`] a request a peer sent
//! us.
//!
//! # What this crate is, and what it deliberately is not
//!
//! It is the **carriage**: where the bytes ride. It is not the semantics.
//! Claim exchange, flush, fees and the refusal taxonomy
//! are `peer-semantics-pre-868.md` §3--§6's and live above the
//! [`connector_runtime::PeerTransport`] port, unchanged by which wire carried
//! them. This crate maps §3's table onto requests and responses, and nothing
//! else:
//!
//! | Concept | ILP-over-HTTP |
//! | ------- | ------------- |
//! | PREPARE | **POST**, OER PREPARE as the request body |
//! | FULFILL / REJECT | **200**, OER FULFILL/REJECT as the response body |
//! | piggybacked claim | `ILP-Payment-Channel-Claim` request header, `base64(JSON)` |
//! | **FLUSH** | **POST with an empty ILP body** plus the claim header -- the standalone-claim shape of `client-edge-spec.md` §1.9 step 5 |
//! | CLAIM_ACK | `Toon-Claim-Ack` response header on the response that already answers the claim-bearing request |
//! | `accumulatedCost` | `Toon-Accumulated-Cost` response header, on a REJECT only |
//! | flush prompt | `Toon-Flush-Requested` response header -- HTTP only, and only a hint (§6.4) |
//!
//! There is no row for a peer credential, and there was one: `Toon-Peer-Auth`
//! carried a `base64({peerId, secret})` on every request. ADR 0060 deleted
//! it -- the claim already in the table above is what proves the peering, so
//! this carriage neither sets that header nor reads one. An arriving one is
//! ignored rather than refused, which is what lets the two ends of a peering
//! be upgraded in either order.
//!
//! **The claim header is `ILP-Payment-Channel-Claim`**, the deployed
//! spelling. ADR 0027's table originally wrote `Payment-Channel-Claim`,
//! mirroring the BTP entry name; §12.1 pinned the deployed name instead
//! under the ADR's own "one codec, reused verbatim" rule -- a new header name
//! would need a second decoder on the HTTP path -- and the ADR was amended.
//!
//! # The two carriage properties, and everything else is a defect
//!
//! §9 is blunt: any peer behaviour on one carriage and not the other is a
//! defect, **except** the two this document names. Both of them are HTTP's:
//!
//! 1. **Origination is one-way** (§2.3, §6.4). On BTP a dialed session is
//!    symmetric once established; on HTTP only the dialing side can
//!    originate. Debt flows with packets and packets flow only in the dialing
//!    direction, so on a one-way-dialed HTTP peering the dialing side is
//!    structurally the **payer** and the accept-only side is structurally the
//!    **payee**. The consequence that bites is at *configuration* time and is
//!    unidirectional packet flow -- not, as ADR 0027 originally put it, a lost
//!    flush bound (§12.3). Before ADR 0031/ADR 0033 (issue #882) what
//!    replaced the bound was an explicit exposure ceiling, refused at load
//!    when absent (#723's `AcceptOnlyPeerWithoutCeiling`); that requirement
//!    is retired along with the credit window it protected, leaving only
//!    §6.4's hint.
//! 2. **Claims can race** (§7.2). Parallel requests carrying nonces *n* and
//!    *n+1* reach the payee's watermark lock in either order. The mitigation
//!    is the client edge's: no more than one claim-bearing request in flight
//!    per channel ([`dial`]).
//!
//! And the corollary an operator meets first (§2.4): **an HTTP-only peer can
//! neither reach nor be reached by a NAT'd peer.** The NAT'd side can only
//! dial, and can only receive over a persistent session, so that session must
//! be BTP. It is a property of this carriage, not a defect scheduled for
//! repair -- and it is the least obvious thing here to diagnose, so
//! [`dial::NAT_NOTE`] rides every refusal and log line this crate produces.
//!
//! # Three things this crate must not do, and how it cannot
//!
//! 1. **Fork a name.** Every header comes from
//!    [`connector_btp::CARRIAGE_NAMES`], where it is declared *as a pair*
//!    with its protocolData twin (spec I2). There is no header string in this
//!    crate.
//! 2. **Fork the claim, the ack or the role decision.**
//!    [`connector_peer_btp::claim_json`] parses the claim (I4, through the
//!    client edge's own validator), [`connector_peer_btp::ack`] encodes and
//!    decodes the verdict (I3, one refusal taxonomy), and
//!    [`connector_peer_btp::role_gate::decide`] decides role (I7) -- the
//!    same call the BTP carriage makes, over the same claim. This crate
//!    calls them.
//! 3. **Blur the two audiences.** The pipeline below the port is shared with
//!    the client edge; **the admission is not**. See [`accept`] -- the devnet
//!    incident §1.9 names is what happens when a client is admitted as a
//!    quasi-peer, and reusing `POST /ilp`'s handler wholesale is how that
//!    happens again.
//!
//! # Why this crate depends on `connector-peer-btp`
//!
//! Because the modules it borrows are not BTP. The claim codec, the ack
//! codec, the canonical channel-id form and the per-relation
//! [`AcceptedClaims`](connector_peer_btp::AcceptedClaims) ledger are carriage
//! *semantics* that happen to have landed in #727's crate first, and I3/I4/I6
//! require exactly one of each. Copying them here would be the drift those
//! invariants exist to prevent; lifting them into a shared
//! `connector-peer-role` crate would be a better home and is mechanical, but
//! it is a refactor of a crate that merged days ago and it changes no
//! behaviour, so it is not done under this issue.
//!
//! # What is not here
//!
//! Listener wiring (issue #678's bring-up: this crate answers a
//! [`PeerRequest`](headers::PeerRequest) and never opens a port), the paired
//! `peer_carriage` vectors and the five stop-ship regressions (issue #729).
//! Two limits are inherited from #727 rather than diverged from: a Solana
//! peer claim is refused `UnsupportedChain` because `ClaimBook` verifies
//! EIP-712 balance proofs only, and the re-ack record is per-process -- a
//! restart loses it, and recovering it belongs with the claim journal's own
//! durability (ADR 0005), not with a carriage.

pub mod accept;
pub mod client;
pub mod dial;
pub mod headers;

pub use accept::{claim_on, evidence_on, FlushHints, PeerHttpPolicy, PeerHttpState};
pub use client::ReqwestPeerClient;
pub use dial::{HttpDialError, HttpPeerTransport, PeerHttpClient, PeerRelation, NAT_NOTE};
pub use headers::{Headers, PeerRequest, PeerResponse};
