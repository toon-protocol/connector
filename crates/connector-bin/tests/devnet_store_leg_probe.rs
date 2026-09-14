//! Devnet acceptance probe for the FORWARDED store leg: a real kind:5094
//! Arweave job, bought at the apex's client edge, carried across the
//! apex<->store peering, terminated by the store box's connector at the
//! payment-oblivious store app, and read back off Arweave by tx id.
//!
//! This is the successor to the deleted `scripts/app/ci-acceptance-probe-store.ts`
//! (retired with the TypeScript connector, ADR 0017). What that probe could
//! not do, and this one does, is cross a PEERING: the route it exercises is
//! `g.toon.ario`, which since #772 is a `peer_id` next hop rather than an
//! outbound HTTPS POST, so a FULFILL here is evidence that two connectors
//! settled with each other for the carriage.
//!
//! ## What a green run proves, in order
//!
//!   1. the terminating connector's identity is NOT the apex's -- the
//!      distinction the whole seal depends on;
//!   2. the apex quotes a price for the forwarded prefix that covers what the
//!      terminating side charges on arrival;
//!   3. an unpaid job is refused with x402 terms and never reaches the app;
//!   4. a PAID, sealed, claim-bearing kind:5094 job FULFILLs, the store app
//!      answers with a real Arweave tx id, and the bytes fetched back from a
//!      public Arweave gateway are byte-identical to what was sent.
//!
//! ## LOCAL / DEV ONLY, and inert unless driven
//!
//! Every test returns immediately unless `STORE_PROBE_EDGE` is set, so an
//! ordinary `cargo test` run never needs a live fleet. The PAID test needs a funded channel as well and stays
//! inert without one; running it SPENDS REAL DEVNET VALUE (one packet, at the
//! quoted price) and ADVANCES THE CHANNEL WATERMARK, so `STORE_PROBE_NONCE` /
//! `STORE_PROBE_CUMULATIVE` must be bumped between runs.
//!
//!   # free checks only
//!   STORE_PROBE_EDGE=https://proxy.devnet.toonprotocol.dev \
//!   STORE_PROBE_TERMINUS=https://proxy.ario.devnet.toonprotocol.dev \
//!     cargo test -p connector --test devnet_store_leg_probe -- --nocapture
//!
//!   # ...plus the paid round trip (adds one packet's cost to the channel)
//!   STORE_PROBE_PAYER_KEY=<32-byte hex, NEVER committed> \
//!   STORE_PROBE_CHANNEL=0x... \
//!   STORE_PROBE_TOKEN_NETWORK=0x... \
//!   STORE_PROBE_CHAIN_ID=84532 \
//!   STORE_PROBE_NONCE=<previous + 1> \
//!   STORE_PROBE_CUMULATIVE=<previous + price> \
//!     cargo test -p connector --test devnet_store_leg_probe -- --nocapture
//!
//! Nothing here is hardcoded to one deployment and no key material is
//! committed: every address, price, prefix and gateway is read from the
//! environment or from the live edge itself.
//!
//! ## The traps this file exists to keep written down
//!
//! Each cost a working afternoon when the leg was first proven by hand, and
//! each is invisible from a passing run:
//!
//!   * **Seal to the TERMINATING connector, never the forwarding one.** The
//!     gift wrap (ADR 0018) is opened by the node that terminates the route,
//!     and a forwarding hop does not hold that key -- it carries the wrap as
//!     opaque bytes. Sealing a forwarded packet to the apex's identity buys a
//!     packet that crosses the peering and then fails F01 "gift wrap could not
//!     be opened" at the far end, with the money already spent.
//!     `the_terminating_connector_is_a_different_node_from_the_forwarding_one`
//!     asserts the two identities actually differ, so a probe pointed at one
//!     URL twice fails loudly instead of proving nothing.
//!
//!   * **There is no execution condition to get right, or wrong (issue
//!     #1269 / ADR 0069).** The PREPARE carries none; the terminating node
//!     derives its fulfilment straight from the shared secret the gift wrap
//!     carries, and this probe's own end-to-end check compares the FULFILL
//!     it gets back against `derive_fulfillment(&secret)` -- the workspace's
//!     own signer, so there is no second implementation to drift.
//!
//!   * **The PREPARE encoding is not stock ILPv4.** The amount is an OER
//!     `VarUInt` (not a fixed `uint64`), the expiry is a 19-byte ASN.1
//!     `GeneralizedTime` `YYYYMMDDHHMMSS.fffZ` (the 17-char Interledger form is
//!     refused as "invalid ASN.1 GeneralizedTime"), and there is no outer
//!     length prefix on the packet. None of that is spelled out below on
//!     purpose: this probe calls `Prepare::encode` from `connector-domain`,
//!     the same code the connector itself parses with. Re-deriving the
//!     encoding in a probe is how a probe ends up testing itself.
//!
//!   * **An envelope `target` of `""` means "the route's own handler path".**
//!     The store route's `handler_url` already ends in `/store`
//!     (`infra/linode-store/connector-rust.toml`), and `resolve_target_under_handler`
//!     appends the target BENEATH it -- an absolute `/store` is accepted only
//!     as the literal restatement it is (issue #621), and anything else
//!     absolute is refused. `""` is what a client with exactly one endpoint
//!     should send.
//!
//!   * **The price arithmetic is a subtraction, and it bites as an F03.** The
//!     apex collects `price` and forwards `price - fee`; the terminating side
//!     charges its OWN price on arrival (#754). `g.toon.ario` is 1002/fee 2 at
//!     the apex precisely so 1000 lands, which is the store box's price. At
//!     1000/fee 2 the far side would receive 998 and refuse -- an F03 that
//!     looks like a client bug and is a config bug.
//!     `the_apex_price_covers_what_the_terminating_side_charges_on_arrival`
//!     checks that subtraction against both live edges rather than against a
//!     comment.
//!
//!   * **The claim's `timestamp` must end in `Z`.** The claim gate refuses a
//!     `+00:00` offset by name, and `chrono`'s plain `to_rfc3339()` emits
//!     exactly that. Cheap to hit and free to learn -- a structurally invalid
//!     claim is refused before the packet is forwarded, so nothing is charged.
//!
//!   * **A claim nonce must ADVANCE the channel's watermark.** Replaying one
//!     comes back "nonce does not advance this channel's watermark (replay)",
//!     and the connector journals the watermark durably (`state_dir`), so it
//!     survives restarts and cannot be reset by redeploying.
//!
//!   * **A fresh Arweave upload is not instantly on every gateway.** The
//!     `@toon-protocol/arweave` list is ar.io-first, and a tx that answers 200
//!     on one gateway can still 404 on another for minutes after the upload.
//!     The read-back below sweeps the list repeatedly rather than trusting the
//!     first host, which is exactly what a real client does. A read-back that
//!     times out does NOT mean the paid write failed -- the FULFILL already
//!     proved that -- so re-check the tx id by hand before spending again.
//!
//!   * **A `User-Agent` is not optional on `arweave.net`.** Its CDN answers
//!     403 to a request without one, which `reqwest` omits by default, and a
//!     403 looks exactly like a gateway refusing to serve a tx that does not
//!     exist. `curl` sends a UA, which is why a hand-check "works" against a
//!     probe that appears not to.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chrono::{Duration as ChronoDuration, SecondsFormat, Utc};
use connector_domain::{EnvelopeRequest, EnvelopeResponse, Fulfill, Prepare, Price, Reject};
use connector_signer::giftwrap::{derive_fulfillment, open_response, seal_request};
use connector_signer::{
    derive_evm_address, evm_balance_proof_digest, to_hex, EvmBalanceProof, LocalSigner,
    PublicKeyBytes, Signer,
};

