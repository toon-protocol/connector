//! The client BTP websocket carriage (client-edge-spec.md §1.9, ADR 0026):
//! one persistent, ordered websocket session carrying BTP-framed ILP packets
//! and claims through exactly the pipeline `handle_ilp` runs per request --
//! the same `ClientClaimGate` instance, the same watermarks and journal, the
//! same refusal taxonomy (`claim_rejection_reject`), the same
//! `Connector::handle_prepare`.
//!
//! This module is the client edge's *policy* on a BTP session. The frame
//! grammar itself is not here: it lives in [`connector_btp`], transport- and
//! role-neutral, so the peer carriage of ADR 0027 (issue #676) can speak the
//! same bytes without reaching any of the client-edge types below (issue
//! #713). The deployed `@toon-protocol/client` dialect and RFC-0023's full
//! symmetric grammar are one codec there, not two -- see that crate's own
//! header. What distinguishes this carriage is only what it *does* with a
//! decoded frame: it answers, and originates nothing. Every inbound MESSAGE
//! path a deployed client exercises is unchanged byte for byte.
//!
//! **Ordering** (issue #688): *claims* on one session are judged strictly
//! sequentially, in arrival order -- the session task itself runs every
//! frame's decoding, greeting and claim admission before touching the next
//! frame, which is what makes in-order claims on one socket unable to race
//! each other into `AmountNotAdvancing`, and it is the one ordering the
//! carriage exists to provide. What is *not* serialized any more is the
//! judged frame's remaining work -- waiting out the journal group commit's
//! fsync (issue #686), the downstream delivery, sending the RESPONSE --
//! which proceeds under a bounded per-session in-flight window
//! (`btp_session_window`, default [`crate::DEFAULT_BTP_SESSION_WINDOW`]).
//! Lockstep frame processing made every paid write cost a full downstream
//! round-trip of session capacity, the measured ~125-150 events/s
//! per-session admission wall. Responses therefore complete in whatever
//! order their downstream answers; that is the dialect's own contract --
//! `requestId` correlates a RESPONSE to its MESSAGE, and the deployed
//! client resolves binary frames through its `pendingRequests` map by
//! exactly that id, never by arrival order.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use connector_btp::{
    decode_frame, encode_error, encode_response, reply, BtpDecodeError, BtpSessionHandle,
    OutboundRequests, ProtocolData, SessionGone, ACCUMULATED_COST_PROTOCOL, AUTH_PROTOCOL,
    BTP_ERROR, BTP_MESSAGE, BTP_RESPONSE, BTP_TRANSFER, CLAIM_PROTOCOL, CONTENT_TYPE_TEXT,
    PAYMENT_REQUIRED_PROTOCOL, PAYOUT_CLAIM_PROTOCOL,
};
use connector_domain::client_claim::ClientClaim;
use connector_domain::{PacketResponse, Prepare, Price, Reject, RejectCode};
use connector_peer_btp::challenge_json::{self, PeerRoleChallenge};
use connector_peer_btp::role_gate::{challenge_in_window, VoucherCheck, VoucherEvidence};
use connector_settlement::batch::ChannelPresentation;

use crate::claim_gate::DurabilityTicket;
use crate::outbound_ledger::PayoutVoucher;
use crate::peer::BtpClaimVerdict;
use crate::{claim_rejection_reject, x402_terms_body, ClaimIngestRejection, ClientEdgeState};

/// `GET /ilp/btp` -- the upgrade. The `btp` subprotocol is selected when
/// the client offers it; an upgrade offering none is accepted identically
/// (the deployed `IsomorphicBtpClient` offers none). Nothing about the
/// session is trusted from the handshake -- authorization to write comes
/// from each frame's claim, exactly as on HTTP.
pub(crate) async fn handle_btp_upgrade(
    State(state): State<Arc<ClientEdgeState>>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.protocols(["btp"])
        .on_upgrade(move |socket| btp_session(socket, state))
}

/// How many completed replies may queue for the socket's writer task
/// before a finishing frame waits its turn -- burst smoothing between the
/// out-of-order completions and the one socket, not admission control
/// (that is the in-flight window's job).
const REPLY_QUEUE_DEPTH: usize = 32;

/// One session (issue #688): the session task reads frames and runs
/// everything order-sensitive inline -- decoding, the greeting, and above
/// all **claim admission**, so one socket's claims are judged strictly in
/// arrival order, the carriage's contract (§1.9). Everything a frame owes
/// *after* its claim is judged -- the group-commit durability wait, the
/// downstream delivery, the RESPONSE -- runs in a task under the
/// per-session in-flight window, so a session's throughput is bounded by
/// `window / downstream-latency` instead of `1 / downstream-latency`. The
/// window doubles as backpressure: when it is full, the session task
/// blocks before reading further frames, exactly the lockstep behavior,
/// degraded to gracefully rather than defaulted to.
///
/// Text frames are ignored; ping/pong is the websocket layer's own
/// concern; a transport error or close ends the session. Claims judged
/// here advance the same watermarks HTTP requests advance, so a session
/// outliving many requests changes nothing about what any one claim must
/// prove. A frame task that outlives the session finishes its durability
/// wait and delivery normally -- the claim was accepted -- and its reply
/// simply has nowhere to go.
async fn btp_session(socket: WebSocket, state: Arc<ClientEdgeState>) {
    let (sink, mut stream) = socket.split();
    // The one writer: frame tasks complete in whatever order their
    // downstream answers, and this channel is where those completions
    // serialize back into socket writes.
    let (replies, mut reply_rx) = mpsc::channel::<Vec<u8>>(REPLY_QUEUE_DEPTH);
    let writer = tokio::spawn(async move {
        let mut sink = sink;
        while let Some(bytes) = reply_rx.recv().await {
            if sink.send(Message::Binary(bytes)).await.is_err() {
                break;
            }
        }
    });
    let window = Arc::new(Semaphore::new(state.btp_session_window.get() as usize));
    // This session's outbound requestId space and RESPONSE/ERROR
    // correlation table (issue #697). Always live, whether or not anything
    // ever originates a request on it -- with nothing pending, every
    // inbound RESPONSE/ERROR still resolves to nothing, exactly as before
    // this table existed.
    let outbound = Arc::new(OutboundRequests::new());
    // This session's binding in the client session registry (issue #698),
    // if auth has named one: the address it registered as and the fencing
    // generation that bind returned. `None` until an auth frame with a
    // usable `peerId` arrives, and for the lifetime of a session that
    // never sends one -- unbound, exactly as every session behaved before
    // this registry existed.
    let mut binding: Option<(String, u64)> = None;
    // ADR 0027 / issue #678: this socket serves two audiences. A session
    // starts as a client's and becomes a peer session when a frame arrives
    // carrying a claim that proves §1.2's P2 *and* P3. Once it does, every
    // remaining frame is the peer carriage's -- which re-decides role from
    // each one, because role is a property of the frame (§1.5). Frames
    // processed before the handover stay client frames and are never
    // retroactively reclassified.
    let mut peer_session: Option<connector_peer_btp::PeerSession> = None;
    while let Some(received) = stream.next().await {
        let frame_bytes = match received {
            Ok(Message::Binary(bytes)) => bytes,
            Ok(Message::Close(_)) => break,
            Ok(_) => continue,
            Err(_) => break,
        };
        if let Some(session) = peer_session.as_mut() {
            match session.handle_frame(&frame_bytes).await {
                Ok(None) => continue,
                // The peer carriage ended the session deliberately, or the
                // socket's send half is gone.
                Ok(Some(_)) | Err(_) => break,
            }
        }
        match peer_handover(&frame_bytes, &state).await {
            // Not consumed: the same frame is handed to the peer session,
            // which decides its own role from it and answers it.
            Some(BtpClaimVerdict::Peer) => {
                let peer_state = state
                    .peers
                    .as_ref()
                    .and_then(|peers| peers.btp_state())
                    .expect("a Peer verdict is only reachable with a BTP carriage mounted");
                let mut session = connector_peer_btp::PeerSession::with_outbound(
                    peer_state,
                    replies.clone(),
                    Arc::clone(&outbound),
                );
                match session.handle_frame(&frame_bytes).await {
                    Ok(None) => {}
                    Ok(Some(_)) | Err(_) => break,
                }
                peer_session = Some(session);
                continue;
            }
            Some(BtpClaimVerdict::Client) | None => {}
        }
        if handle_frame(
            &frame_bytes,
            &state,
            &window,
            &replies,
            &outbound,
            &mut binding,
        )
        .await
        .is_err()
        {
            // The socket's send half is gone; nothing further this session
            // reads could ever be answered.
            break;
        }
    }
    // Clear this session's own binding, fenced against a reconnect that
    // has already superseded it (issue #698's "may never say take over"):
    // `unbind` is a no-op if `binding`'s generation is no longer current.
    if let Some((address, generation)) = binding {
        end_session(&state, &address, generation);
    }
    drop(replies);
    let _ = writer.await;
}

