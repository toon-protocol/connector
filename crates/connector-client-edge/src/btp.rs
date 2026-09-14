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
//! each other into `NonceNotAdvancing`, and it is the one ordering the
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
use connector_domain::client_claim::{ClientClaim, EVM_NAMESPACE};
use connector_domain::{PacketResponse, Prepare, Price, Reject, RejectCode};
use connector_signer::{verify_evm_claim_state_challenge, EvmClaimStateChallenge};

use crate::channels::decode_hex_bytes;
use crate::claim_gate::DurabilityTicket;
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
        match peer_handover(&frame_bytes, &state) {
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
        state.session_registry.unbind(&address, generation);
    }
    drop(replies);
    let _ = writer.await;
}

/// Whether this frame's claim hands the rest of the session to the peer
/// carriage (`peer-carriage-spec.md` §1.2, §1.5, issue #678).
///
/// `None` for every frame carrying no claim entry, and for every node that
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
fn peer_handover(frame_bytes: &[u8], state: &Arc<ClientEdgeState>) -> Option<BtpClaimVerdict> {
    let peers = state.peers.as_ref()?;
    let frame = decode_frame(frame_bytes).ok()?;
    if frame.frame_type != BTP_MESSAGE && frame.frame_type != BTP_TRANSFER {
        return None;
    }
    connector_peer_btp::claim_json::from_protocol_data(&frame.protocol_data)?;
    Some(peers.btp_claim_verdict(&frame.protocol_data))
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
/// `session_address` is this socket's own binding in
/// [`crate::session_registry::SessionRegistry`] (`None` if this session
/// never sent a usable auth frame). When present, this claim -- already
/// fully verified by [`crate::claim_gate::ClientClaimGate::admit`] by the
/// time this runs -- teaches the gate which channel this session speaks
/// for (issue #787): the missing join between a session bound under its
/// ILP address and a payout ledger keyed by channel id. EVM only, matching
/// [`crate::outbound_ledger::ClientPayoutLedger`]'s own reach -- there is
/// no Solana payout to resolve towards yet.
fn record_accepted_claim(
    state: &ClientEdgeState,
    claim: &ClientClaim,
    session_address: Option<&str>,
) {
    let channel_key = claim.channel_key();
    state.connector.recognize_channel(&channel_key);
    state
        .claim_gate
        .note_claim_time(&channel_key, crate::now_unix());

    if let (Some(address), Some((EVM_NAMESPACE, channel_id))) =
        (session_address, channel_key.split_once(':'))
    {
        state
            .claim_gate
            .record_session_channel(address, channel_id.to_string());
    }
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

/// A client's declared claim to control a channel, carried on the same
/// auth frame as its `peerId` binding (issue #790). This is the BTP twin
/// of `/ilp/claim-state`'s own per-channel proof (`crate::claim_state`):
/// reusing that endpoint's identical domain-separated challenge signature
/// (`connector_signer::EvmClaimStateChallenge`) rather than inventing a
/// second scheme, and rather than a claim's own balance-proof scheme --
/// issue #558's own rule against letting one signature scheme stand in for
/// another applies here too, or a captured claim-state proof and a
/// captured claim could be replayed as each other. EVM only, matching
/// [`crate::claim_gate::ClientClaimGate`]'s `session_channels` map's own
/// reach.
struct DeclaredChannelProof {
    channel_id: String,
    expires: u64,
    signature: String,
}

/// Best-effort extraction of an auth frame's declared channel-control
/// proof, alongside [`auth_peer_id`]'s extraction of the same frame's
/// `peerId`. `None` for anything that is not JSON, or that omits any one
/// of the three fields -- there is no partial declaration, since a
/// signature with no channel to check it against (or vice versa) can never
/// be verified.
fn auth_channel_proof(auth_data: &[u8]) -> Option<DeclaredChannelProof> {
    let json: serde_json::Value = serde_json::from_slice(auth_data).ok()?;
    Some(DeclaredChannelProof {
        channel_id: json.get("channelId")?.as_str()?.to_string(),
        expires: json.get("expires")?.as_u64()?,
        signature: json.get("signature")?.as_str()?.to_string(),
    })
}

/// Verify a BTP session's declared channel-control proof (issue #790)
/// against [`crate::channels::ClientChannelRegistry`]'s registered
/// counterparty for the channel it names -- the identical check
/// `/ilp/claim-state` runs for a read, reused here to teach
/// [`crate::claim_gate::ClientClaimGate::record_session_channel`] a
/// channel *before* this session has ever presented a claim. Without this,
/// an agent that only ever earns -- opens a channel, serves paid work,
/// sends no claim of its own -- is never creditable at all:
/// `record_accepted_claim` only learns the association from a genuinely
/// verified inbound claim, which such an agent never sends.
///
/// Best-effort and silent on any failure, same posture as the `peerId`
/// bind it rides alongside: an expired, malformed, unresolvable or
/// wrongly-signed proof simply leaves this session's channel association
/// exactly where it was. `record_accepted_claim`'s own inbound-claim path
/// is untouched and stays the fallback it always was; a session with
/// neither a valid proof nor a claim yet is credited nothing, exactly as
/// issue #787 already decided.
async fn verify_and_record_declared_channel(
    state: &ClientEdgeState,
    address: &str,
    proof: DeclaredChannelProof,
) {
    if proof.expires <= crate::now_unix() {
        tracing::debug!(
            address = %address,
            "a BTP session's declared channel-control proof has already expired -- ignoring it"
        );
        return;
    }
    let Some(channel_id) = decode_hex_bytes::<32>(&proof.channel_id) else {
        return;
    };
    let Some(signature) = decode_hex_bytes::<65>(&proof.signature) else {
        return;
    };
    let requester = format!("btp-auth-channel-proof:{address}");
    let Ok(Some(channel)) = state
        .claim_gate
        .channels()
        .evm(&channel_id, &requester)
        .await
    else {
        return;
    };
    let challenge = EvmClaimStateChallenge {
        channel_id,
        expires: proof.expires,
        chain_id: channel.chain_id,
        token_network_address: channel.token_network_address,
    };
    if !verify_evm_claim_state_challenge(&challenge, &signature, &channel.counterparty) {
        tracing::debug!(
            address = %address,
            channel_id = %proof.channel_id,
            "a BTP session's declared channel-control proof did not verify -- ignoring it"
        );
        return;
    }
    state
        .claim_gate
        .record_session_channel(address, format!("0x{}", hex::encode(channel_id)));
    tracing::info!(
        address = %address,
        channel_id = %proof.channel_id,
        "a BTP session proved control of a channel at auth -- it can now be credited without \
         presenting a claim of its own first"
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
            Some(generation),
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

/// The protocolData entry a payout TRANSFER carries (issue #699): the
/// signed cumulative claim this connector owes the client on that channel,
/// as JSON -- `channelId`/`nonce`/`cumulativeAmount` matching
/// `WireClaim`'s own fields, `signature` hex-encoded the same way
/// `ClientClaimGate`'s inbound claim JSON already expects one
/// (`0x`-prefixed 65-byte `r‖s‖recoveryId`). JSON rather than
/// [`connector_runtime::WireClaim::encode`]'s peer-role binary shape,
/// matching every other protocolData entry this dialect ever carries -- the
/// auth secret, the inbound claim, the x402 terms, the accumulated-cost
/// total -- all of which are raw UTF-8 text, never a second binary
/// sub-format riding inside the frame's own binary envelope.
///
/// There is no `signerAddress` field: unlike a client's self-declared one
/// (never trusted -- see `ClientClaimGate`'s own doc), this claim's signer
/// is the channel's own recorded counterparty from the client's point of
/// view, implicit in which channel the TRANSFER arrived on, exactly as a
/// peer-role `WireClaim` carries no signer field either.
///
/// This is the mapping of a *claim* onto the grammar, so it stays with the
/// client edge rather than moving into [`connector_btp`] with the codec
/// (issue #713): the codec owns the entry's name and the bytes that carry
/// it, and deliberately cannot see a claim type.
///
/// Production caller: `crate::session_route::route_prepare` (issue #770),
/// once a client session's own PREPARE genuinely fulfills -- see that
/// module for when a payout TRANSFER goes out.
pub(crate) fn payout_claim_protocol_data(claim: &connector_runtime::WireClaim) -> ProtocolData {
    let json = serde_json::json!({
        "channelId": claim.channel_id,
        "nonce": claim.nonce,
        "cumulativeAmount": claim.cumulative_amount,
        "signature": format!("0x{}", hex::encode(claim.signature.to_bytes())),
    });
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
        if let Some(address) = auth_peer_id(&entry.data) {
            let handle = BtpSessionHandle::new(replies.clone(), Arc::clone(outbound));
            let generation =
                state
                    .session_registry
                    .bind(address.clone(), handle, crate::now_unix());
            if let Some(proof) = auth_channel_proof(&entry.data) {
                verify_and_record_declared_channel(state, &address, proof).await;
            }
            // After the channel proof above, never before it: a session
            // whose channel this connector first learns at auth (issue
            // #790) has nothing to resend on until that proof has been
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
                    let session_address = binding.as_ref().map(|(address, _)| address.clone());
                    tokio::spawn(async move {
                        let _slot = permit;
                        match durability.durable().await {
                            Ok(()) => {
                                record_accepted_claim(&state, &claim, session_address.as_deref())
                            }
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
        Some(json) if !state.connector.envelope_target_would_be_refused(&prepare) => {
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
    let session_address = binding.as_ref().map(|(address, _)| address.clone());
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
        session_address,
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
    session_address: Option<String>,
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
            Ok(()) => record_accepted_claim(&state, &claim, session_address.as_deref()),
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
    use connector_btp::{decode_frame, BtpFrame, BtpSessionHandle};

    /// Issue #699: a payout claim rides a TRANSFER's protocolData as JSON,
    /// matching every other entry this dialect carries (all UTF-8 text,
    /// never a second binary sub-format) rather than
    /// [`connector_runtime::WireClaim::encode`]'s peer-role bytes.
    #[test]
    fn payout_claim_protocol_data_encodes_the_wire_claims_fields_as_json() {
        let claim = connector_runtime::WireClaim {
            channel_id: format!("0x{:064x}", 1),
            nonce: 3,
            cumulative_amount: 750,
            signature: connector_runtime::ClaimSignature::Evm(connector_signer::Signature {
                r: [0x11; 32],
                s: [0x22; 32],
                recovery_id: 1,
            }),
        };

        let pd = payout_claim_protocol_data(&claim);

        assert_eq!(pd.name, PAYOUT_CLAIM_PROTOCOL);
        assert_eq!(pd.content_type, CONTENT_TYPE_TEXT);
        let json: serde_json::Value = serde_json::from_slice(&pd.data).expect("valid JSON");
        assert_eq!(json["channelId"], claim.channel_id);
        assert_eq!(json["nonce"], 3);
        assert_eq!(json["cumulativeAmount"], 750);
        assert_eq!(
            json["signature"],
            format!("0x{}", hex::encode(claim.signature.to_bytes()))
        );
    }

    /// Issue #699 end-to-end through the real production types: a
    /// [`crate::outbound_ledger::ClientPayoutLedger`] signs a payout claim,
    /// [`payout_claim_protocol_data`] carries it as a TRANSFER's
    /// protocolData over [`BtpSessionHandle::send_transfer`], and a
    /// stand-in client -- reading off the same wire `btp_session`'s writer
    /// task would write to -- decodes the frame, parses the JSON claim
    /// back out, and verifies its signature against the connector's own
    /// public key. Nothing here is a fake shortcut: the ledger, the frame
    /// codec and the origination path are exactly what a real session
    /// runs today, driven by `crate::session_route::route_prepare` (issue
    /// #770).
    #[tokio::test]
    async fn a_signed_payout_claim_is_delivered_over_transfer_and_verifies() {
        use crate::outbound_ledger::ClientPayoutLedger;
        use connector_runtime::ChannelDomain;
        use connector_signer::{
            derive_evm_address, verify_evm_balance_proof, EvmBalanceProof, LocalSigner, Signer,
        };

        let signer = Arc::new(LocalSigner::generate("payout-key"));
        let connector_address = derive_evm_address(&signer.public_key().unwrap());
        let domain = ChannelDomain {
            chain_id: 84_532,
            token_network_address: [0x33; 20],
        };
        let channel_id = format!("0x{:064x}", 7);

        let mut ledger = ClientPayoutLedger::new();
        ledger.set_signer(signer);
        ledger
            .set_channel_domain(channel_id.clone(), domain)
            .expect("valid channel id");
        let claim = ledger
            .record_payout(&channel_id, 42_000, "2030-01-01T00:00:00Z".parse().unwrap())
            .expect("signer and domain configured");

        let (replies, mut reply_rx) = mpsc::channel::<Vec<u8>>(1);
        let outbound = Arc::new(OutboundRequests::new());
        let handle = BtpSessionHandle::new(replies, Arc::clone(&outbound));

        let expected_channel_id = channel_id.clone();
        let peer = tokio::spawn(async move {
            let sent = reply_rx.recv().await.expect("the TRANSFER was written");
            let decoded = decode_frame(&sent).expect("the connector's own encoder");
            assert_eq!(decoded.frame_type, BTP_TRANSFER);
            assert_eq!(decoded.amount, Some(42_000));

            let pd = decoded
                .protocol_data
                .iter()
                .find(|pd| pd.name == PAYOUT_CLAIM_PROTOCOL)
                .expect("the payout claim rode the TRANSFER");
            let json: serde_json::Value = serde_json::from_slice(&pd.data).expect("valid JSON");
            assert_eq!(json["channelId"], expected_channel_id);
            assert_eq!(json["nonce"], 1);
            assert_eq!(json["cumulativeAmount"], 42_000);
            let signature_hex = json["signature"].as_str().unwrap();
            let signature_bytes = hex::decode(signature_hex.strip_prefix("0x").unwrap()).unwrap();

            let mut on_chain_id = [0u8; 32];
            on_chain_id[31] = 7;
            let proof = EvmBalanceProof {
                channel_id: on_chain_id,
                nonce: json["nonce"].as_u64().unwrap(),
                transferred_amount: u128::from(json["cumulativeAmount"].as_u64().unwrap()),
                locked_amount: 0,
                locks_root: [0u8; 32],
                chain_id: domain.chain_id,
                token_network_address: domain.token_network_address,
            };
            assert!(verify_evm_balance_proof(
                &proof,
                &signature_bytes,
                &connector_address
            ));

            outbound.resolve(BtpFrame {
                frame_type: BTP_RESPONSE,
                request_id: decoded.request_id,
                amount: None,
                protocol_data: Vec::new(),
                ilp_packet: Vec::new(),
            });
        });

        handle
            .send_transfer(42_000, &[payout_claim_protocol_data(&claim)])
            .await
            .expect("the peer answered before the timeout");
        peer.await.expect("the peer task");
    }

    /// Issue #787's production wiring, proven directly: once a claim has
    /// cleared `ClientClaimGate::admit` on a session bound at `address`,
    /// [`record_accepted_claim`] -- the shared call site both a standalone
    /// claim message and a claim riding a packet finish through -- teaches
    /// the gate that `address` speaks for this claim's channel. A later
    /// fulfilment credited through
    /// `ClientClaimGate::credit_session_payout` (`crate::session_route`'s
    /// own caller) then finds it, rather than crediting nothing -- which
    /// is exactly what happened on every real deployment before this fix,
    /// since production binds a session under its ILP address, never a
    /// channel id (issue #736/toon-client#503).
    #[tokio::test]
    async fn record_accepted_claim_teaches_the_session_channel_association() {
        use crate::claim_gate::ClientClaimGate;
        use crate::outbound_ledger::ClientPayoutLedger;
        use connector_domain::client_claim::{ClientClaimCommon, EvmClientClaim};
        use connector_runtime::{ChannelDomain, InMemoryJournal};
        use connector_signer::LocalSigner;

        let channel_id = format!("0x{:064x}", 5);
        let mut ledger = ClientPayoutLedger::new();
        ledger.set_signer(Arc::new(LocalSigner::generate("payout-key")));
        ledger
            .set_channel_domain(
                channel_id.clone(),
                ChannelDomain {
                    chain_id: 84_532,
                    token_network_address: [0x77; 20],
                },
            )
            .expect("valid channel id");
        let ledger = Arc::new(ledger);

        let gate = ClientClaimGate::restore(Default::default(), Arc::new(InMemoryJournal::new()))
            .expect("a fresh in-memory journal has nothing to replay")
            .with_payout_ledger(Arc::clone(&ledger));
        let state = test_state(gate);

        let claim = ClientClaim::Evm(EvmClientClaim {
            common: ClientClaimCommon {
                message_id: "m1".to_string(),
                timestamp: "2030-01-01T00:00:00Z".to_string(),
                sender_id: "sender".to_string(),
            },
            channel_id: channel_id.clone(),
            nonce: 1,
            transferred_amount: 500,
            locked_amount: "0".to_string(),
            locks_root: format!("0x{}", "00".repeat(32)),
            signature: format!("0x{}", "11".repeat(65)),
            signer_address: format!("0x{}", "22".repeat(20)),
            chain_id: None,
            token_network_address: None,
            token_address: None,
        });

        record_accepted_claim(&state, &claim, Some("g.toon.agent"));

        let fulfillment = [9u8; 32];
        let payout = state
            .claim_gate
            .credit_session_payout("g.toon.agent", &fulfillment, 500, chrono::Utc::now())
            .await
            .expect("the session's channel was just taught by the claim above");
        assert_eq!(payout.channel_id, channel_id);
    }

    /// A signed `ClaimStateChallenge` over `channel_id`/`expires`, matching
    /// what `/ilp/claim-state`'s own tests produce for the identical
    /// signature scheme this auth-time proof reuses (issue #790).
    fn sign_channel_control_proof(
        secret: &libsecp256k1::SecretKey,
        challenge: &EvmClaimStateChallenge,
    ) -> String {
        let digest = connector_signer::evm_claim_state_challenge_digest(challenge);
        let message = libsecp256k1::Message::parse(&digest);
        let (signature, recovery_id) = libsecp256k1::sign(&message, secret);
        let mut bytes = signature.serialize().to_vec();
        let recovery_byte: u8 = recovery_id.into();
        bytes.push(recovery_byte + 27);
        format!("0x{}", hex::encode(bytes))
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

    /// Builds a [`ClientEdgeState`] over one EVM channel, declared with a
    /// known counterparty keypair and a payout ledger already bound to its
    /// domain -- everything [`verify_and_record_declared_channel`] and
    /// [`crate::claim_gate::ClientClaimGate::credit_session_payout`] need,
    /// without a chain: a declared (`record_evm`) channel is resolved from
    /// memory, exactly like a real node's `[[client_channels]]` config
    /// row.
    fn state_over_one_declared_channel(
        channel_id: &str,
        counterparty: connector_signer::Address,
        chain_id: u64,
        token_network_address: [u8; 20],
    ) -> ClientEdgeState {
        use crate::channels::{DepositFloor, EvmChannel};
        use crate::claim_gate::ClientClaimGate;
        use crate::outbound_ledger::ClientPayoutLedger;
        use connector_runtime::{ChannelDomain, InMemoryJournal};
        use connector_signer::LocalSigner;

        let mut channels = crate::ClientChannelRegistry::new();
        channels
            .record_evm(
                channel_id,
                EvmChannel {
                    counterparty,
                    chain_id,
                    token_network_address,
                    deposit_floor: DepositFloor::Unknown,
                },
            )
            .expect("a valid 32-byte channel id");

        let mut ledger = ClientPayoutLedger::new();
        ledger.set_signer(Arc::new(LocalSigner::generate("payout-key")));
        ledger
            .set_channel_domain(
                channel_id.to_string(),
                ChannelDomain {
                    chain_id,
                    token_network_address,
                },
            )
            .expect("valid channel id");

        let gate = ClientClaimGate::restore(channels, Arc::new(InMemoryJournal::new()))
            .expect("a fresh in-memory journal has nothing to replay")
            .with_payout_ledger(Arc::new(ledger));

        test_state(gate)
    }

    /// A MESSAGE frame carrying an `auth` entry with `peerId` and,
    /// optionally, a channel-control proof's three fields alongside it.
    fn auth_message_frame(peer_id: &str, channel_proof: Option<serde_json::Value>) -> Vec<u8> {
        let mut body = serde_json::json!({ "peerId": peer_id, "secret": "" });
        if let Some(proof) = channel_proof {
            body.as_object_mut()
                .unwrap()
                .extend(proof.as_object().unwrap().clone());
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
    /// worth of plumbing, and returns the resulting `binding`.
    async fn run_auth_frame(
        state: &Arc<ClientEdgeState>,
        frame_bytes: &[u8],
    ) -> Option<(String, u64)> {
        let (replies, _reply_rx) = mpsc::channel::<Vec<u8>>(REPLY_QUEUE_DEPTH);
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
        binding
    }

    /// Issue #790: an agent that only ever earns -- opens a channel, serves
    /// paid work, sends no claim of its own -- must still be creditable.
    /// This proves the BTP auth path end to end: a session's auth frame
    /// carries a channel-control proof (the same domain-separated
    /// challenge `/ilp/claim-state` verifies for a read, issue #558's rule
    /// against reusing a claim's own signature scheme applied here too),
    /// `handle_frame` verifies it against the channel's registered
    /// counterparty and teaches `session_channels` *before* any claim has
    /// ever been presented, and a later fulfilment is credited through
    /// `credit_session_payout` exactly as it would be for a session that
    /// had paid first (issue #787).
    #[tokio::test]
    async fn a_declared_channel_control_proof_at_auth_credits_a_session_that_never_paid() {
        use libsecp256k1::{PublicKey, SecretKey};

        let secret = SecretKey::parse(&[7u8; 32]).unwrap();
        let public = PublicKey::from_secret_key(&secret);
        let counterparty = connector_signer::derive_evm_address(&public.serialize());

        let channel_id_bytes = [5u8; 32];
        let channel_id = format!("0x{}", hex::encode(channel_id_bytes));
        let chain_id = 84_532u64;
        let token_network_address = [0x77u8; 20];

        let state = Arc::new(state_over_one_declared_channel(
            &channel_id,
            counterparty,
            chain_id,
            token_network_address,
        ));

        let expires = crate::now_unix() + 3600;
        let signature = sign_channel_control_proof(
            &secret,
            &EvmClaimStateChallenge {
                channel_id: channel_id_bytes,
                expires,
                chain_id,
                token_network_address,
            },
        );

        let frame = auth_message_frame(
            "g.toon.agent",
            Some(serde_json::json!({
                "channelId": channel_id,
                "expires": expires,
                "signature": signature,
            })),
        );
        let binding = run_auth_frame(&state, &frame).await;
        assert_eq!(
            binding.map(|(address, _)| address),
            Some("g.toon.agent".to_string())
        );

        let fulfillment = [9u8; 32];
        let payout = state
            .claim_gate
            .credit_session_payout("g.toon.agent", &fulfillment, 500, chrono::Utc::now())
            .await
            .expect("the auth-time proof taught the session's channel with no claim ever sent");
        assert_eq!(payout.channel_id, channel_id);
    }

    /// Issue #790's own enforcement half: a session cannot cause payouts to
    /// be credited to a channel it does not control just by naming it --
    /// the declared proof must actually verify against that channel's
    /// registered counterparty. A wrong key's signature is silently
    /// ignored, leaving the session exactly as uncreditable as it was
    /// before #787: `credit_session_payout` finds no association.
    #[tokio::test]
    async fn a_channel_control_proof_signed_by_the_wrong_key_teaches_nothing() {
        use libsecp256k1::{PublicKey, SecretKey};

        let secret = SecretKey::parse(&[7u8; 32]).unwrap();
        let public = PublicKey::from_secret_key(&secret);
        let counterparty = connector_signer::derive_evm_address(&public.serialize());
        let forger_secret = SecretKey::parse(&[42u8; 32]).unwrap();

        let channel_id_bytes = [5u8; 32];
        let channel_id = format!("0x{}", hex::encode(channel_id_bytes));
        let chain_id = 84_532u64;
        let token_network_address = [0x77u8; 20];

        let state = Arc::new(state_over_one_declared_channel(
            &channel_id,
            counterparty,
            chain_id,
            token_network_address,
        ));

        let expires = crate::now_unix() + 3600;
        let forged_signature = sign_channel_control_proof(
            &forger_secret,
            &EvmClaimStateChallenge {
                channel_id: channel_id_bytes,
                expires,
                chain_id,
                token_network_address,
            },
        );

        let frame = auth_message_frame(
            "g.toon.agent",
            Some(serde_json::json!({
                "channelId": channel_id,
                "expires": expires,
                "signature": forged_signature,
            })),
        );
        run_auth_frame(&state, &frame).await;

        let fulfillment = [9u8; 32];
        let payout = state
            .claim_gate
            .credit_session_payout("g.toon.agent", &fulfillment, 500, chrono::Utc::now())
            .await;
        assert!(
            payout.is_none(),
            "a proof signed by the wrong key must not teach the session a channel"
        );
    }

    /// Issue #790's expiry guard, exercised the same way
    /// `/ilp/claim-state`'s own `an_expired_challenge_is_refused_distinctly_from_an_unverified_one`
    /// exercises the identical check on that sibling endpoint: an expired
    /// proof is ignored before any decoding or channel lookup even runs, so
    /// a captured proof cannot be replayed at auth after its `expires` has
    /// passed.
    #[tokio::test]
    async fn an_expired_channel_control_proof_teaches_nothing() {
        use libsecp256k1::{PublicKey, SecretKey};

        let secret = SecretKey::parse(&[7u8; 32]).unwrap();
        let public = PublicKey::from_secret_key(&secret);
        let counterparty = connector_signer::derive_evm_address(&public.serialize());

        let channel_id_bytes = [5u8; 32];
        let channel_id = format!("0x{}", hex::encode(channel_id_bytes));
        let chain_id = 84_532u64;
        let token_network_address = [0x77u8; 20];

        let state = Arc::new(state_over_one_declared_channel(
            &channel_id,
            counterparty,
            chain_id,
            token_network_address,
        ));

        let expires = crate::now_unix().saturating_sub(1);
        let signature = sign_channel_control_proof(
            &secret,
            &EvmClaimStateChallenge {
                channel_id: channel_id_bytes,
                expires,
                chain_id,
                token_network_address,
            },
        );

        let frame = auth_message_frame(
            "g.toon.agent",
            Some(serde_json::json!({
                "channelId": channel_id,
                "expires": expires,
                "signature": signature,
            })),
        );
        run_auth_frame(&state, &frame).await;

        let fulfillment = [9u8; 32];
        let payout = state
            .claim_gate
            .credit_session_payout("g.toon.agent", &fulfillment, 500, chrono::Utc::now())
            .await;
        assert!(
            payout.is_none(),
            "an expired proof must not teach the session a channel"
        );
    }

    /// Issue #792's "also worth adding while here": production's actual
    /// sequence is authenticate bare, open a channel, *then*
    /// re-authenticate to declare it -- not a single auth frame carrying
    /// the proof from the very first connection. `handle_frame`'s auth
    /// branch is shared by every auth frame on a session, so this drives
    /// two frames through it with the same `binding` a real reconnect-free
    /// session would share, proving the credit lands on the *second* auth
    /// rather than only ever being exercised on the first.
    #[tokio::test]
    async fn a_second_auth_on_an_already_bound_session_still_teaches_its_channel() {
        use libsecp256k1::{PublicKey, SecretKey};

        let secret = SecretKey::parse(&[7u8; 32]).unwrap();
        let public = PublicKey::from_secret_key(&secret);
        let counterparty = connector_signer::derive_evm_address(&public.serialize());

        let channel_id_bytes = [5u8; 32];
        let channel_id = format!("0x{}", hex::encode(channel_id_bytes));
        let chain_id = 84_532u64;
        let token_network_address = [0x77u8; 20];

        let state = Arc::new(state_over_one_declared_channel(
            &channel_id,
            counterparty,
            chain_id,
            token_network_address,
        ));

        let (replies, _reply_rx) = mpsc::channel::<Vec<u8>>(REPLY_QUEUE_DEPTH);
        let window = Arc::new(Semaphore::new(4));
        let outbound = Arc::new(OutboundRequests::new());
        let mut binding = None;

        // First auth: a bare bind, no declaration -- nothing to credit yet.
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
        assert_eq!(
            binding.as_ref().map(|(address, _)| address.clone()),
            Some("g.toon.agent".to_string())
        );
        let fulfillment = [9u8; 32];
        assert!(
            state
                .claim_gate
                .credit_session_payout("g.toon.agent", &fulfillment, 500, chrono::Utc::now())
                .await
                .is_none(),
            "a bare bind with no declaration and no claim must not yet be creditable"
        );

        // Second auth on the SAME session (same `binding`, same replies /
        // window / outbound): declares the channel.
        let expires = crate::now_unix() + 3600;
        let signature = sign_channel_control_proof(
            &secret,
            &EvmClaimStateChallenge {
                channel_id: channel_id_bytes,
                expires,
                chain_id,
                token_network_address,
            },
        );
        let declare_frame = auth_message_frame(
            "g.toon.agent",
            Some(serde_json::json!({
                "channelId": channel_id,
                "expires": expires,
                "signature": signature,
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
        assert_eq!(
            binding.as_ref().map(|(address, _)| address.clone()),
            Some("g.toon.agent".to_string())
        );

        let payout = state
            .claim_gate
            .credit_session_payout("g.toon.agent", &fulfillment, 500, chrono::Utc::now())
            .await
            .expect("the re-auth's declaration taught the session's channel");
        assert_eq!(payout.channel_id, channel_id);
    }

    /// [`auth_channel_proof`] requires all three fields; a partial
    /// declaration -- missing a signature to check, or a channel to check
    /// it against -- is not a declaration at all, per that function's own
    /// doc.
    #[test]
    fn auth_channel_proof_requires_all_three_fields() {
        assert!(auth_channel_proof(br#"{"peerId":"g.toon.agent"}"#).is_none());
        assert!(auth_channel_proof(br#"{"channelId":"0xab","expires":1}"#).is_none());
        assert!(auth_channel_proof(b"not json").is_none());

        let proof = auth_channel_proof(
            br#"{"peerId":"g.toon.agent","channelId":"0xab","expires":1,"signature":"0xcd"}"#,
        )
        .expect("all three fields present");
        assert_eq!(proof.channel_id, "0xab");
        assert_eq!(proof.expires, 1);
        assert_eq!(proof.signature, "0xcd");
    }

    /// Issue #779: a session (re)establishing is resent a payout claim
    /// stranded by an earlier failed delivery, with no new job in sight --
    /// driven through the real `handle_frame` auth/bind path (not
    /// `deliver_pending_claim` called directly), so this fails if the auth
    /// branch's [`spawn_stranded_claim_resend`] call is deleted, per the
    /// issue's own AC4.
    ///
    /// `record_session_channel` is called up front to stand in for "this
    /// gate already learned this session's channel before" -- exactly what
    /// a genuine prior claim or channel-control proof on an earlier
    /// connection would have taught it (issues #787/#790); this test is
    /// about the resend, not that association.
    #[tokio::test]
    async fn a_reconnecting_session_is_resent_its_stranded_payout_claim() {
        use crate::claim_gate::ClientClaimGate;
        use crate::outbound_ledger::ClientPayoutLedger;
        use connector_runtime::{ChannelDomain, InMemoryJournal};
        use connector_signer::LocalSigner;

        let address = "g.toon.stranded";
        let channel_id = format!("0x{:064x}", 21);

        let mut ledger = ClientPayoutLedger::new();
        ledger.set_signer(Arc::new(LocalSigner::generate("payout-key")));
        ledger
            .set_channel_domain(
                channel_id.clone(),
                ChannelDomain {
                    chain_id: 84_532,
                    token_network_address: [0x99; 20],
                },
            )
            .expect("valid channel id");
        let stranded = ledger
            .record_payout(&channel_id, 12_345, "2030-01-01T00:00:00Z".parse().unwrap())
            .expect("signer and domain configured");
        let ledger = Arc::new(ledger);

        let gate = ClientClaimGate::restore(Default::default(), Arc::new(InMemoryJournal::new()))
            .expect("a fresh in-memory journal has nothing to replay")
            .with_payout_ledger(Arc::clone(&ledger));
        gate.record_session_channel(address, channel_id.clone());
        let state = Arc::new(test_state(gate));

        let (replies, mut reply_rx) = mpsc::channel::<Vec<u8>>(REPLY_QUEUE_DEPTH);
        let window = Arc::new(Semaphore::new(4));
        let outbound = Arc::new(OutboundRequests::new());
        let mut binding = None;

        let frame = auth_message_frame(address, None);
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
                let pd = decoded
                    .protocol_data
                    .iter()
                    .find(|pd| pd.name == PAYOUT_CLAIM_PROTOCOL)
                    .expect("the stranded claim rode this TRANSFER");
                let json: serde_json::Value = serde_json::from_slice(&pd.data).expect("valid JSON");
                assert_eq!(json["channelId"], channel_id);
                assert_eq!(json["nonce"], stranded.nonce);
                assert_eq!(json["cumulativeAmount"], 12_345);

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
            "a session that reconnects with a known channel must be resent its stranded claim"
        );

        // The resend's acknowledgement runs in the spawned task, after the
        // RESPONSE above wakes its `send_transfer` await -- give the
        // executor a chance to poll it to completion.
        let mut cleared = false;
        for _ in 0..1000 {
            if ledger.pending_claim(&channel_id).is_none() {
                cleared = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            cleared,
            "a successfully delivered resend must acknowledge and clear pending_claim"
        );
        assert_eq!(
            ledger.credited(&channel_id),
            12_345,
            "acknowledging a resend must never disturb credited"
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

        let gate = ClientClaimGate::restore(Default::default(), Arc::new(InMemoryJournal::new()))
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
        assert_eq!(terms.accepts[0].amount, "0");
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
        let gate = crate::claim_gate::ClientClaimGate::restore(
            Default::default(),
            Arc::new(connector_runtime::InMemoryJournal::new()),
        )
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