const CLAIM_HEADER: &str = "ilp-payment-channel-claim";

/// NIP-90 Arweave blob-storage job (`BLOB_STORAGE_REQUEST_KIND` in
/// `@toon-protocol/core`), the one kind the store app's default handler serves.
const BLOB_STORAGE_REQUEST_KIND: u64 = 5094;

/// The ordered Arweave gateway preference list, ar.io first -- the same order
/// and the same hosts as `@toon-protocol/arweave`'s `ARWEAVE_GATEWAYS`
/// (toon-client `packages/arweave/src/gateways.ts`), which is the single
/// source of truth every TOON client reads through. Overridable with
/// `STORE_PROBE_GATEWAYS` so a run can be pointed at a private gateway.
const DEFAULT_ARWEAVE_GATEWAYS: &[&str] = &[
    "https://ar-io.dev",
    "https://arweave.net",
    "https://permagate.io",
];

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// The FORWARDING connector's client edge -- the apex, where a buyer arrives
/// and pays. `None` makes every test in this file a no-op.
fn edge() -> Option<String> {
    env("STORE_PROBE_EDGE")
}

/// The TERMINATING connector's client edge -- the store box, whose identity
/// the packet is sealed to and whose route price must be covered. Note this is
/// only ever read for its `/ilp/identity` and `/ilp/routes/price`: the probe
/// never sends it a packet directly, because the whole point is to go through
/// the apex.
fn terminus() -> String {
    env("STORE_PROBE_TERMINUS")
        .unwrap_or_else(|| "https://proxy.ario.devnet.toonprotocol.dev".to_string())
}