/// Whether this frame's claim hands the rest of the session to the peer
/// carriage (`peer-carriage-spec.md` §1.2, §1.5, issue #678) -- or its
/// voucher, or its peer-role challenge (ADR 0075 decision 5).
///
/// `None` for every frame carrying neither a claim nor a challenge entry,
/// and for every node that
/// mounts no BTP peer carriage -- which is the whole of what this costs a
/// client session: one `find` over a frame's protocolData, and on the
/// frames that do carry a claim, one signature check against a channel this
/// node has no `[[peer_channels]]` row for, which fails at the row lookup
/// before any curve arithmetic.
///
/// Both frame types are peeked, not only MESSAGE: a peering's FLUSH is a
/// TRANSFER, and it carries a claim exactly as a claim-bearing MESSAGE
/// does. Under the credential this could key on MESSAGE alone because the
/// `auth` entry only ever rode one.
///
/// The frame is **peeked, not consumed**. §1.3 forbids inferring role from
/// the listener, and this function inspects nothing but the frame's claim
/// and the configured policy; the peer session decides again from this very
/// frame.
async fn peer_handover(
    frame_bytes: &[u8],
    state: &Arc<ClientEdgeState>,
) -> Option<BtpClaimVerdict> {
    let peers = state.peers.as_ref()?;
    let frame = decode_frame(frame_bytes).ok()?;
    if frame.frame_type != BTP_MESSAGE && frame.frame_type != BTP_TRANSFER {
        return None;
    }
    let presents_evidence = frame.protocol_data.iter().any(|entry| {
        entry.name == connector_btp::CLAIM_PROTOCOL
            || entry.name == connector_btp::PEER_CHALLENGE_PROTOCOL
    });
    if !presents_evidence {
        return None;
    }
    Some(peers.btp_claim_verdict(&frame).await)
}

/// A slot in the session's in-flight window, holding the read loop back
/// once `btp_session_window` frames are past admission and not yet
/// answered.
async fn window_slot(window: &Arc<Semaphore>) -> OwnedSemaphorePermit {
    Arc::clone(window)
        .acquire_owned()
        .await
        .expect("the session window semaphore is never closed")
}

/// A claim's batch just cleared durability: mark its channel recognized,
/// making the sender eligible to probe (issue #548), and note the
/// claim-state endpoint's liveness timestamp (issue #693), same as
/// `handle_ilp`'s HTTP carriage. Shared by both places a BTP claim can
/// finish durable -- a standalone claim message and a claim riding a
/// packet -- so the two stay in lockstep.
///
/// `session` is this socket's own binding in
/// [`crate::session_registry::SessionRegistry`] -- its address and the
/// generation it was bound under -- `None` if this session never sent a
/// usable auth frame. When present, an accepted **voucher** -- already
/// fully verified by [`crate::claim_gate::ClientClaimGate::admit`] by the
/// time this runs -- teaches the gate who *this session* is paid as (issues
/// #787, #1381, #1396): its channel's voucher signer, as the chain records
/// it, is the key a payout channel toward this client names as receiver
/// (ADR 0075 decision 7). Every accepted claim is a voucher (#1384).
///
/// A voucher that changes the session's payee is also the moment a payout
/// already owed to that key -- stranded by an earlier session of the same
/// client that dropped before its delivery landed -- becomes deliverable to
/// this one, so it is resent here, exactly as a `channelChallenge` at auth
/// resends it ([`spawn_stranded_claim_resend`]).
fn record_accepted_claim(
    state: &Arc<ClientEdgeState>,
    claim: &ClientClaim,
    session: Option<&(String, u64)>,
) {
    let channel_key = claim.channel_key();
    state.connector.recognize_channel(&channel_key);
    state
        .claim_gate
        .note_claim_time(&channel_key, crate::now_unix());

    if let (Some((address, generation)), Some(payee)) =
        (session, state.claim_gate.voucher_signer(&channel_key))
    {
        if state
            .claim_gate
            .record_session_payee(address, *generation, payee)
        {
            spawn_stranded_claim_resend(state, address, *generation);
        }
    }
}

/// Close this socket's session `generation` at `address`: its binding in
/// the [`crate::session_registry::SessionRegistry`] -- fenced, so a
/// session already superseded by a reconnect cannot clear the newer
/// binding -- and its payee (issue #1396), which belongs to this session
/// alone and so is cleared whether or not it is still the current one.
fn end_session(state: &ClientEdgeState, address: &str, generation: u64) {
    state.session_registry.unbind(address, generation);
    state.claim_gate.close_session(address, generation);
}

/// Best-effort extraction of an auth frame's declared identity (issue
/// #698): the same `{"peerId": ..., "secret": ...}` shape the client
/// already sends (`auth_frame_vector` in this module's own tests), used
/// only as the session registry's bind key -- never as authorization,
/// which still comes solely from each frame's claim, exactly as the doc
/// comment on the auth branch above says. `None` for anything that does
/// not parse as JSON, carries no `peerId`, or declares an empty one; a
/// session with no usable declared identity is simply never bound.
fn auth_peer_id(auth_data: &[u8]) -> Option<String> {
    let json: serde_json::Value = serde_json::from_slice(auth_data).ok()?;
    let peer_id = json.get("peerId")?.as_str()?;
    if peer_id.is_empty() {
        None
    } else {
        Some(peer_id.to_string())
    }
}

/// The `auth` entry's fields that made up the retired `auth_channel_proof`
/// (issue #790): a flat `channelId`/`expires`/`signature` beside `peerId`,
/// signing the `TokenNetwork`-domain `ClaimStateChallenge`. ADR 0075
/// decision 5 replaced it with [`AUTH_CHANNEL_CHALLENGE_FIELD`] (issue
/// #1384); an `auth` entry still carrying any of these is refused by name
/// ([`RETIRED_AUTH_CHANNEL_PROOF`]), never read.
const RETIRED_AUTH_CHANNEL_PROOF_FIELDS: [&str; 3] = ["channelId", "expires", "signature"];

/// What the ERROR frame refusing a retired `auth_channel_proof` says.
const RETIRED_AUTH_CHANNEL_PROOF: &[u8] = b"auth_channel_proof is refused: ADR 0075 retired the \
    TokenNetwork-domain channel proof ('channelId'/'expires'/'signature' on the auth entry) -- \
    declare a channel with 'channelChallenge', the voucher claim-state challenge";

/// The `auth` entry's field that declares a channel before its session has
/// paid anything (ADR 0075 decision 5, issue #1384): the **voucher
/// claim-state challenge** -- exactly the object a `POST /ilp/claim-state`
/// entry and a peer's `peer-role-challenge` carry
/// (`connector_peer_btp::challenge_json`), with `scheme: "batch-settlement"`
/// required.
const AUTH_CHANNEL_CHALLENGE_FIELD: &str = "channelChallenge";

/// Whether an `auth` entry carries any field of the retired
/// `auth_channel_proof`. Not JSON, or not an object, carries none.
fn carries_retired_channel_proof(auth_data: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(auth_data)
        .ok()
        .and_then(|json| json.as_object().cloned())
        .is_some_and(|object| {
            RETIRED_AUTH_CHANNEL_PROOF_FIELDS
                .iter()
                .any(|field| object.contains_key(*field))
        })
}

/// Best-effort extraction of an `auth` entry's declared channel challenge,
/// alongside [`auth_peer_id`]'s extraction of the same entry's `peerId`.
/// `None` for an entry that is not JSON, carries no `channelChallenge`, or
/// whose challenge does not parse (`scheme` other than `batch-settlement`
/// included) -- a declaration that cannot be read teaches nothing.
fn auth_channel_challenge(auth_data: &[u8]) -> Option<PeerRoleChallenge> {
    let json: serde_json::Value = serde_json::from_slice(auth_data).ok()?;
    let challenge = json.get(AUTH_CHANNEL_CHALLENGE_FIELD)?;
    challenge_json::parse(challenge.to_string().as_bytes())
        .inspect_err(|error| {
            tracing::debug!(%error, "a BTP session's channel challenge could not be read");
        })
        .ok()
}