/// The forwarded prefix under test. `g.toon.ario` is the name a real client
/// addresses -- buzz pins it in compiled code -- so it is the one worth
/// proving.
fn destination() -> String {
    env("STORE_PROBE_DESTINATION").unwrap_or_else(|| "g.toon.ario".to_string())
}

/// The envelope target. `""` resolves to the route's own `handler_url` path
/// (`http://store:3300/store`); see the module docs.
fn target() -> String {
    env("STORE_PROBE_TARGET").unwrap_or_default()
}

fn gateways() -> Vec<String> {
    match env("STORE_PROBE_GATEWAYS") {
        Some(list) => list
            .split(',')
            .map(|g| g.trim().trim_end_matches('/').to_string())
            .filter(|g| !g.is_empty())
            .collect(),
        None => DEFAULT_ARWEAVE_GATEWAYS
            .iter()
            .map(|g| (*g).to_string())
            .collect(),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> Vec<u8> {
    let s = s.trim_start_matches("0x");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

// ── reading the live edges ───────────────────────────────────────────────────

/// A connector's own identity, read from the running node the way a real
/// sender learns it. Never reconstructed from a key file: what this probe
/// seals to is genuinely what the deployed process holds.
async fn fetch_identity(base: &str) -> PublicKeyBytes {
    let body: serde_json::Value = reqwest::get(format!("{base}/ilp/identity"))
        .await
        .expect("GET /ilp/identity")
        .json()
        .await
        .expect("identity json");
    let bytes = hex_decode(body["publicKey"].as_str().expect("publicKey"));
    bytes.as_slice().try_into().expect("65-byte public key")
}

/// What an edge charges a client for `destination`, from its own greeting --
/// so the probe's arithmetic is checked against the running config, not
/// against a number written down here.
///
/// The answer is a schedule, not a number (ADR 0065): `price` is the flat
/// part and `price_per_kib` the slope, and what a packet is charged is
/// `Price::charge(data.len())`, evaluated on the SEALED payload. A flat route
/// answers with no slope, which is the zero-slope schedule. Issue #1265: this
/// used to return the flat part alone, and against a scheduled route the
/// probe then either under-paid the far hop (F03 on arrival, the client
/// refunded and the forwarding operator eating it) or, if it guessed high,
/// was refused by the edge for declaring more than the charge.
async fn fetch_price(base: &str, destination: &str) -> Price {
    let body: serde_json::Value =
        reqwest::get(format!("{base}/ilp/routes/price?destination={destination}"))
            .await
            .expect("GET /ilp/routes/price")
            .json()
            .await
            .expect("price json");
    let flat = body["price"]
        .as_u64()
        .unwrap_or_else(|| panic!("no price for {destination}: {body}"));
    let per_kib = body["price_per_kib"].as_u64().unwrap_or(0);
    Price::scheduled(flat, per_kib)
}

/// The one-KiB point on a schedule: what any sealed packet of up to 1024
/// bytes is charged. The probe's blobs are a few hundred bytes plus the seal,
/// so this is the number that goes into a job's `bid` before the packet
/// exists; [`sealed_prepare`] then charges the real length, and the paid test
/// asserts the two agree rather than spending on a guess.
fn one_kib_charge(schedule: &Price) -> u64 {
    schedule.charge(1024)
}

// ── the kind:5094 job ────────────────────────────────────────────────────────

/// A genuinely signed kind:5094 Arweave blob-storage job.
///
/// THE BODY SHAPE IS THE POINT OF THIS FUNCTION. The store app
/// (`store` repo, `src/store-backend.ts`, `POST /store`) wants
/// `{"event": <signed nostr event>}` and answers 422
/// `{"accept":false,"code":"F00","message":"Invalid request body"}` to anything
/// else -- including a perfectly good blob sent as raw bytes, which is the
/// failure this probe was written to stop recurring. The event's tags are
/// `@toon-protocol/core`'s `buildBlobStorageRequest` layout, which the store's
/// handler parses with `parseBlobStorageRequest`:
///
///   ["i",      <base64 of the blob>, "blob"]   REQUIRED; the third element
///                                              must literally be "blob"
///   ["bid",    <usdc micro-units>,   "usdc"]   REQUIRED, non-empty
///   ["output", <content type>]                 optional; defaults to
///                                              application/octet-stream
///
/// plus optional `["param", key, value]` rows for chunked uploads
/// (`uploadId` -- a v4 UUID or it is ignored -- `chunkIndex`, `totalChunks`).
/// `content` is empty: the blob travels in the `i` tag, not in the content.
///
/// The store verifies the Schnorr signature (`verifyEvent`, devMode off), so a
/// made-up event is refused -- which is what makes "the store accepted it" a
/// statement about a real event. BIP-340 Schnorr over the event's own SHA-256
/// id: neither `libsecp256k1` 0.6 nor `connector-signer` signs that way, hence
/// `k256`'s `schnorr` here.
fn signed_blob_storage_job(
    blob: &[u8],
    content_type: &str,
    bid: u64,
) -> (serde_json::Value, String) {
    use k256::schnorr::signature::hazmat::PrehashSigner;
    use k256::schnorr::SigningKey;
    use sha2::{Digest, Sha256};

    // A per-run key: this event is a receipt for one upload and is never
    // republished, so there is nothing to gain by making it stable, and a
    // fixed key would make two concurrent runs indistinguishable.
    let mut seed = [0u8; 32];
    seed[..8].copy_from_slice(&now_secs().to_be_bytes());
    seed[8..24].copy_from_slice(&Sha256::digest(blob)[..16]);
    seed[31] |= 1; // never the zero scalar
    let signing_key = SigningKey::from_bytes(&seed).expect("nostr signing key");
    let pubkey = hex_encode(&signing_key.verifying_key().to_bytes());
    let created_at = now_secs();

    let tags = serde_json::json!([
        ["i", BASE64.encode(blob), "blob"],
        ["bid", bid.to_string(), "usdc"],
        ["output", content_type],
    ]);

    // NIP-01's serialization for the id: the six-element array, no whitespace.
    let serialized =
        serde_json::json!([0, pubkey, created_at, BLOB_STORAGE_REQUEST_KIND, tags, ""]).to_string();
    let id = hex_encode(&Sha256::digest(serialized.as_bytes()));

    let signature: k256::schnorr::Signature = signing_key
        .sign_prehash(&hex_decode(&id))
        .expect("schnorr sign the event id");

    let event = serde_json::json!({
        "id": id,
        "pubkey": pubkey,
        "created_at": created_at,
        "kind": BLOB_STORAGE_REQUEST_KIND,
        "tags": tags,
        "content": "",
        "sig": hex_encode(&signature.to_bytes()),
    });
    (event, id)
}

/// The store app's `POST /store` body: `{"event": <event>}` and nothing else.
fn store_job_body(event: &serde_json::Value) -> Vec<u8> {
    serde_json::json!({ "event": event })
        .to_string()
        .into_bytes()
}

// ── the packet ───────────────────────────────────────────────────────────────

/// A `Prepare` a real sender forms: an OER `EnvelopeRequest` gift-wrapped to
/// the TERMINATING connector's identity (ADR 0018), which derives its
/// fulfilment from that same wrap's shared secret (ADR 0019) -- there is no
/// execution condition to mint any more (ADR 0069).
///
/// `identity` is deliberately a parameter rather than something this function
/// fetches: the ONE thing a forwarded packet gets wrong is sealing to the hop
/// instead of the terminus, and passing it in keeps that choice visible at
/// every call site.
fn sealed_prepare(
    schedule: &Price,
    destination: &str,
    target: &str,
    body: &[u8],
    identity: &PublicKeyBytes,
) -> (Prepare, [u8; 32]) {
    let plaintext = EnvelopeRequest {
        method: "POST".to_string(),
        target: target.to_string(),
        headers: vec![("content-type".to_string(), "application/json".to_string())],
        body: body.to_vec(),
    }
    .encode();
    let (data, shared_secret) = seal_request(&plaintext, identity).expect("seal");
    // The edge charges `schedule.charge(data.len())` on the sealed bytes and
    // refuses an amount that differs from it in EITHER direction, so the
    // amount is decided here, after the seal, where the length is known.
    let amount = schedule.charge(data.len());
    (
        Prepare {
            amount,
            // Generous, because this packet crosses a peering and an Arweave
            // upload before it can be answered.
            expires_at: Utc::now() + ChronoDuration::minutes(2),
            greeting: false,
            destination: destination.to_string(),
            data,
        },
        shared_secret,
    )
}

// ── the claim ────────────────────────────────────────────────────────────────

/// Everything needed to sign one claim against an already-open, already-funded
/// channel. Read wholly from the environment: this probe opens no channel,
/// touches no faucet, and holds no key of its own.
struct Payer {
    secret_hex: String,
    channel_id: [u8; 32],
    token_network: [u8; 20],
    chain_id: u64,
    nonce: u64,
    cumulative: u128,
}

impl Payer {
    /// `None` unless a funded channel is fully described, which is what keeps
    /// the paid test inert -- and therefore free -- by default.
    fn from_env() -> Option<Payer> {
        let secret_hex = env("STORE_PROBE_PAYER_KEY")?;
        let channel_id: [u8; 32] = hex_decode(&env("STORE_PROBE_CHANNEL")?)
            .try_into()
            .expect("STORE_PROBE_CHANNEL is a 32-byte channel id");
        let token_network: [u8; 20] = hex_decode(&env("STORE_PROBE_TOKEN_NETWORK")?)
            .try_into()
            .expect("STORE_PROBE_TOKEN_NETWORK is a 20-byte address");
        Some(Payer {
            secret_hex,
            channel_id,
            token_network,
            chain_id: env("STORE_PROBE_CHAIN_ID")?.parse().expect("chain id"),
            // No defaults for these two on purpose: guessing a watermark
            // either replays (refused) or silently overpays.
            nonce: env("STORE_PROBE_NONCE")?.parse().expect("nonce"),
            cumulative: env("STORE_PROBE_CUMULATIVE")?
                .parse()
                .expect("cumulative transferred amount"),
        })
    }

    fn signer(&self) -> LocalSigner {
        let secret: [u8; 32] = hex_decode(&self.secret_hex)
            .try_into()
            .expect("STORE_PROBE_PAYER_KEY is a 32-byte hex secret");
        LocalSigner::from_secret_bytes("store-probe", secret).expect("signer")
    }

    fn address(&self) -> [u8; 20] {
        derive_evm_address(&self.signer().public_key().expect("public key"))
    }

    fn proof(&self) -> EvmBalanceProof {
        EvmBalanceProof {
            channel_id: self.channel_id,
            nonce: self.nonce,
            transferred_amount: self.cumulative,
            locked_amount: 0,
            locks_root: [0u8; 32],
            chain_id: self.chain_id,
            token_network_address: self.token_network,
        }
    }

    /// The client-edge claim JSON, signed through this workspace's PRODUCTION
    /// signing path (`Signer::sign` + `Signature::to_bytes`), whose byte 64 is
    /// libsecp256k1's raw recovery id in `{0,1}`. Deliberately no `+27`: issue
    /// #590/#591 moved that normalisation to the settlement boundary, and a
    /// probe that pre-shifted the byte would prove nothing about whether the
    /// boundary does its job.
    fn claim_json(&self) -> String {
        let proof = self.proof();
        let signature = self
            .signer()
            .sign(&evm_balance_proof_digest(&proof))
            .expect("sign")
            .to_bytes();
        format!(
            r#"{{
                "version": "1.0",
                "blockchain": "evm",
                "messageId": "store-probe-{nonce}",
                "timestamp": "{timestamp}",
                "senderId": "{address}",
                "channelId": "0x{channel_id}",
                "nonce": {nonce},
                "transferredAmount": "{amount}",
                "lockedAmount": "0",
                "locksRoot": "0x{zeros}",
                "signature": "0x{signature}",
                "signerAddress": "{address}",
                "chainId": {chain_id},
                "tokenNetworkAddress": "{token_network}"
            }}"#,
            nonce = proof.nonce,
            // `Z`, not `+00:00`. The claim gate refuses a `+00:00` offset
            // outright -- "'timestamp' must be ISO 8601 with a 'Z' timezone" --
            // and `chrono`'s plain `to_rfc3339()` produces exactly the spelling
            // it rejects. This is a structural refusal at the gate, so the
            // packet is never forwarded and nothing is charged, but it is an
            // easy half hour to lose.
            timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            channel_id = hex_encode(&proof.channel_id),
            amount = proof.transferred_amount,
            zeros = "0".repeat(64),
            signature = hex_encode(&signature),
            address = to_hex(&self.address()),
            chain_id = proof.chain_id,
            token_network = to_hex(&proof.token_network_address),
        )
    }
}

// ── reading the payload back off Arweave ─────────────────────────────────────

/// How long the read-back keeps trying before giving up, and how long it
/// waits between sweeps of the gateway list. A tx that has just been uploaded
/// is typically servable within a couple of minutes, but this is a public
/// network and the figure is not a guarantee -- override with
/// `STORE_PROBE_ARWEAVE_ATTEMPTS` rather than editing.
const ARWEAVE_READBACK_ATTEMPTS: usize = 12;
const ARWEAVE_READBACK_INTERVAL_SECS: u64 = 20;

/// Fetch `tx_id` from the ar.io-first gateway list, walking the whole list and
/// then retrying it.
///
/// Two behaviours here are load-bearing and both look like failures the first
/// time:
///
///   * a just-uploaded tx propagates UNEVENLY. One gateway answering 404 while
///     another answers 200 is normal for the first minutes and is not evidence
///     the upload failed -- so the list is swept repeatedly, and only the whole
///     window expiring counts as a miss;
///   * `arweave.net` sits behind a CDN that answers **403** to a request with
///     no `User-Agent`, which `reqwest` omits by default. That 403 is not
///     Arweave saying "no such tx"; it is the CDN saying "no such client". Send
///     a UA and it serves the bytes.
///
/// Returns the first bytes any gateway serves, with the host that served them.
async fn fetch_from_arweave(tx_id: &str) -> Option<(String, Vec<u8>)> {
    let attempts: usize = env("STORE_PROBE_ARWEAVE_ATTEMPTS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(ARWEAVE_READBACK_ATTEMPTS);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(concat!(
            "toon-connector-store-probe/",
            env!("CARGO_PKG_VERSION")
        ))
        // Gateways answer a 302 to a base32 sandbox subdomain before serving
        // the bytes, so redirects must be followed -- an un-followed 302 is a
        // "not found" that isn't.
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .expect("http client");

    for attempt in 1..=attempts {
        for gateway in gateways() {
            let url = format!("{gateway}/{tx_id}");
            match client.get(&url).send().await {
                Ok(response) if response.status().is_success() => {
                    let bytes = response.bytes().await.expect("gateway body").to_vec();
                    return Some((gateway, bytes));
                }
                Ok(response) => {
                    println!(
                        "  {url} -> HTTP {} (attempt {attempt}/{attempts})",
                        response.status()
                    );
                }
                Err(err) => println!("  {url} -> {err} (attempt {attempt}/{attempts})"),
            }
        }
        if attempt < attempts {
            tokio::time::sleep(std::time::Duration::from_secs(
                ARWEAVE_READBACK_INTERVAL_SECS,
            ))
            .await;
        }
    }
    None
}