/// Verify a BTP session's declared channel challenge (issues #790, #1384)
/// and, if it verifies, teach
/// [`crate::claim_gate::ClientClaimGate::record_session_payee`] the
/// session's payee *before* the session has ever presented a voucher --
/// without this, a client that only ever earns (opens a channel toward this
/// node, serves paid work, pays nothing) could never be paid out.
///
/// The challenge is verified exactly as a peer's is
/// ([`VoucherEvidence::check_challenge`]): the channel it names is resolved
/// by the batch-settlement backend, its **voucher signer is read from the
/// chain** (EVM `payerAuthorizer`, Solana `authorized_signer`), and the
/// signature must recover to it -- so the payee taught is that signer, the
/// same key an accepted voucher on the channel teaches
/// (`record_accepted_claim`), and never anything the challenge itself
/// declares. Its `expires` must still be ahead and no further than
/// [`connector_peer_btp::role_gate::MAX_PEER_CHALLENGE_LIFETIME_SECS`]:
/// within it a challenge is a bearer proof, so it is kept short.
///
/// The payee is this session's -- the one `generation` names -- and no
/// other's (issue #1396): proving a channel here changes nothing about
/// where any other session at the same `peerId` is paid.
///
/// Best-effort and silent on any failure, the same posture as the `peerId`
/// bind it rides alongside: an expired, unresolvable or wrongly signed
/// challenge leaves this session's payee exactly where it was.
async fn verify_and_record_declared_channel(
    state: &ClientEdgeState,
    address: &str,
    generation: u64,
    challenge: PeerRoleChallenge,
) {
    if !challenge_in_window(challenge.expires(), crate::now_unix()) {
        tracing::debug!(
            address = %address,
            "a BTP session's channel challenge is expired or too far ahead -- ignoring it"
        );
        return;
    }
    let VoucherCheck::Verified(payee) = state.claim_gate.check_challenge(&challenge).await else {
        tracing::debug!(
            address = %address,
            channel = %challenge.channel(),
            "a BTP session's channel challenge did not verify -- ignoring it"
        );
        return;
    };
    state
        .claim_gate
        .record_session_payee(address, generation, payee);
    tracing::info!(
        address = %address,
        channel = %challenge.channel(),
        "a BTP session proved control of a channel at auth -- it can now be paid without \
         presenting a voucher of its own first"
    );
}

/// Issue #779: give a session that has just bound at `address` under
/// `generation` a chance to receive a payout claim stranded by an earlier
/// failed delivery, even with no new job in sight -- see
/// [`crate::session_route::deliver_pending_claim`] for what "stranded"
/// means and why there is nothing to do in the ordinary case.
///
/// Spawned rather than awaited: `deliver_pending_claim` waits out the
/// client's own answer to the TRANSFER it sends, and a slow or dead client
/// must never stall this session's auth ack -- nor any frame read after it,
/// since the read loop is what would eventually carry that answer.
fn spawn_stranded_claim_resend(state: &Arc<ClientEdgeState>, address: &str, generation: u64) {
    let state = Arc::clone(state);
    let address = address.to_string();
    tokio::spawn(async move {
        crate::session_route::deliver_pending_claim(
            &state,
            &address,
            generation,
            crate::now_unix(),
        )
        .await;
    });
}

/// A REJECT as a RESPONSE frame: the OER body as the ILP packet, the
/// running cost total as `toon-accumulated-cost` protocolData (§1.6's
/// header, carried the only way a frame can), plus whatever `extra`
/// entries the refusal itself owes (the F06 greeting's terms).
fn reject_response(request_id: u32, reject: Reject, extra: Vec<ProtocolData>) -> Vec<u8> {
    let mut protocol_data = vec![ProtocolData {
        name: ACCUMULATED_COST_PROTOCOL.to_string(),
        content_type: CONTENT_TYPE_TEXT,
        data: reject.accumulated_cost.to_string().into_bytes(),
    }];
    protocol_data.extend(extra);
    encode_response(request_id, &protocol_data, &reject.encode())
}

/// The protocolData entry a payout TRANSFER carries (ADR 0075 decision 7,
/// issue #1381): the payout **voucher**, as JSON, spelled exactly as a
/// client spells its own voucher claim (client-edge-spec.md §1.3) less the
/// envelope fields a claim carries for its payee's bookkeeping --
/// `blockchain`, `scheme: "batch-settlement"`, `channelId`,
/// `maxClaimableAmount` as a decimal string and `signature` in its chain's
/// spelling (`0x` + 130 hex on EVM, base58 of 64 bytes on Solana). An EVM
/// payout always carries its `channelConfig`, which `claim` needs and the
/// client may not have kept; a Solana one carries `expiresAt: 0`, which
/// its signed message commits to.
///
/// Everything a client needs to land the voucher on chain itself, with no
/// further word from this connector: the channel pays the client, so only
/// the client can land it.
///
/// This is the mapping of a *voucher* onto the grammar, so it stays with
/// the client edge rather than moving into [`connector_btp`] with the codec
/// (issue #713): the codec owns the entry's name and the bytes that carry
/// it, and deliberately cannot see a voucher type.
///
/// Production caller: `crate::session_route` (issue #770), once a client
/// session's own PREPARE genuinely fulfills, and on a session's (re)auth.
pub(crate) fn payout_voucher_protocol_data(payout: &PayoutVoucher) -> ProtocolData {
    let mut json = serde_json::json!({
        "scheme": "batch-settlement",
        "channelId": payout.channel_id(),
        "maxClaimableAmount": payout.cumulative_amount().to_string(),
    });
    match &payout.presentation {
        ChannelPresentation::Evm { config, .. } => {
            let address = |bytes: &[u8; 20]| format!("0x{}", hex::encode(bytes));
            json["blockchain"] = "evm".into();
            json["signature"] = format!("0x{}", hex::encode(&payout.voucher.signature)).into();
            json["channelConfig"] = serde_json::json!({
                "payer": address(&config.payer),
                "payerAuthorizer": address(&config.payer_authorizer),
                "receiver": address(&config.receiver),
                "receiverAuthorizer": address(&config.receiver_authorizer),
                "token": address(&config.token),
                "withdrawDelay": config.withdraw_delay,
                "salt": format!("0x{}", hex::encode(config.salt)),
            });
        }
        ChannelPresentation::Solana { .. } => {
            json["blockchain"] = "solana".into();
            json["expiresAt"] = 0.into();
            json["signature"] = bs58::encode(&payout.voucher.signature).into_string().into();
        }
    }
    ProtocolData {
        name: PAYOUT_CLAIM_PROTOCOL.to_string(),
        content_type: CONTENT_TYPE_TEXT,
        data: serde_json::to_vec(&json).expect("a json! object always serializes"),
    }
}

/// Process one frame: everything order-sensitive inline (claims are judged
/// here, in arrival order), the rest handed to a windowed task (issue
/// #688). Nothing is queued where the contract answers nothing: an
/// unrecognized frame type, a standalone claim (fire-and-forget by the
/// client's own contract), a MESSAGE carrying neither auth nor claim nor
/// packet, and a frame too short to even name a requestId.
///
/// `binding` is this session's own slot in the client session registry
/// (issue #698): `None` until an auth frame installs one, updated here on
/// auth and touched on every frame afterward so `btp_session`'s own
/// unbind-on-close has something to fence against.
async fn handle_frame(
    frame_bytes: &[u8],
    state: &Arc<ClientEdgeState>,
    window: &Arc<Semaphore>,
    replies: &mpsc::Sender<Vec<u8>>,
    outbound: &Arc<OutboundRequests>,
    binding: &mut Option<(String, u64)>,
) -> Result<(), SessionGone> {
    let frame = match decode_frame(frame_bytes) {
        Ok(frame) => frame,
        Err(BtpDecodeError::TooShort) => return Ok(()),
        Err(BtpDecodeError::Malformed { request_id, reason }) => {
            return reply(
                replies,
                encode_error(request_id, "F00", "NotAcceptedError", reason.as_bytes()),
            )
            .await;
        }
    };
    // A live session is still live (issue #698 AC5's backstop TTL clock),
    // whatever it just sent -- a no-op if `binding`'s generation has
    // already been superseded by a reconnect on another socket.
    if let Some((address, generation)) = binding.as_ref() {
        state
            .session_registry
            .touch(address, *generation, crate::now_unix());
    }
    // RESPONSE/ERROR answering a request *this connector* originated
    // (issue #697): correlate and stop, whether or not it matched --
    // ordinary inbound traffic that answers nothing the connector itself
    // sent (every RESPONSE/ERROR today, absent a caller of
    // `BtpSessionHandle`) leaves `resolve` a no-op and the frame silently
    // dropped, byte-identical to this session's pre-#697 behavior.
    if frame.frame_type == BTP_RESPONSE || frame.frame_type == BTP_ERROR {
        outbound.resolve(frame);
        return Ok(());
    }
    // TRANSFER (issue #697): acknowledged, not yet accounted -- RFC-23
    // requires a responder answer every request, and the settlement/netting
    // semantics this frame will eventually carry are the payout-ledger
    // ticket's job (toon-meta#262), not this foundation ticket's. An empty
    // RESPONSE is the same shape the `auth` ack below already uses for
    // "received, nothing more to say yet".
    if frame.frame_type == BTP_TRANSFER {
        return reply(replies, encode_response(frame.request_id, &[], &[])).await;
    }
    if frame.frame_type != BTP_MESSAGE {
        return Ok(());
    }

    // Auth (§1.9 step 1): acknowledged, not verified -- §1.2 is not
    // implemented on the HTTP carriage either, and an empty `secret` is the
    // documented permissionless mirror. Authorization to write comes from
    // the claim on each packet, never from the session.
    //
    // Issue #698: a declared, non-empty `peerId` doubles as this session's
    // key in the client session registry -- "the socket is the lease".
    // Binding is best-effort and never blocks the ack: a session with no
    // usable `peerId` (missing, empty, or the auth body not even JSON) is
    // simply never registered, unaffected otherwise, exactly as before
    // this registry existed. Re-auth on an already-bound session rebinds
    // under a fresh generation, which is the correct outcome even though
    // nothing sends a second auth today.
    if let Some(entry) = frame
        .protocol_data
        .iter()
        .find(|pd| pd.name == AUTH_PROTOCOL)
    {
        // ADR 0075 (issue #1384): the retired `auth_channel_proof` is refused
        // by name -- an ERROR frame, and no bind -- rather than ignored, so a
        // client still sending it learns why its channel is not declared.
        if carries_retired_channel_proof(&entry.data) {
            return reply(
                replies,
                encode_error(
                    frame.request_id,
                    "F00",
                    "NotAcceptedError",
                    RETIRED_AUTH_CHANNEL_PROOF,
                ),
            )
            .await;
        }
        if let Some(address) = auth_peer_id(&entry.data) {
            // A re-auth on this socket ends the session it replaces: nothing
            // else would, since only the newest binding is closed when the
            // socket goes.
            if let Some((previous, generation)) = binding.take() {
                end_session(state, &previous, generation);
            }
            let handle = BtpSessionHandle::new(replies.clone(), Arc::clone(outbound));
            let generation =
                state
                    .session_registry
                    .bind(address.clone(), handle, crate::now_unix());
            // A new session is paid nowhere until it proves a key itself
            // (issue #1396) -- whatever an earlier session that declared
            // the same `peerId` proved.
            state.claim_gate.open_session(&address, generation);
            if let Some(challenge) = auth_channel_challenge(&entry.data) {
                verify_and_record_declared_channel(state, &address, generation, challenge).await;
            }
            // After the channel challenge above, never before it: a session
            // whose payee this connector first learns at auth (issue #790)
            // has nothing to resend on until that challenge has been
            // recorded.
            spawn_stranded_claim_resend(state, &address, generation);
            *binding = Some((address, generation));
        }
        return reply(replies, encode_response(frame.request_id, &[], &[])).await;
    }

    let claim_json = match frame
        .protocol_data
        .iter()
        .find(|pd| pd.name == CLAIM_PROTOCOL)
    {
        Some(pd) => match String::from_utf8(pd.data.clone()) {
            Ok(json) => Some(json),
            Err(error) => {
                let rejection = ClaimIngestRejection::Malformed(format!(
                    "claim protocolData is not valid UTF-8: {error}"
                ));
                return reply(
                    replies,
                    reject_response(
                        frame.request_id,
                        claim_rejection_reject(rejection, 0),
                        Vec::new(),
                    ),
                )
                .await;
            }
        },
        None => None,
    };

    // A standalone claim (§1.9 step 4): admitted against price 0 --
    // identify-not-pay, exactly `handle_probe`'s semantics -- and answered
    // with nothing, per the client's `sendClaimMessage` contract. A refusal
    // here is logged, not framed: there is no response to carry it. The
    // admission is inline (in claim order, like every claim); only the
    // durability wait rides a windowed task, and a claim whose batch fails
    // is likewise only logged.
    if frame.ilp_packet.is_empty() {
        if let Some(json) = claim_json {
            match state.claim_gate.admit(&json, 0).await {
                Ok((claim, durability)) => {
                    let permit = window_slot(window).await;
                    let state = Arc::clone(state);
                    let session = binding.clone();
                    tokio::spawn(async move {
                        let _slot = permit;
                        match durability.durable().await {
                            Ok(()) => record_accepted_claim(&state, &claim, session.as_ref()),
                            Err(rejection) => tracing::debug!(
                                rejection = %rejection.message(),
                                "standalone BTP claim accepted but not durably recorded"
                            ),
                        }
                    });
                }
                Err(rejection) => {
                    tracing::debug!(
                        rejection = %rejection.message(),
                        "standalone BTP claim refused"
                    );
                }
            }
        }
        return Ok(());
    }

    let prepare = match Prepare::decode(&frame.ilp_packet) {
        Ok(prepare) => prepare,
        Err(error) => {
            // The HTTP carriage's 400 (§1.1): a transport-level answer, not
            // an ILP-level one, so an ERROR frame rather than a REJECT.
            return reply(
                replies,
                encode_error(
                    frame.request_id,
                    "F00",
                    "NotAcceptedError",
                    error.to_string().as_bytes(),
                ),
            )
            .await;
        }
    };

    // One lookup serves every fact (issue #701, ADR 0028): see
    // `handle_ilp`'s mirror of this on the HTTP carriage.
    let client_route = state.connector.client_route(&prepare.destination);
    // The schedule at this packet's own payload length (ADR 0065), exactly
    // as `handle_ilp` computes it on the HTTP carriage -- one rule, so a
    // sender is charged the same for the same packet whichever transport it
    // arrives on.
    let schedule = client_route
        .as_ref()
        .map_or(Price::FREE, |route| route.price);
    let charge = schedule.charge(prepare.data.len());
    // Issue #1210: see `handle_ilp`'s mirror of this on the HTTP carriage --
    // what a client should send to use this route, read off the same lookup
    // its price and transport policy come from.
    let request = client_route
        .as_ref()
        .and_then(|route| route.request.as_ref());
    // Issue #807 / #1269: see `handle_ilp`'s mirror of this on the HTTP
    // carriage -- a PREPARE that declares `greeting` is structurally a
    // bootstrap probe, never a real payment attempt, regardless of
    // destination.

    // Transport policy (issue #701, toon-meta#262 decision 11), BTP-shaped:
    // checked before payment is considered at all, exactly like the HTTP
    // carriage's `handle_ilp` -- a route restricted to HTTP is unreachable
    // over this session whether or not the PREPARE carries a valid claim.
    // F02 (Unreachable) is the honest code from this carriage's own point
    // of view: there is no route to this destination reachable over BTP,
    // even though one exists over HTTP. The terms JSON rides the same
    // `payment-required` protocolData slot the §1.4 greeting below uses,
    // self-diagnosing via `extra.requiredTransport` rather than a second
    // mechanism.
    if let Some(policy) = client_route.as_ref().map(|route| route.transport_policy) {
        if !policy.accepts_btp() {
            let terms = x402_terms_body(
                &prepare.destination,
                schedule,
                prepare.data.len(),
                &state.node,
                Some(policy.name()),
                request,
            );
            let reject = Reject {
                code: RejectCode::f02_unreachable(),
                triggered_by: String::new(),
                message: format!(
                    "route '{}' requires transport '{}'",
                    prepare.destination,
                    policy.name()
                ),
                data: Vec::new(),
                accumulated_cost: 0,
            };
            return reply(
                replies,
                reject_response(
                    frame.request_id,
                    reject,
                    vec![ProtocolData {
                        name: PAYMENT_REQUIRED_PROTOCOL.to_string(),
                        content_type: CONTENT_TYPE_TEXT,
                        data: terms,
                    }],
                ),
            )
            .await;
        }
    }

    // The §1.4 greeting, BTP-shaped (§1.9 step 3): BTP cannot answer HTTP
    // 402, so the same terms JSON rides as protocolData on an F06 REJECT.
    // A claimless PREPARE to an unpriced route falls through unchanged,
    // exactly as on HTTP -- unless the PREPARE itself declares `greeting`
    // (issue #807), the same broadening `handle_ilp` applies.
    if claim_json.is_none() && (charge > 0 || prepare.greeting) {
        let terms = x402_terms_body(
            &prepare.destination,
            schedule,
            prepare.data.len(),
            &state.node,
            None,
            request,
        );
        let reject = Reject {
            code: RejectCode::f06_unexpected_payment(),
            triggered_by: String::new(),
            message: "No payment channel claim attached".to_string(),
            data: Vec::new(),
            accumulated_cost: 0,
        };
        return reply(
            replies,
            reject_response(
                frame.request_id,
                reject,
                vec![ProtocolData {
                    name: PAYMENT_REQUIRED_PROTOCOL.to_string(),
                    content_type: CONTENT_TYPE_TEXT,
                    data: terms,
                }],
            ),
        )
        .await;
    }

    // ADR 0028's amount bound, BTP-shaped: the same rule as `handle_ilp`'s,
    // in the same place -- after the greeting, before the claim is admitted
    // -- so a packet this connector will not carry never spends the
    // client's watermark on either carriage (§9's no-drift invariant).
    if let Some(route) = client_route.as_ref() {
        if let Some(reject) =
            crate::over_carried_reject(&prepare.destination, route.kind, prepare.amount, charge)
        {
            return reply(
                replies,
                reject_response(frame.request_id, reject, Vec::new()),
            )
            .await;
        }
    }

    // §1.3, verbatim: the same gate, watermarks and refusal taxonomy as
    // `handle_ilp`. Admission -- the only order-sensitive step -- happens
    // here, inline; a refusal answers immediately, and an acceptance
    // carries its durability ticket into the windowed task below.
    //
    // Issue #869: mirrors `handle_ilp`'s own check, so the two carriages
    // cannot drift on this invariant (§9) -- a packet whose envelope will
    // be refused for its own target shape (`AppOutcome::Refused`, F00) is
    // never going to reach the app, however good the claim covering it
    // is, so that claim is left entirely unadmitted rather than spent on
    // a packet already known to be going nowhere. `finish_frame` below
    // still routes `prepare` unchanged and raises the identical F00
    // itself.
    let admitted = match claim_json {
        Some(json) if !crate::claim_must_not_be_admitted(state, &prepare) => {
            match state.claim_gate.admit(&json, charge).await {
                Ok(accepted) => Some(accepted),
                Err(rejection) => {
                    return reply(
                        replies,
                        reject_response(
                            frame.request_id,
                            claim_rejection_reject(rejection, charge),
                            Vec::new(),
                        ),
                    )
                    .await;
                }
            }
        }
        _ => None,
    };

    // Issue #1012: decided here, off the one route lookup above, because
    // `finish_frame` runs detached from it -- the same fact `handle_ilp`
    // reads inline on the HTTP carriage, through the same helper.
    let is_forwarded_route = crate::is_forwarded_route(client_route);
    let session = binding.clone();
    let permit = window_slot(window).await;
    let task = finish_frame(
        Arc::clone(state),
        admitted,
        MatchedRoute {
            prepare,
            charge,
            is_forwarded_route,
        },
        frame.request_id,
        replies.clone(),
        session,
    );
    tokio::spawn(async move {
        let _slot = permit;
        task.await;
    });
    Ok(())
}