// ── 1. the two connectors are genuinely two nodes ────────────────────────────

/// The distinction the seal depends on. A forwarding hop carries the gift wrap
/// as opaque bytes and cannot open it, so a packet sealed to the APEX's
/// identity is bought, forwarded, and then rejected F01 at the far end. If a
/// misconfigured run pointed `STORE_PROBE_TERMINUS` at the apex, every other
/// test here would still pass while proving nothing about a peering -- so the
/// difference is asserted first, before any money is spent.
#[tokio::test]
async fn the_terminating_connector_is_a_different_node_from_the_forwarding_one() {
    let Some(edge) = edge() else { return };
    let forwarding = fetch_identity(&edge).await;
    let terminating = fetch_identity(&terminus()).await;

    assert_eq!(terminating.len(), 65);
    assert_eq!(terminating[0], 0x04, "uncompressed secp256k1 key");
    assert_ne!(
        forwarding, terminating,
        "the terminus must be a DIFFERENT node from the hop -- seal to the one \
         that opens the wrap, or buy an F01"
    );
    println!("forwarding hop : 0x{}", hex_encode(&forwarding));
    println!("terminating    : 0x{}", hex_encode(&terminating));
}

// ── 2. the price arithmetic across the hop ───────────────────────────────────

/// ADR 0028's arithmetic, checked against both live edges. The apex collects
/// its own price and forwards `price - fee`; since #754 the terminating side
/// charges its own price on ARRIVAL, so the forwarded amount must be at least
/// what the far side wants. `g.toon.ario` is priced 1002/fee 2 at the apex for
/// exactly this reason -- 1000/fee 2 would forward 998 into a route priced
/// 1000 and every packet would come home F03 "insufficient amount", which
/// reads like a client bug and is a config bug.
///
/// The fee is not published by either edge, so this asserts the reachable
/// half: the apex charges at least what the terminus charges. A hop that
/// charged less could not possibly forward enough.
#[tokio::test]
async fn the_apex_price_covers_what_the_terminating_side_charges_on_arrival() {
    let Some(edge) = edge() else { return };
    let destination = destination();
    let hop_price = one_kib_charge(&fetch_price(&edge, &destination).await);
    let terminus_price = one_kib_charge(&fetch_price(&terminus(), &destination).await);

    println!("{destination}: apex charges {hop_price}, terminus charges {terminus_price}");
    assert!(hop_price > 0, "a free forwarded route is a free gateway");
    assert!(
        hop_price >= terminus_price,
        "the apex charges {hop_price} for {destination} but the terminating side \
         wants {terminus_price} on arrival -- the forward cannot cover it, and \
         every packet will come home F03"
    );
}