/// [`finish_frame`]'s own `prepare` plus the two facts about its matched
/// route (issue #701, ADR 0028) that finishing it needs -- bundled so
/// `finish_frame` stays under clippy's argument-count lint rather than
/// growing a ninth loose parameter for issue #1012.
struct MatchedRoute {
    prepare: Prepare,
    /// What this connector charges for **this** packet: the route's schedule
    /// evaluated at its own payload length (ADR 0065), which for a flat route
    /// is the flat price it always was.
    charge: u64,
    /// Whether `prepare`'s destination matched a *forwarded* route --
    /// see [`roll_back_uncarried_forward`](crate::roll_back_uncarried_forward)'s
    /// own doc for why `finish_frame` needs to know.
    is_forwarded_route: bool,
}

/// A judged frame's remaining, order-insensitive work (issue #688), run
/// under one in-flight-window slot: wait for the claim's batch fsync
/// (durable-before-service, exactly `ingest`'s own contract -- the
/// downstream is never asked to do work whose payment a restart could
/// forget), then route the packet and queue the RESPONSE. A claim whose
/// batch could not be made durable answers the same `NotDurable` REJECT
/// the HTTP carriage sends, and its packet is never routed.
async fn finish_frame(
    state: Arc<ClientEdgeState>,
    admitted: Option<(ClientClaim, DurabilityTicket)>,
    matched: MatchedRoute,
    request_id: u32,
    replies: mpsc::Sender<Vec<u8>>,
    session: Option<(String, u64)>,
) {
    let MatchedRoute {
        prepare,
        charge,
        is_forwarded_route,
    } = matched;
    // Issue #535/ADR 0036: the channel a covering claim admitted this
    // packet on, read before `admitted` is consumed below, so it can ride
    // into the `"packet"` span the same way the HTTP carriage's `handle_ilp`
    // threads its own `admitted.channel_key` through.
    let client_channel_id = admitted.as_ref().map(|(claim, _)| claim.channel_key());
    // Issue #1012: the watermark this claim advanced its channel to, kept
    // for the same reason `handle_ilp`'s HTTP carriage keeps it -- so a
    // forwarded route whose next hop terminally rejects can roll back
    // exactly this claim.
    let admitted_claim_watermark = admitted
        .as_ref()
        .map(|(claim, _)| crate::AdmittedWatermark::of(claim));

    if let Some((claim, durability)) = admitted {
        match durability.durable().await {
            // A claim that cleared the gate makes the sender eligible to
            // probe and notes the claim-state endpoint's liveness
            // timestamp -- see `record_accepted_claim`.
            Ok(()) => record_accepted_claim(&state, &claim, session.as_ref()),
            Err(rejection) => {
                let _ = reply(
                    &replies,
                    reject_response(
                        request_id,
                        claim_rejection_reject(rejection, charge),
                        Vec::new(),
                    ),
                )
                .await;
                return;
            }
        }
    }

    // Issue #736: the same fourth routing arm `handle_ilp`'s HTTP carriage
    // uses -- a configured route first, then whatever client session
    // `state.session_registry` has bound to this destination.
    let packet_response =
        crate::session_route::route_prepare(&state, prepare, client_channel_id.as_deref()).await;
    crate::roll_back_uncarried_forward(
        &state,
        is_forwarded_route,
        client_channel_id.as_deref(),
        admitted_claim_watermark,
        &packet_response,
    )
    .await;
    let response = match packet_response {
        PacketResponse::Fulfill(fulfill) => encode_response(request_id, &[], &fulfill.encode()),
        PacketResponse::Reject(reject) => reject_response(request_id, reject, Vec::new()),
    };
    let _ = reply(&replies, response).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use connector_btp::{decode_frame, BtpFrame};
    use connector_settlement::batch::{EvmChannelConfig, Voucher, VoucherSigner};
    use connector_settlement::ChannelId;

    use crate::test_support;

    use crate::outbound_ledger::test_ledger_paying as ledger_paying;

    /// The committed cross-repo wire vectors (issue #1408): this module's
    /// own `client_auth_channel_challenge` cases, read back and checked
    /// against the real parser below.
    const WIRE_VECTORS: &str = include_str!("../../../vectors/wire-vectors.json");

    /// ADR 0075 decision 7: a payout rides a TRANSFER's protocolData as the
    /// payout voucher's JSON, spelled as a client spells its own voucher,
    /// with everything the client needs to land it: on EVM the config.
    #[test]
    fn a_payout_voucher_is_carried_as_the_json_a_client_lands_from() {
        let config = EvmChannelConfig {
            payer: [0x01; 20],
            payer_authorizer: [0x01; 20],
            receiver: [0x02; 20],
            receiver_authorizer: [0x02; 20],
            token: [0x70; 20],
            withdraw_delay: 86_400,
            salt: [0x66; 32],
        };
        let channel = format!("0x{}", "ab".repeat(32));
        let payout = PayoutVoucher {
            payee: VoucherSigner::Evm([0x02; 20]),
            presentation: ChannelPresentation::Evm {
                channel: ChannelId(channel.clone()),
                config,
            },
            voucher: Voucher {
                cumulative_amount: 750,
                signature: vec![0x11; 65],
            },
        };

        let pd = payout_voucher_protocol_data(&payout);

        assert_eq!(pd.name, PAYOUT_CLAIM_PROTOCOL);
        assert_eq!(pd.content_type, CONTENT_TYPE_TEXT);
        let json: serde_json::Value = serde_json::from_slice(&pd.data).expect("valid JSON");
        assert_eq!(json["blockchain"], "evm");
        assert_eq!(json["scheme"], "batch-settlement");
        assert_eq!(json["channelId"], channel);
        assert_eq!(json["maxClaimableAmount"], "750");
        assert_eq!(json["signature"], format!("0x{}", "11".repeat(65)));
        assert_eq!(
            json["channelConfig"]["receiver"],
            format!("0x{}", "02".repeat(20))
        );
        assert_eq!(json["channelConfig"]["withdrawDelay"], 86_400);
        assert!(json.get("nonce").is_none(), "a voucher has no nonce");

        let solana = PayoutVoucher {
            payee: VoucherSigner::Solana([0x02; 32]),
            presentation: ChannelPresentation::Solana {
                channel: ChannelId(bs58::encode([0xc3; 32]).into_string()),
            },
            voucher: Voucher {
                cumulative_amount: 42,
                signature: vec![0x22; 64],
            },
        };
        let json: serde_json::Value =
            serde_json::from_slice(&payout_voucher_protocol_data(&solana).data).expect("JSON");
        assert_eq!(json["blockchain"], "solana");
        assert_eq!(json["expiresAt"], 0);
        assert_eq!(
            json["signature"],
            bs58::encode([0x22; 64]).into_string(),
            "a Solana voucher's signature is base58, as x402 spells it"
        );
        assert!(json.get("channelConfig").is_none());
    }

    /// ADR 0075 decision 8 (issue #1384): a client frame whose claim entry is
    /// the retired `toon-channel` claim is refused by name -- a REJECT whose
    /// message names the retirement -- and teaches the session nothing.
    #[tokio::test]
    async fn a_toon_channel_claim_frame_is_refused_by_name() {
        let gate = test_support::voucher_gate().with_payout_ledger(
            ledger_paying(test_support::address_of(&test_support::authorizer())).await,
        );
        let mut state = test_state(gate);
        // A destination something serves: a claim to one nothing serves is
        // never looked at (issue #1446), so would never be refused by name.
        state.connector = Arc::new(connector_runtime::Connector::new(
            vec![connector_config::StaticRoute::new("g.nowhere", "http://localhost:4000").unwrap()],
            vec![],
            Arc::new(connector_runtime::FakeAppClient::new()),
            Arc::new(connector_runtime::InProcessPeerTransport::new()),
            Arc::new(connector_runtime::TestClock::new(
                chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2030, 1, 1, 0, 0, 0).unwrap(),
            )),
        ));
        let state = Arc::new(state);
        let prepare = Prepare {
            amount: 0,
            expires_at: chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2031, 1, 1, 0, 0, 0)
                .unwrap(),
            greeting: false,
            destination: "g.nowhere".to_string(),
            data: Vec::new(),
        };
        let frame = connector_btp::encode_message(
            1,
            &[ProtocolData {
                name: CLAIM_PROTOCOL.to_string(),
                content_type: CONTENT_TYPE_TEXT,
                data: test_support::toon_channel_claim().into_bytes(),
            }],
            &prepare.encode(),
        );

        let (replies, mut reply_rx) = mpsc::channel::<Vec<u8>>(REPLY_QUEUE_DEPTH);
        let window = Arc::new(Semaphore::new(4));
        let outbound = Arc::new(OutboundRequests::new());
        let mut binding = Some(("g.toon.agent".to_string(), 1));
        handle_frame(&frame, &state, &window, &replies, &outbound, &mut binding)
            .await
            .expect("the reply channel has a live receiver");

        let sent = reply_rx.recv().await.expect("a reply was sent");
        let decoded = decode_frame(&sent).expect("the connector's own encoder");
        let reject = Reject::decode(&decoded.ilp_packet).expect("a REJECT");
        assert_eq!(reject.code.as_str(), "F01");
        assert!(
            reject.message.contains("toon-channel"),
            "{}",
            reject.message
        );
        assert!(reject.message.contains("ADR 0075"), "{}", reject.message);
        assert_eq!(state.claim_gate.session_payee("g.toon.agent", 1), None);
    }

    /// A [`ClientEdgeState`] around `claim_gate` and nothing else a BTP
    /// session test needs to vary: a connector with no routes, no app and
    /// no peer transport, a fresh session registry, and every optional
    /// field at the shape a client-only node has. Spelled out once here so
    /// a new `ClientEdgeState` field costs one edit rather than one per
    /// test.
    fn test_state(claim_gate: crate::claim_gate::ClientClaimGate) -> ClientEdgeState {
        use chrono::TimeZone;
        use connector_runtime::{Connector, FakeAppClient, InProcessPeerTransport, TestClock};
        use connector_signer::LocalSigner;

        let clock = Arc::new(TestClock::new(
            chrono::Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
        ));
        ClientEdgeState {
            connector: Arc::new(Connector::new(
                vec![],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                clock,
            )),
            signer: Arc::new(LocalSigner::generate("session-signer")),
            claim_gate: claim_gate.into(),
            wrap_receiver_secret: None,
            node: Arc::new(connector_domain::NodeFacts::default()),
            btp_session_window: crate::DEFAULT_BTP_SESSION_WINDOW,
            session_registry: Arc::new(crate::session_registry::SessionRegistry::new()),
            peers: None,
            identities: Arc::from([]),
        }
    }

    /// A [`ClientEdgeState`] whose gate admits the test channels
    /// (`crate::test_support`) and whose payout ledger has a channel open
    /// toward the EVM channel's voucher signer -- everything
    /// [`verify_and_record_declared_channel`] and
    /// [`crate::claim_gate::ClientClaimGate::credit_session_payout`] need,
    /// without a chain.
    async fn state_over_the_test_channels() -> ClientEdgeState {
        let gate = test_support::voucher_gate().with_payout_ledger(
            ledger_paying(test_support::address_of(&test_support::authorizer())).await,
        );
        test_state(gate)
    }

    /// A MESSAGE frame carrying an `auth` entry with `peerId` and,
    /// optionally, further fields merged in beside it.
    fn auth_message_frame(peer_id: &str, extra: Option<serde_json::Value>) -> Vec<u8> {
        let mut body = serde_json::json!({ "peerId": peer_id, "secret": "" });
        if let Some(extra) = extra {
            body.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
        }
        connector_btp::encode_message(
            1,
            &[ProtocolData {
                name: AUTH_PROTOCOL.to_string(),
                content_type: CONTENT_TYPE_TEXT,
                data: body.to_string().into_bytes(),
            }],
            &[],
        )
    }

    /// Drives one frame through [`handle_frame`] with a fresh session's
    /// worth of plumbing, and returns the resulting `binding` and every
    /// frame written.
    async fn run_auth_frame(
        state: &Arc<ClientEdgeState>,
        frame_bytes: &[u8],
    ) -> (Option<(String, u64)>, Vec<Vec<u8>>) {
        let (replies, mut reply_rx) = mpsc::channel::<Vec<u8>>(REPLY_QUEUE_DEPTH);
        let window = Arc::new(Semaphore::new(4));
        let outbound = Arc::new(OutboundRequests::new());
        let mut binding = None;
        handle_frame(
            frame_bytes,
            state,
            &window,
            &replies,
            &outbound,
            &mut binding,
        )
        .await
        .expect("the reply channel has a live receiver");
        let mut sent = Vec::new();
        while let Ok(frame) = reply_rx.try_recv() {
            sent.push(frame);
        }
        (binding, sent)
    }

    /// Issues #790 and #1384: an agent that only ever earns -- opens a
    /// channel toward this node, serves paid work, pays nothing -- is still
    /// creditable. Its auth frame carries a `channelChallenge`, the voucher
    /// claim-state challenge signed by the channel's voucher signer; the
    /// gate verifies it against the signer the backend reads for the
    /// channel and teaches the session that signer as its payee, before any
    /// voucher has been presented.
    #[tokio::test]
    async fn a_channel_challenge_at_auth_credits_a_session_that_never_paid() {
        let state = Arc::new(state_over_the_test_channels().await);
        let challenge =
            test_support::evm_challenge(&test_support::authorizer(), crate::now_unix() + 60);

        let frame = auth_message_frame(
            "g.toon.agent",
            Some(serde_json::json!({ "channelChallenge": challenge })),
        );
        let (binding, _) = run_auth_frame(&state, &frame).await;
        let (address, generation) = binding.expect("the auth bound the session");
        assert_eq!(address, "g.toon.agent");

        let payout = state
            .claim_gate
            .credit_session_payout("g.toon.agent", generation, None, &[9u8; 32], 500)
            .await
            .expect("the auth-time challenge taught the session its payee with no voucher sent");
        assert_eq!(
            payout.payee,
            VoucherSigner::Evm(test_support::address_of(&test_support::authorizer()))
        );
    }

    /// The same on Solana: the payee taught is the channel's
    /// `authorized_signer`, as the chain records it.
    #[tokio::test]
    async fn a_solana_channel_challenge_at_auth_teaches_its_authorized_signer() {
        let state = Arc::new(state_over_the_test_channels().await);
        let signer = test_support::solana_signer();
        let challenge = test_support::solana_challenge(&signer, crate::now_unix() + 60);
        let frame = auth_message_frame(
            "g.toon.agent",
            Some(serde_json::json!({ "channelChallenge": challenge })),
        );
        let (binding, _) = run_auth_frame(&state, &frame).await;
        let (_, generation) = binding.expect("the auth bound the session");
        assert_eq!(
            state.claim_gate.session_payee("g.toon.agent", generation),
            Some(VoucherSigner::Solana(signer.public.to_bytes()))
        );
    }

    /// Issue #790's enforcement half: a session cannot teach itself a payee
    /// by naming a channel. A challenge signed by a key that is not the
    /// channel's voucher signer, one already expired, and one signed further
    /// ahead than the peer-role bound all teach nothing -- and the session is
    /// still bound, since the challenge is best-effort beside the bind.
    #[tokio::test]
    async fn a_channel_challenge_that_does_not_hold_teaches_nothing() {
        let now = crate::now_unix();
        let too_far = now + connector_peer_btp::role_gate::MAX_PEER_CHALLENGE_LIFETIME_SECS + 60;
        for (case, challenge) in [
            (
                "wrong key",
                test_support::evm_challenge(&test_support::stranger(), now + 60),
            ),
            (
                "expired",
                test_support::evm_challenge(&test_support::authorizer(), now - 1),
            ),
            (
                "too far ahead",
                test_support::evm_challenge(&test_support::authorizer(), too_far),
            ),
        ] {
            let state = Arc::new(state_over_the_test_channels().await);
            let frame = auth_message_frame(
                "g.toon.agent",
                Some(serde_json::json!({ "channelChallenge": challenge })),
            );
            let (binding, _) = run_auth_frame(&state, &frame).await;
            let (_, generation) = binding.expect("the bind itself stands");
            assert_eq!(
                state.claim_gate.session_payee("g.toon.agent", generation),
                None,
                "{case}: taught nothing"
            );
        }
    }

    /// ADR 0075 decision 5 (issue #1384): the retired `auth_channel_proof`
    /// -- `channelId`, `expires` and `signature` beside `peerId`, signing the
    /// `TokenNetwork`-domain challenge -- is refused by name: an ERROR frame
    /// naming it, and no bind, so a client still sending it learns why.
    #[tokio::test]
    async fn the_retired_auth_channel_proof_is_refused_by_name() {
        let state = Arc::new(state_over_the_test_channels().await);
        for fields in [
            serde_json::json!({
                "channelId": format!("0x{}", "ab".repeat(32)),
                "expires": crate::now_unix() + 60,
                "signature": format!("0x{}", "11".repeat(65)),
            }),
            serde_json::json!({ "channelId": format!("0x{}", "ab".repeat(32)) }),
        ] {
            let frame = auth_message_frame("g.toon.agent", Some(fields));
            let (binding, sent) = run_auth_frame(&state, &frame).await;
            assert_eq!(binding, None, "a refused auth binds nothing");
            assert_eq!(sent.len(), 1);
            let decoded = decode_frame(&sent[0]).expect("the connector's own encoder");
            assert_eq!(decoded.frame_type, BTP_ERROR);
            let text = String::from_utf8_lossy(&sent[0]);
            assert!(text.contains("auth_channel_proof"), "{text}");
            assert!(text.contains("ADR 0075"), "{text}");
            assert!(text.contains("channelChallenge"), "{text}");
        }
    }

    /// Issue #1408: the committed `client_auth_channel_challenge` vectors
    /// (`vectors/wire-vectors.json`) are read by this module's own
    /// [`auth_channel_challenge`] -- the function [`handle_frame`] calls on
    /// every `auth` entry -- and judged by [`challenge_in_window`] exactly
    /// as [`verify_and_record_declared_channel`] does: extraction succeeds
    /// whether or not a challenge is within its window (only the window
    /// decides `accepted`, never whether the JSON parses), and each case's
    /// `accepted` is that check's verdict alone -- one already ahead but
    /// expired, one signed too far ahead of `now` to be a bearer proof yet.
    /// Checked both from the bare `channelChallenge` JSON and from the
    /// pinned BTP MESSAGE frame's own `auth` entry, so the wire bytes and
    /// the object they carry are proven to agree.
    #[test]
    fn the_committed_client_auth_channel_challenge_vectors_match_the_real_parser() {
        let vectors: serde_json::Value =
            serde_json::from_str(WIRE_VECTORS).expect("the committed vectors parse");
        for chain in ["evm", "solana"] {
            let cases = vectors["client_auth_channel_challenge"][chain]
                .as_array()
                .unwrap_or_else(|| panic!("client_auth_channel_challenge.{chain} is an array"));
            assert!(
                !cases.is_empty(),
                "client_auth_channel_challenge.{chain} has cases"
            );
            for case in cases {
                let name = case["name"].as_str().expect("a name");
                let now = case["now"].as_u64().expect("a now");
                let expires = case["expires"].as_u64().expect("an expires");
                let accepted = case["accepted"].as_bool().expect("an accepted flag");
                let auth_entry_json = case["auth_entry_json"].as_str().expect("auth_entry_json");
                let btp_message_hex = case["btp_message_hex"].as_str().expect("btp_message_hex");

                let challenge = auth_channel_challenge(auth_entry_json.as_bytes())
                    .unwrap_or_else(|| panic!("{name}: the real parser must read this challenge"));
                assert_eq!(challenge.expires(), expires, "{name}");
                assert_eq!(
                    challenge_in_window(challenge.expires(), now),
                    accepted,
                    "{name}: the real window check must agree with the vector"
                );

                let bytes = hex::decode(btp_message_hex).expect("the vector's hex decodes");
                let frame = decode_frame(&bytes).expect("the connector's own decoder");
                assert_eq!(frame.frame_type, BTP_MESSAGE, "{name}");
                let entry = frame
                    .protocol_data
                    .iter()
                    .find(|pd| pd.name == AUTH_PROTOCOL)
                    .unwrap_or_else(|| panic!("{name}: the frame carries an auth entry"));
                assert_eq!(entry.data, auth_entry_json.as_bytes(), "{name}");
                let challenge_from_frame =
                    auth_channel_challenge(&entry.data).unwrap_or_else(|| {
                        panic!("{name}: the real parser must read the frame's auth entry")
                    });
                assert_eq!(challenge_from_frame, challenge, "{name}");
            }
        }
    }

    /// Issue #792's production sequence: authenticate bare, open a channel,
    /// *then* re-authenticate to declare it. The credit lands on the
    /// *second* auth of one session.
    #[tokio::test]
    async fn a_second_auth_on_an_already_bound_session_still_teaches_its_payee() {
        let state = Arc::new(state_over_the_test_channels().await);
        let (replies, _reply_rx) = mpsc::channel::<Vec<u8>>(REPLY_QUEUE_DEPTH);
        let window = Arc::new(Semaphore::new(4));
        let outbound = Arc::new(OutboundRequests::new());
        let mut binding = None;

        let bare_frame = auth_message_frame("g.toon.agent", None);
        handle_frame(
            &bare_frame,
            &state,
            &window,
            &replies,
            &outbound,
            &mut binding,
        )
        .await
        .expect("the reply channel has a live receiver");
        let bare_generation = binding.as_ref().expect("bound").1;
        assert!(state
            .claim_gate
            .credit_session_payout("g.toon.agent", bare_generation, None, &[9u8; 32], 500)
            .await
            .is_none());

        let declare_frame = auth_message_frame(
            "g.toon.agent",
            Some(serde_json::json!({
                "channelChallenge": test_support::evm_challenge(
                    &test_support::authorizer(),
                    crate::now_unix() + 60,
                ),
            })),
        );
        handle_frame(
            &declare_frame,
            &state,
            &window,
            &replies,
            &outbound,
            &mut binding,
        )
        .await
        .expect("the reply channel has a live receiver");

        let generation = binding.as_ref().expect("bound").1;
        let payout = state
            .claim_gate
            .credit_session_payout("g.toon.agent", generation, None, &[9u8; 32], 500)
            .await
            .expect("the re-auth's challenge taught the session its payee");
        assert_eq!(
            state
                .claim_gate
                .session_payee("g.toon.agent", bare_generation),
            None,
            "the session the re-auth replaced was closed"
        );
        assert_eq!(
            payout.payee,
            VoucherSigner::Evm(test_support::address_of(&test_support::authorizer()))
        );
    }

    /// Issue #779: a session (re)establishing is resent a payout voucher
    /// stranded by an earlier failed delivery, with no new job in sight --
    /// driven through the real `handle_frame` auth/bind path (not
    /// `deliver_pending_claim` called directly), so this fails if the auth
    /// branch's [`spawn_stranded_claim_resend`] call is deleted, per the
    /// issue's own AC4.
    ///
    /// Issue #1396: the new session is resent the voucher because it proves
    /// the key the voucher is owed to again, with a `channelChallenge` on
    /// this auth -- a bare reconnect proves nothing and is resent nothing.
    #[tokio::test]
    async fn a_reconnecting_session_is_resent_its_stranded_payout_voucher() {
        let address = "g.toon.stranded";
        let state = Arc::new(state_over_the_test_channels().await);
        let payee = VoucherSigner::Evm(test_support::address_of(&test_support::authorizer()));
        let ledger = Arc::clone(state.claim_gate.payout_ledger().expect("a payout ledger"));
        let stranded = ledger
            .record_payout_once(payee, &[1; 32], 1_234)
            .await
            .expect("a channel toward the payee is open");

        let (replies, mut reply_rx) = mpsc::channel::<Vec<u8>>(REPLY_QUEUE_DEPTH);
        let window = Arc::new(Semaphore::new(4));
        let outbound = Arc::new(OutboundRequests::new());
        let mut binding = None;

        let frame = auth_message_frame(
            address,
            Some(serde_json::json!({
                "channelChallenge": test_support::evm_challenge(
                    &test_support::authorizer(),
                    crate::now_unix() + 60,
                ),
            })),
        );
        handle_frame(&frame, &state, &window, &replies, &outbound, &mut binding)
            .await
            .expect("the reply channel has a live receiver");
        assert_eq!(
            binding.as_ref().map(|(a, _)| a.clone()),
            Some(address.to_string())
        );

        // This socket sees two frames: the auth ack and the resent
        // stranded TRANSFER -- the resend is a spawned task racing the
        // ack write, so only the *content* seen across both, not their
        // order, is this test's contract.
        let mut saw_transfer = false;
        for _ in 0..2 {
            let sent = reply_rx.recv().await.expect("both frames are written");
            let decoded = decode_frame(&sent).expect("the connector's own encoder");
            if decoded.frame_type == BTP_TRANSFER {
                saw_transfer = true;
                assert_eq!(decoded.amount, Some(1_234));
                let pd = decoded
                    .protocol_data
                    .iter()
                    .find(|pd| pd.name == PAYOUT_CLAIM_PROTOCOL)
                    .expect("the stranded voucher rode this TRANSFER");
                let json: serde_json::Value = serde_json::from_slice(&pd.data).expect("valid JSON");
                assert_eq!(json["channelId"], stranded.channel_id());
                assert_eq!(json["maxClaimableAmount"], "1234");

                outbound.resolve(BtpFrame {
                    frame_type: BTP_RESPONSE,
                    request_id: decoded.request_id,
                    amount: None,
                    protocol_data: Vec::new(),
                    ilp_packet: Vec::new(),
                });
            }
        }
        assert!(
            saw_transfer,
            "a session that reconnects and proves its payee again must be resent its stranded voucher"
        );

        // The resend's acknowledgement runs in the spawned task, after the
        // RESPONSE above wakes its `send_transfer` await -- give the
        // executor a chance to poll it to completion.
        let mut cleared = false;
        for _ in 0..1000 {
            if ledger.pending_for(&payee).is_empty() {
                cleared = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            cleared,
            "a successfully delivered resend must acknowledge and clear the pending voucher"
        );
    }

    /// Issue #807's BTP mirror of `handle_ilp`'s HTTP-side broadening: a
    /// `greeting`-flagged PREPARE is structurally a bootstrap probe, never a
    /// real payment attempt, so it must be answered with the §1.4 greeting
    /// even when -- as here -- `destination` matches no configured route at
    /// all. This is `edge-client.ts`'s `fetchGreeting` probe, and is what
    /// left a client with a stale or missing genesis peer seed unable to
    /// bootstrap before this fix. Until issue #1269 the same shape was
    /// inferred from an all-zero execution condition; it is now the
    /// packet's own explicit `greeting` flag.
    #[tokio::test]
    async fn a_greeting_prepare_over_btp_is_answered_with_the_greeting() {
        use crate::claim_gate::ClientClaimGate;
        use crate::X402PaymentRequired;
        use chrono::TimeZone;
        use connector_runtime::InMemoryJournal;

        let gate = ClientClaimGate::restore(Arc::new(InMemoryJournal::new()))
            .expect("a fresh in-memory journal has nothing to replay");
        let state = Arc::new(test_state(gate));

        let prepare = Prepare {
            amount: 0,
            expires_at: chrono::Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
            greeting: true,
            destination: "g.nowhere".to_string(),
            data: Vec::new(),
        };
        let frame = connector_btp::encode_message(1, &[], &prepare.encode());

        let (replies, mut reply_rx) = mpsc::channel::<Vec<u8>>(REPLY_QUEUE_DEPTH);
        let window = Arc::new(Semaphore::new(4));
        let outbound = Arc::new(OutboundRequests::new());
        let mut binding = None;
        handle_frame(&frame, &state, &window, &replies, &outbound, &mut binding)
            .await
            .expect("the reply channel has a live receiver");

        let sent = reply_rx.recv().await.expect("a reply was sent");
        let decoded = decode_frame(&sent).expect("the connector's own encoder");
        let reject = Reject::decode(&decoded.ilp_packet).expect("a REJECT carries the terms");
        assert_eq!(
            reject.code.as_str(),
            "F06",
            "a greeting PREPARE to an unmatched destination must be greeted, not F02'd"
        );

        let terms_bytes = decoded
            .protocol_data
            .iter()
            .find(|pd| pd.name == PAYMENT_REQUIRED_PROTOCOL)
            .expect("the greeting's terms ride as payment-required protocolData")
            .data
            .clone();
        let terms: X402PaymentRequired =
            serde_json::from_slice(&terms_bytes).expect("valid x402 terms JSON");
        assert_eq!(terms.toon().unwrap().amount, "0");
    }

    /// Issue #1210: a route's `request` table rides the BTP
    /// `payment-required` REJECT's protocolData exactly as it rides the
    /// HTTP 402 body -- both carriages share the one emitter
    /// (`x402_terms_body`), so this is that shared assertion's BTP half.
    #[tokio::test]
    async fn an_unpaid_prepare_over_btp_to_a_route_with_a_request_table_is_greeted_with_it() {
        use chrono::TimeZone;
        use connector_config::StaticRoute;
        use connector_runtime::{Connector, FakeAppClient, InProcessPeerTransport, TestClock};
        use connector_signer::LocalSigner;

        let declared = serde_json::json!({"protocol": "nip90", "kinds": [5096, 5098]});
        let route = StaticRoute::new_priced("g.toon.gas", "http://localhost:4000", 100)
            .unwrap()
            .with_request(declared.clone());
        let clock = Arc::new(TestClock::new(
            chrono::Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
        ));
        let gate = crate::claim_gate::ClientClaimGate::restore(Arc::new(
            connector_runtime::InMemoryJournal::new(),
        ))
        .expect("a fresh in-memory journal has nothing to replay");
        let state = Arc::new(ClientEdgeState {
            connector: Arc::new(Connector::new(
                vec![route],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                clock,
            )),
            signer: Arc::new(LocalSigner::generate("session-signer")),
            claim_gate: gate.into(),
            wrap_receiver_secret: None,
            node: Arc::new(connector_domain::NodeFacts::default()),
            btp_session_window: crate::DEFAULT_BTP_SESSION_WINDOW,
            session_registry: Arc::new(crate::session_registry::SessionRegistry::new()),
            peers: None,
            identities: Arc::from([]),
        });

        let prepare = Prepare {
            amount: 0,
            expires_at: chrono::Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
            greeting: false,
            destination: "g.toon.gas".to_string(),
            data: Vec::new(),
        };
        let frame = connector_btp::encode_message(1, &[], &prepare.encode());

        let (replies, mut reply_rx) = mpsc::channel::<Vec<u8>>(REPLY_QUEUE_DEPTH);
        let window = Arc::new(Semaphore::new(4));
        let outbound = Arc::new(OutboundRequests::new());
        let mut binding = None;
        handle_frame(&frame, &state, &window, &replies, &outbound, &mut binding)
            .await
            .expect("the reply channel has a live receiver");

        let sent = reply_rx.recv().await.expect("a reply was sent");
        let decoded = decode_frame(&sent).expect("the connector's own encoder");
        let terms_bytes = decoded
            .protocol_data
            .iter()
            .find(|pd| pd.name == PAYMENT_REQUIRED_PROTOCOL)
            .expect("the greeting's terms ride as payment-required protocolData")
            .data
            .clone();
        let value: serde_json::Value = serde_json::from_slice(&terms_bytes).unwrap();
        assert_eq!(value["request"], declared);
    }
}