// ── 3. an unpaid job is refused before the app is asked to work ──────────────

#[tokio::test]
async fn an_unpaid_store_job_is_answered_with_x402_terms_and_never_reaches_the_app() {
    let Some(edge) = edge() else { return };
    let identity = fetch_identity(&terminus()).await;
    let (event, id) = signed_blob_storage_job(
        b"an unpaid job, which must never be stored",
        "text/plain",
        1,
    );
    let (prepare, _secret) = sealed_prepare(
        &fetch_price(&edge, &destination()).await,
        &destination(),
        &target(),
        &store_job_body(&event),
        &identity,
    );

    let response = reqwest::Client::new()
        .post(format!("{edge}/ilp"))
        .body(prepare.encode())
        .send()
        .await
        .expect("POST /ilp");

    assert_eq!(response.status().as_u16(), 402, "issue #526's guarantee");
    assert!(response.headers().contains_key("payment-required"));
    let terms: serde_json::Value = response.json().await.expect("x402 terms");
    assert_eq!(terms["x402Version"], 2);
    assert_eq!(terms["accepts"][0]["scheme"], "toon-channel");
    // No OER packet came back at all, so the forwarding decision was never
    // reached and no peering carried anything for free.
    assert!(Prepare::decode(terms.to_string().as_bytes()).is_err());
    println!("unpaid kind:5094 job {id} REFUSED with x402 terms: {terms}");
}

// ── 4. the paid round trip, and the bytes back off Arweave ───────────────────

/// The whole leg in one run, and the only test here that spends anything: a
/// paid, sealed kind:5094 job crosses the apex<->store peering, the store app
/// uploads the blob to Arweave and answers with a tx id, and the bytes fetched
/// back from a public gateway are compared to the bytes that were sent.
///
/// What makes the round trip evidence about a PEERING rather than about one
/// node: the wrap is sealed to the terminating connector's identity, so only
/// that node can have opened it; the fulfilment is derived from that wrap's
/// shared secret, so only that node can have produced the FULFILL; and the tx
/// id in the answer is a real Arweave id whose bytes are checked. To see the
/// value move as well, read the claim journals on both boxes around a run --
/// `docker exec <connector> cat /app/state/peer-claims.log` -- where the
/// forwarding side's `outbound_claim_signed` and the terminating side's
/// `inbound_claim_accepted` advance in step by the forwarded amount.
#[tokio::test]
async fn a_paid_kind_5094_job_crosses_the_peering_and_the_payload_reads_back_from_arweave() {
    let Some(edge) = edge() else { return };
    let Some(payer) = Payer::from_env() else {
        println!("no funded channel in the environment -- paid round trip SKIPPED (nothing spent)");
        return;
    };

    let schedule = fetch_price(&edge, &destination()).await;
    let price = one_kib_charge(&schedule);
    let identity = fetch_identity(&terminus()).await;

    // Unique per run, so the tx id fetched back can only be this run's upload.
    let blob = format!(
        "connector devnet store-leg probe\n{}\nrun {}\n",
        Utc::now().to_rfc3339(),
        now_secs()
    )
    .into_bytes();
    let (event, id) = signed_blob_storage_job(&blob, "text/plain", price);
    let (prepare, shared_secret) = sealed_prepare(
        &schedule,
        &destination(),
        &target(),
        &store_job_body(&event),
        &identity,
    );
    // The bid inside the sealed event was written before the packet existed,
    // at the one-KiB point; the amount was charged on the sealed length. If
    // they differ the blob outgrew a KiB and the run stops here, before any
    // claim is signed, rather than paying one number and bidding another.
    assert_eq!(
        prepare.amount,
        price,
        "the sealed packet is {} bytes and is charged {} by the schedule, but the job's bid \
         was written at the one-KiB point ({price}) -- the probe's blob has outgrown a KiB; \
         raise the bid's assumption rather than spending on a mismatch",
        prepare.data.len(),
        prepare.amount
    );
    println!(
        "paying {price} for {} -- kind:5094 event {id}, {} byte blob",
        destination(),
        blob.len()
    );

    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .expect("http client")
        .post(format!("{edge}/ilp"))
        .header(CLAIM_HEADER, BASE64.encode(payer.claim_json().as_bytes()))
        .body(prepare.encode())
        .send()
        .await
        .expect("POST /ilp");
    assert_eq!(response.status().as_u16(), 200);
    let bytes = response.bytes().await.expect("body");

    let fulfill = match Fulfill::decode(&bytes) {
        Ok(fulfill) => fulfill,
        Err(_) => {
            let reject = Reject::decode(&bytes).expect("neither FULFILL nor REJECT");
            // F03 here is almost always the price subtraction above; F01 is
            // almost always a wrap sealed to the hop instead of the terminus;
            // and a "replay" message means the watermark did not advance.
            panic!(
                "expected a FULFILL, got REJECT {} from {}: {}",
                reject.code.as_str(),
                reject.triggered_by,
                reject.message
            );
        }
    };
    // The connector derived this itself from the wrap's shared secret (ADR
    // 0019) -- the store app holds no secret and performs no cryptography, so
    // only the node this packet was sealed to can have produced it.
    assert_eq!(fulfill.fulfillment, derive_fulfillment(&shared_secret));

    let opened = open_response(&shared_secret, &fulfill.data).expect("open the sealed answer");
    let envelope = EnvelopeResponse::decode(&opened).expect("decode envelope");
    let answer: serde_json::Value = serde_json::from_slice(&envelope.body).unwrap_or_else(|_| {
        panic!(
            "the store app's answer was not JSON (status {}): {}",
            envelope.status,
            String::from_utf8_lossy(&envelope.body)
        )
    });
    assert_eq!(
        envelope.status, 200,
        "the store app refused the job: {answer}"
    );
    assert_eq!(answer["accept"], true, "{answer}");
    let tx_id = answer["txId"]
        .as_str()
        .unwrap_or_else(|| panic!("no Arweave txId in the store's answer: {answer}"))
        .to_string();
    println!("FULFILL -- store answered {answer}");

    // The read back: content-addressed bytes, from a gateway that had nothing
    // to do with the upload.
    let (gateway, fetched) = fetch_from_arweave(&tx_id)
        .await
        .unwrap_or_else(|| panic!("no gateway served {tx_id} -- see the attempts above"));
    assert_eq!(
        fetched,
        blob,
        "{gateway} served {} bytes for {tx_id}, but they are not the bytes that were paid to store",
        fetched.len()
    );
    println!(
        "READ BACK -- {gateway}/{tx_id} served {} bytes, byte-identical to what was sent",
        fetched.len()
    );
}
