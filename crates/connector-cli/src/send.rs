//! `connector send` -- originate a packet through a node's operator surface.
//!
//! The connector could already originate a packet: `POST /packets` on the
//! operator router "originates a packet outward, exactly as the client edge
//! does for an external caller" (ADR 0008). What it could not do was *form*
//! one. The body of that request is an OER-encoded `Prepare` whose payload is
//! gift-wrapped to the terminating connector's identity (ADR 0018), and the
//! request itself carries an RFC 9421 signature from a key on the node's
//! `[operator] write_keys` allowlist. Assembling all of that by hand is why
//! nobody drove a node this way.
//!
//! The `Prepare` carries no execution condition (issue #1269 / ADR 0069):
//! this is where the sender's own end-to-end check lives instead. [`send`]
//! compares the returned FULFILL's fulfilment against
//! `derive_fulfillment(&shared_secret)` -- the same derivation the
//! terminating connector uses (ADR 0019) -- and reports
//! [`Outcome::FulfilledWithWrongFulfillment`] rather than trusting a hop's
//! word that the packet was genuinely delivered.
//!
//! This is the missing half, and it is deliberately the *same* half a test
//! would have written privately: one implementation, in the shipped binary,
//! so what the local topologies exercise is what an operator can run.
//!
//! # What this is not
//!
//! Not a client SDK. ADR 0012 puts end-user key handling, recovery and wallet
//! concerns in `toon-client`, and none of that is here: this holds no channel,
//! signs no claim, and has no identity of its own beyond the operator key it
//! is handed. It is an operator tool that drives an operator endpoint with an
//! operator key -- the same category as `connector announce`.
//!
//! # Why `--seal-to` is separate from `--operator`
//!
//! A payload is sealed to the connector that will *terminate* it, which in a
//! multi-hop topology is not the node the packet is handed to. ADR 0050
//! publishes that node's identity on its own URL, but a client still has no
//! way to *discover* which URL that is from the destination address alone
//! (that half is ADR 0054's, not built) -- so the caller names it directly.
//!
//! `--seal-to` takes that URL as ADR 0050 defines it: the one whose `GET`
//! answers the self-description, e.g. `http://host:3000/ilp` -- the same
//! spelling `POST /peers` takes for a peer's URL, never an origin.
//!
//! # Two URLs, judged one at a time
//!
//! This verb dials exactly twice -- the `--operator` surface it signs a
//! write to, and the `--seal-to` node whose self-description it reads -- and
//! ADR 0070 decision 5 applies the carriage's host-selected rule to **both**,
//! independently. An endpoint whose host ends in `.onion` leaves through
//! `--socks-proxy`; every other endpoint is dialed direct; and nothing else
//! participates in the choice. So an onion node whose terminating peer is
//! also onion-only is probeable in one command, and a clearnet URL passed
//! alongside an onion one still goes direct.
//!
//! The flag is the whole configuration surface, and its value comes from the
//! argument vector and nowhere else: `send` loads no config file, so the
//! `socks_proxy` key cannot reach it, and no environment variable is
//! consulted for it either -- see [`crate::parse_socks_proxy`] for why that
//! is a decision rather than an omission.

use std::path::Path;

use chrono::{Duration, Utc};
use connector_config::is_onion_endpoint;
use connector_domain::{EnvelopeRequest, EnvelopeResponse, Fulfill, Prepare, Reject};
use connector_operator::signing::{keyid_hex, sign_request};
use connector_signer::giftwrap::{derive_fulfillment, open_response, seal_request};
use connector_signer::PublicKeyBytes;
use url::Url;

/// The path the operator router mounts packet origination at.
const PACKETS_PATH: &str = "/packets";

/// Why an onion URL cannot be dialed by an invocation that passed no
/// `--socks-proxy` (ADR 0070 decision 5).
///
/// An ordinary dial failure and nothing more exotic: a `.onion` or
/// `.anyone` name resolves nowhere without a proxy to resolve it, so this
/// reads like every
/// other "could not reach that host" -- and it is raised *before* any lookup,
/// so the operator is not handed a resolver error about a name that is
/// spelled correctly and that nothing here should have looked up.
const NO_SOCKS_PROXY: &str = "the URL's host ends in .onion or .anyone and this invocation \
     passed no --socks-proxy, so there is nothing that can resolve or reach it. Pass \
     '--socks-proxy socks5h://<host>:<port>' naming a SOCKS5 proxy onto that network (ADR 0070 \
     decision 5)";

/// How long a signed operator write stays valid. Short on purpose: the
/// signature is replay-rejected by the node anyway, and a long window is a
/// larger thing to leak.
const SIGNATURE_TTL_SECONDS: u64 = 60;

/// Everything `connector send` needs, after argument parsing and before
/// anything has been read from disk or the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendOptions {
    /// Base URL of the node whose operator surface originates the packet.
    pub operator_url: String,
    /// File holding the ed25519 secret key whose public half is on that
    /// node's `[operator] write_keys`. 32 raw bytes or 64 hex characters,
    /// matching every other key file this binary reads (ADR 0012 -- a
    /// location, never an inline value).
    pub operator_key_file: String,
    /// The ILP address the packet is bound for.
    pub destination: String,
    /// The packet's amount. Each hop it crosses takes that peering's flat
    /// fee out of it (ADR 0010), so this must cover the terminating side's
    /// price plus every fee on the way; the packet declares no floor of its
    /// own (ADR 0057).
    pub amount: u64,
    /// The connector that will TERMINATE this packet, as its self-description
    /// URL (ADR 0050) -- the one whose `GET` answers with the identity the
    /// payload is sealed to, e.g. `http://host:3000/ilp`. See the module
    /// header.
    pub seal_to: String,
    /// The envelope's request target, as the terminating app will see it.
    pub target: String,
    /// The envelope's request method.
    pub method: String,
    /// The envelope's request body.
    pub body: Vec<u8>,
    /// How long the PREPARE is valid for.
    pub expires_in_seconds: i64,
    /// The SOCKS5 proxy an **onion** URL is dialed through, and `None` on an
    /// invocation that named none (ADR 0070 decision 5).
    ///
    /// One value, applied per URL by host: it is not a mode that reroutes
    /// everything. `None` is not an error condition -- it is the ordinary
    /// case, and it only fails a dial if one of the two URLs turns out to be
    /// an onion one.
    ///
    /// Already validated by [`crate::parse_socks_proxy`]: `socks5h`, and
    /// naming a host. Nothing downstream re-checks it.
    pub socks_proxy: Option<Url>,
    /// Form and sign everything, print it, and send nothing.
    pub dry_run: bool,
    /// Treat anything other than a correctly-fulfilled packet as a failure,
    /// exiting non-zero.
    ///
    /// Without this a REJECT is reported and the process exits 0, which is
    /// right for an operator probing what a route does. It is wrong for a
    /// gate: `local/`'s topologies assert that a paid packet is *delivered*,
    /// and a run that prints "REJECT F02" and exits 0 is the same
    /// green-tick-over-nothing failure ADR 0007 bans a chain-less test for.
    pub expect_fulfill: bool,
}

/// What came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A FULFILL whose fulfilment matched the one this sender's own wrap
    /// derives -- so the packet genuinely reached a connector holding the
    /// shared secret, not merely *a* connector willing to answer.
    Fulfilled {
        /// The sealed answer, opened.
        status: u16,
        body: Vec<u8>,
    },
    /// A FULFILL whose fulfilment did NOT match. Reported rather than
    /// panicked on: it is a real wire condition and the operator needs to
    /// see it named, not as a decode failure further down.
    FulfilledWithWrongFulfillment,
    /// A REJECT.
    Rejected { code: String, message: String },
    /// `--dry-run`: nothing was sent.
    NotSent,
}

/// The result of a [`send`], in the shape the one summary line is built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendOutcome {
    pub destination: String,
    pub amount: u64,
    /// The `keyid` the request was signed under -- the exact value that has
    /// to appear in the target node's `[operator] write_keys`. Printed
    /// because "403 from the operator surface" and "this key is not
    /// allowlisted" are the same event, and only one of them is actionable.
    pub keyid: String,
    pub outcome: Outcome,
}

#[derive(Debug)]
pub enum SendError {
    /// The operator key file is missing, unreadable, or not a 32-byte key.
    KeyFile { path: String, reason: String },
    /// The terminating connector's identity could not be read.
    Identity { url: String, reason: String },
    /// The payload could not be sealed to that identity.
    Seal(String),
    /// The operator surface could not be reached, or answered non-2xx.
    Transport { url: String, reason: String },
    /// The answer decoded as neither FULFILL nor REJECT.
    Undecodable(String),
    /// `--expect-fulfill` was set and the packet was not fulfilled.
    NotFulfilled(String),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::KeyFile { path, reason } => write!(
                f,
                "operator key file '{path}': {reason}. It must hold 32 raw bytes or 64 hex \
                 characters -- the same shape as every other key file this binary reads. Its \
                 PUBLIC half must be listed in the target node's [operator] write_keys."
            ),
            SendError::Identity { url, reason } => write!(
                f,
                "could not read the terminating connector's identity from {url}: {reason}. A \
                 payload is sealed to the connector that terminates it (ADR 0018), so --seal-to \
                 must name that node's self-description URL -- the one whose GET answers with \
                 its identity, e.g. http://host:3000/ilp (ADR 0050) -- not necessarily the node \
                 given to --operator."
            ),
            SendError::Seal(reason) => write!(f, "could not seal the payload: {reason}"),
            SendError::Transport { url, reason } => {
                write!(f, "POST {url}: {reason}")
            }
            SendError::Undecodable(reason) => write!(
                f,
                "the operator surface answered with neither a FULFILL nor a REJECT: {reason}"
            ),
            SendError::NotFulfilled(detail) => write!(
                f,
                "--expect-fulfill was set and the packet was not fulfilled: {detail}"
            ),
        }
    }
}

impl std::error::Error for SendError {}

/// Read a 32-byte secret key from `path`, accepting either 32 raw bytes or
/// 64 hex characters. Mirrors `connector_config`'s own key-file reading so an
/// operator does not have to remember two conventions.
fn read_key_file(path: &str) -> Result<ed25519_dalek::Keypair, SendError> {
    let raw = std::fs::read(Path::new(path)).map_err(|error| SendError::KeyFile {
        path: path.to_string(),
        reason: error.to_string(),
    })?;
    let bytes = decode_key_bytes(&raw).ok_or_else(|| SendError::KeyFile {
        path: path.to_string(),
        reason: format!(
            "expected 32 raw bytes or 64 hex characters, found {}",
            raw.len()
        ),
    })?;
    let secret =
        ed25519_dalek::SecretKey::from_bytes(&bytes).map_err(|error| SendError::KeyFile {
            path: path.to_string(),
            reason: error.to_string(),
        })?;
    let public = ed25519_dalek::PublicKey::from(&secret);
    Ok(ed25519_dalek::Keypair { secret, public })
}

/// 32 raw bytes, or 64 hex characters possibly followed by whitespace a
/// `>` redirect or an editor left behind.
fn decode_key_bytes(raw: &[u8]) -> Option<[u8; 32]> {
    if raw.len() == 32 {
        return raw.try_into().ok();
    }
    let text = std::str::from_utf8(raw).ok()?.trim();
    if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// The client `url` is dialed on (ADR 0070 decision 5), or why there is
/// none.
///
/// Both of this verb's dials come through here, one at a time, which is what
/// makes them independently judged: the answer is a function of the URL in
/// hand and the one proxy value, never of what the other URL was.
///
/// [`is_onion_endpoint`] is called rather than re-derived. The suffix that
/// decides this dial, the suffix that decides a peering's carriage and the
/// suffix that decides the carriage's own proxy selection are one
/// implementation, so `connector send` cannot probe a node over a path the
/// node itself would not have used.
///
/// A URL that does not parse is dialed direct: it is `reqwest`'s to refuse
/// with the error it will give anyway, and inventing a second complaint about
/// it here would only add a shape the caller has to tell apart from a real
/// dial failure.
fn client_for(url: &str, socks_proxy: Option<&Url>) -> Result<reqwest::Client, String> {
    if !Url::parse(url).is_ok_and(|parsed| is_onion_endpoint(&parsed)) {
        return Ok(reqwest::Client::new());
    }
    let proxy = socks_proxy.ok_or_else(|| NO_SOCKS_PROXY.to_string())?;
    reqwest::Proxy::all(proxy.as_str())
        .and_then(|socks| reqwest::Client::builder().proxy(socks).build())
        .map_err(|error| format!("--socks-proxy '{proxy}' could not be used: {error}"))
}

/// The terminating connector's own identity, read from the running node the
/// way a real sender learns it -- never reconstructed from a key file, so
/// what gets sealed is genuinely what that process holds.
///
/// `connector_url` is that node's self-description URL (ADR 0050), e.g.
/// `http://host:3000/ilp` -- never an origin. The identity-only endpoint is
/// `/identity` beneath it, so the request made is
/// `http://host:3000/ilp/identity`.
///
/// `socks_proxy` is judged against the URL actually requested, and against
/// that URL alone: a `--seal-to` on an onion host is fetched through the
/// proxy whatever `--operator` was, and a clearnet one is fetched direct
/// whatever `--operator` was (ADR 0070 decision 5).
async fn fetch_identity(
    connector_url: &str,
    socks_proxy: Option<&Url>,
) -> Result<PublicKeyBytes, SendError> {
    let url = format!("{}/identity", connector_url.trim_end_matches('/'));
    let fail = |reason: String| SendError::Identity {
        url: url.clone(),
        reason,
    };
    let body: serde_json::Value = client_for(&url, socks_proxy)
        .map_err(fail)?
        .get(&url)
        .send()
        .await
        .map_err(|error| fail(error.to_string()))?
        .json()
        .await
        .map_err(|error| fail(error.to_string()))?;
    let hex = body["publicKey"]
        .as_str()
        .ok_or_else(|| fail("no `publicKey` in the answer".to_string()))?;
    let bytes = decode_hex(hex).ok_or_else(|| fail(format!("`publicKey` is not hex: {hex}")))?;
    bytes.as_slice().try_into().map_err(|_| {
        fail(format!(
            "expected a 65-byte public key, found {}",
            bytes.len()
        ))
    })
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    let text = text.strip_prefix("0x").unwrap_or(text);
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).ok())
        .collect()
}

/// The `keyid` a key file signs under -- the exact 64-hex value that has to
/// be on a node's `[operator] write_keys` for a write signed by it to be
/// accepted.
///
/// Deriving an ed25519 public key from a secret is not something a shell can
/// reasonably do, and "what do I put in write_keys for this key file?" is the
/// first question anyone provisioning an operator surface has. Answering it
/// from the binary that will do the signing means the answer cannot disagree
/// with the signature.
pub fn print_keyid(key_file: &str) -> Result<String, SendError> {
    Ok(keyid_hex(&read_key_file(key_file)?))
}

/// Form the packet, sign the write, and hand it to the operator surface.
pub async fn send(options: &SendOptions) -> Result<SendOutcome, SendError> {
    let keypair = read_key_file(&options.operator_key_file)?;
    let keyid = keyid_hex(&keypair);
    let identity = fetch_identity(&options.seal_to, options.socks_proxy.as_ref()).await?;

    let plaintext = EnvelopeRequest {
        method: options.method.clone(),
        target: options.target.clone(),
        headers: vec![("content-type".to_string(), "application/json".to_string())],
        body: options.body.clone(),
    }
    .encode();
    let (data, shared_secret) =
        seal_request(&plaintext, &identity).map_err(|error| SendError::Seal(error.to_string()))?;

    let prepare = Prepare {
        amount: options.amount,
        expires_at: Utc::now() + Duration::seconds(options.expires_in_seconds),
        greeting: false,
        destination: options.destination.clone(),
        data,
    };
    let body = prepare.encode();

    if options.dry_run {
        return Ok(SendOutcome {
            destination: options.destination.clone(),
            amount: options.amount,
            keyid,
            outcome: Outcome::NotSent,
        });
    }

    let base = options.operator_url.trim_end_matches('/');
    let url = format!("{base}{PACKETS_PATH}");
    let created = Utc::now().timestamp().max(0) as u64;
    let (signature_input, signature, content_digest) = sign_request(
        &keypair,
        "POST",
        PACKETS_PATH,
        &body,
        created,
        Some(created + SIGNATURE_TTL_SECONDS),
    );

    // The second of the two dials, judged on its own URL (ADR 0070 decision
    // 5). The identity fetch above is already done and its verdict has no
    // bearing here.
    let response = client_for(&url, options.socks_proxy.as_ref())
        .map_err(|reason| SendError::Transport {
            url: url.clone(),
            reason,
        })?
        .post(&url)
        .header("content-type", "application/octet-stream")
        .header("content-digest", content_digest)
        .header("signature-input", signature_input)
        .header("signature", signature)
        .body(body)
        .send()
        .await
        .map_err(|error| SendError::Transport {
            url: url.clone(),
            reason: error.to_string(),
        })?;

    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|error| SendError::Transport {
            url: url.clone(),
            reason: error.to_string(),
        })?;

    // A non-2xx here is the operator surface refusing the WRITE -- an
    // unallowlisted key, a stale signature, a replayed one -- and never a
    // packet-level answer. Naming the keyid is the whole point: 401/403 from
    // this endpoint is almost always "that key is not in write_keys".
    if !status.is_success() {
        return Err(SendError::Transport {
            url,
            reason: format!(
                "{status} -- {}. The write was refused before any packet was formed; check that \
                 keyid {keyid} is on the target node's [operator] write_keys.",
                String::from_utf8_lossy(&bytes).trim()
            ),
        });
    }

    let outcome = match Fulfill::decode(&bytes) {
        Ok(fulfill) => {
            if fulfill.fulfillment != derive_fulfillment(&shared_secret) {
                Outcome::FulfilledWithWrongFulfillment
            } else {
                let opened = open_response(&shared_secret, &fulfill.data)
                    .map_err(|error| SendError::Undecodable(error.to_string()))?;
                let envelope = EnvelopeResponse::decode(&opened)
                    .map_err(|error| SendError::Undecodable(error.to_string()))?;
                Outcome::Fulfilled {
                    status: envelope.status,
                    body: envelope.body,
                }
            }
        }
        Err(fulfill_error) => match Reject::decode(&bytes) {
            Ok(reject) => Outcome::Rejected {
                code: reject.code.as_str().to_string(),
                message: reject.message,
            },
            Err(reject_error) => {
                return Err(SendError::Undecodable(format!(
                    "not a FULFILL ({fulfill_error}) and not a REJECT ({reject_error})"
                )))
            }
        },
    };

    if options.expect_fulfill && !matches!(outcome, Outcome::Fulfilled { .. }) {
        return Err(SendError::NotFulfilled(match &outcome {
            Outcome::Rejected { code, message } => format!("REJECT {code} -- {message}"),
            Outcome::FulfilledWithWrongFulfillment => {
                "the fulfilment did not match the one this sender's own gift wrap derives, so \
                 whatever answered was not the node --seal-to names"
                    .to_string()
            }
            Outcome::NotSent => "--dry-run sends nothing, so nothing can be fulfilled".to_string(),
            Outcome::Fulfilled { .. } => unreachable!("guarded by the matches! above"),
        }));
    }

    Ok(SendOutcome {
        destination: options.destination.clone(),
        amount: options.amount,
        keyid,
        outcome,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::Write;

    use axum::routing::{get, post};
    use axum::{Json, Router};
    use connector_domain::RejectCode;
    use connector_runtime::Socks5TestServer;
    use connector_signer::{LocalSigner, Signer};

    use super::*;

    /// A real socket answering `GET /ilp/identity` the way a node's client
    /// edge does -- `fetch_identity` should ask this exact path when handed
    /// this node's self-description URL, `http://{addr}/ilp`.
    fn serve_identity(public_key_hex: String) -> String {
        format!("http://{}/ilp", serve_identity_at(public_key_hex))
    }

    /// The same listener, as the address it bound -- what a test needs when
    /// the URL the fetch is *given* is not the address the connection has to
    /// arrive at, which is the whole shape of a proxied dial.
    fn serve_identity_at(public_key_hex: String) -> std::net::SocketAddr {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let app = Router::new().route(
            "/ilp/identity",
            get(move || {
                let public_key_hex = public_key_hex.clone();
                async move {
                    Json(serde_json::json!({
                        "keyId": "test-key",
                        "publicKey": public_key_hex,
                    }))
                }
            }),
        );
        tokio::spawn(async move {
            let _ = axum::Server::from_tcp(listener)
                .expect("serve the bound listener")
                .serve(app.into_make_service())
                .await;
        });
        addr
    }

    /// `--seal-to` names a connector's self-description URL (ADR 0050),
    /// e.g. `http://host:3000/ilp` -- never an origin. `fetch_identity`
    /// must compose `/identity` beneath exactly that URL, landing on the
    /// same `/ilp/identity` route the client edge has always served.
    #[tokio::test]
    async fn fetch_identity_composes_identity_beneath_the_self_description_url() {
        let public_key_hex = "0x04".to_owned() + &"cd".repeat(64);
        let connector_url = serve_identity(public_key_hex.clone());

        let identity = fetch_identity(&connector_url, None)
            .await
            .expect("the served /ilp/identity must be readable from the /ilp URL");

        let expected = decode_hex(&public_key_hex).expect("test fixture is valid hex");
        assert_eq!(identity.as_slice(), expected.as_slice());
    }

    /// A trailing slash on the self-description URL must not produce
    /// `//identity` -- `fetch_identity` trims it before composing.
    #[tokio::test]
    async fn fetch_identity_tolerates_a_trailing_slash() {
        let public_key_hex = "0x04".to_owned() + &"ab".repeat(64);
        let connector_url = serve_identity(public_key_hex.clone());
        let with_slash = format!("{connector_url}/");

        let identity = fetch_identity(&with_slash, None)
            .await
            .expect("a trailing slash on --seal-to must still resolve");

        let expected = decode_hex(&public_key_hex).expect("test fixture is valid hex");
        assert_eq!(identity.as_slice(), expected.as_slice());
    }

    /// The error names the URL actually requested -- the composed
    /// `.../identity`, not merely the `--seal-to` value handed in -- so an
    /// operator can tell what was asked rather than guess at it.
    #[tokio::test]
    async fn identity_fetch_error_names_the_url_actually_requested() {
        // Nothing is listening on this loopback port, so the request fails
        // at connect and the error carries the URL fetch_identity composed.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        drop(listener);
        let connector_url = format!("http://{addr}/ilp");

        let error = fetch_identity(&connector_url, None)
            .await
            .expect_err("nothing is listening on this port");

        let SendError::Identity { ref url, .. } = error else {
            panic!("expected SendError::Identity, got {error:?}");
        };
        assert_eq!(
            url,
            &format!("{connector_url}/identity"),
            "the error must name the exact URL fetch_identity requested"
        );
        assert!(
            error.to_string().contains(url.as_str()),
            "the rendered message must include the URL actually requested: {error}"
        );
    }

    // ── Which socket each of the two dials left on (ADR 0070 decision 5) ──
    //
    // `connector send` dials twice, and the flag applies the same
    // host-selected rule to both, judged one at a time: the `--operator`
    // surface it signs a write to, and the `--seal-to` node whose
    // self-description it fetches. No parse and no `SendOptions` can answer
    // "where did that connection go", so the assertion is made against a
    // **real SOCKS5 server** (`connector_runtime::Socks5TestServer`) beside
    // the real HTTP listeners above -- and never against a call being
    // recorded (ADR 0007). Everything here is loopback: no onion daemon, no
    // third-party network, and no name that any resolver could resolve --
    // which is precisely why finding the onion name among the proxy's
    // CONNECT targets proves both that the dial traversed the proxy and that
    // it deferred resolution to it (`socks5h`).

    /// A v3 onion address's shape -- 56 base32 characters -- so the host
    /// under test is one an operator could have copied out of an `anon`
    /// daemon's `HiddenServiceDir/hostname` (ADR 0070 decision 7). No such
    /// service exists; nothing here expects one to.
    const ONION_HOST: &str = "toonexampleconnectoraddress234567abcdefghijklmnopqrstuvw.onion";

    /// The CONNECT target a dial to `http://<ONION_HOST>/...` must name: the
    /// **name**, and the default HTTP port, because nothing resolved it.
    fn onion_target() -> String {
        format!("{ONION_HOST}:80")
    }

    /// An onion node's self-description URL, as `--seal-to` takes it.
    fn onion_seal_to() -> String {
        format!("http://{ONION_HOST}/ilp")
    }

    /// An onion node's operator surface, as `--operator` takes it -- an
    /// origin, since `send` composes `/packets` beneath it.
    fn onion_operator() -> String {
        format!("http://{ONION_HOST}")
    }

    /// A **real** secp256k1 identity, hex as `GET /ilp/identity` publishes
    /// it. The tests that call [`send`] seal a payload to it, and a point
    /// that is not on the curve would fail at the seal rather than at the
    /// dial the test is about.
    fn a_real_identity() -> String {
        let identity = LocalSigner::generate("terminating-node")
            .public_key()
            .expect("a local signer has a public key");
        let mut hex = "0x".to_string();
        for byte in identity {
            hex.push_str(&format!("{byte:02x}"));
        }
        hex
    }

    /// An operator key file: 32 raw bytes, the same shape every other key
    /// file this binary reads. Held by the caller, since dropping it deletes
    /// it out from under the send.
    fn operator_key_file() -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().expect("temp key file");
        file.write_all(&[9u8; 32]).expect("write raw 32-byte key");
        file
    }

    /// A real socket answering `POST /packets` with a REJECT, the way an
    /// operator surface answers a packet it could not route.
    ///
    /// A REJECT rather than a status code on purpose: a `send` that got one
    /// completed the whole round trip -- request out, ILP answer back,
    /// decoded -- which is a stronger statement about the dial than a
    /// connection error would be.
    fn serve_operator_reject() -> std::net::SocketAddr {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let app = Router::new().route(
            "/packets",
            post(|| async {
                (
                    [("content-type", "application/octet-stream")],
                    Reject {
                        code: RejectCode::f02_unreachable(),
                        triggered_by: "g.test.operator".to_string(),
                        message: "the operator surface answered this dial".to_string(),
                        data: Vec::new(),
                        accumulated_cost: 0,
                    }
                    .encode(),
                )
            }),
        );
        tokio::spawn(async move {
            let _ = axum::Server::from_tcp(listener)
                .expect("serve the bound listener")
                .serve(app.into_make_service())
                .await;
        });
        addr
    }

    /// Everything a [`send`] needs, with the two URLs and the one proxy the
    /// test is actually about.
    fn send_options(
        operator_url: &str,
        seal_to: &str,
        key_file: &tempfile::NamedTempFile,
        socks_proxy: Option<Url>,
    ) -> SendOptions {
        SendOptions {
            operator_url: operator_url.to_string(),
            operator_key_file: key_file.path().display().to_string(),
            destination: "g.example.app".to_string(),
            amount: 1,
            seal_to: seal_to.to_string(),
            target: "/".to_string(),
            method: "POST".to_string(),
            body: Vec::new(),
            expires_in_seconds: 60,
            socks_proxy,
            dry_run: false,
            expect_fulfill: false,
        }
    }

    /// A `.onion` `--seal-to` is fetched **through** the proxy -- asserted by
    /// the dial arriving at the SOCKS5 server.
    ///
    /// The proxy routes nothing here, so the fetch fails: that is the point.
    /// What is under test is where the connection went, and a proxy that
    /// refuses the CONNECT still records the target it was asked for.
    #[tokio::test]
    async fn an_onion_seal_to_is_fetched_through_the_proxy() {
        let proxy = Socks5TestServer::spawn_recording_only().await;

        let failed = fetch_identity(&onion_seal_to(), Some(&proxy.proxy_url())).await;

        assert!(
            failed.is_err(),
            "the proxy routes nothing, so the fetch cannot succeed: {failed:?}"
        );
        assert_eq!(
            proxy.targets(),
            vec![onion_target()],
            "the fetch reached the proxy, and reached it as a name -- `socks5h` defers \
             resolution to the proxy because no resolver here can resolve a .onion"
        );
    }

    /// The **other spelling** of a hidden-service host takes the same path
    /// (issue #1284).
    ///
    /// `anon` renamed the TLD it publishes and routes between v0.4.9.7
    /// (`.onion`) and v0.4.10.2 (`.anyone`), and neither release resolves
    /// the other's. `connector send` is how an operator probes such a node,
    /// so a flag that reached one spelling and not the other would send the
    /// probe direct, to a name no resolver here can answer for.
    #[tokio::test]
    async fn an_anyone_seal_to_is_fetched_through_the_proxy_too() {
        const ANYONE_HOST: &str = "toonexampleconnectoraddress234567abcdefghijklmnopqrstuvw.anyone";
        let proxy = Socks5TestServer::spawn_recording_only().await;

        let failed = fetch_identity(
            &format!("http://{ANYONE_HOST}/ilp"),
            Some(&proxy.proxy_url()),
        )
        .await;

        assert!(
            failed.is_err(),
            "the proxy routes nothing, so the fetch cannot succeed: {failed:?}"
        );
        assert_eq!(
            proxy.targets(),
            vec![format!("{ANYONE_HOST}:80")],
            "a `.anyone` host is selected by the same rule `.onion` is: one function \
             (`is_onion_endpoint`) asked, never a second list of suffixes"
        );
    }

    /// A clearnet `--seal-to` is fetched **direct**, even on an invocation
    /// that named a proxy.
    ///
    /// The flag is host-selected, not a mode: passing it must not reroute a
    /// URL that has somewhere real to go.
    #[tokio::test]
    async fn a_clearnet_seal_to_is_fetched_direct_even_with_a_proxy_named() {
        let public_key_hex = a_real_identity();
        let connector_url = serve_identity(public_key_hex.clone());
        let proxy = Socks5TestServer::spawn_recording_only().await;

        let identity = fetch_identity(&connector_url, Some(&proxy.proxy_url()))
            .await
            .expect("a clearnet self-description is reachable without the proxy");

        let expected = decode_hex(&public_key_hex).expect("the identity is hex");
        assert_eq!(identity.as_slice(), expected.as_slice());
        assert!(
            proxy.targets().is_empty(),
            "a clearnet URL was never offered to the proxy: {:?}",
            proxy.targets()
        );
    }

    /// The proxy really carries the identity fetch, end to end.
    ///
    /// The onion name is routed through the SOCKS5 server to a real
    /// self-description listener, which is what an onion daemon does with a
    /// circuit. The identity comes back from a host nothing on this machine
    /// resolved.
    #[tokio::test]
    async fn the_proxy_carries_the_identity_fetch_to_an_onion_seal_to() {
        let public_key_hex = a_real_identity();
        let upstream = serve_identity_at(public_key_hex.clone());
        let proxy = Socks5TestServer::spawn(HashMap::from([(onion_target(), upstream)])).await;

        let identity = fetch_identity(&onion_seal_to(), Some(&proxy.proxy_url()))
            .await
            .expect("the proxy routes this name to a listener that is really there");

        let expected = decode_hex(&public_key_hex).expect("the identity is hex");
        assert_eq!(identity.as_slice(), expected.as_slice());
        assert!(proxy.saw_host_ending_in(".onion"));
    }

    /// With no `--socks-proxy` an onion fetch is refused **before any
    /// lookup**, and says why.
    ///
    /// Without this the same failure would arrive by way of a local DNS
    /// lookup for a `.onion` name: a resolver error the operator cannot act
    /// on, about a host they can see is spelled correctly, from a lookup this
    /// verb should never have made. The reason names the flag, because that
    /// is the one thing they have to change.
    #[tokio::test]
    async fn with_no_proxy_an_onion_seal_to_is_refused_before_any_lookup() {
        let refused = fetch_identity(&onion_seal_to(), None)
            .await
            .expect_err("no proxy, no way to reach a .onion");

        let SendError::Identity { ref reason, .. } = refused else {
            panic!("expected SendError::Identity, got {refused:?}");
        };
        assert!(
            reason.contains("--socks-proxy"),
            "the refusal names the flag that would fix it: {reason}"
        );
        assert!(
            !reason.contains("dns"),
            "and it is this verb's own refusal, not a resolver's answer about a name no resolver \
             should have been asked for: {reason}"
        );
    }

    /// **One onion and one clearnet in the same invocation, each taking the
    /// right path**: a `.onion` `--operator` goes through the proxy while the
    /// clearnet `--seal-to` beside it does not.
    ///
    /// This is the independence claim in the direction that matters most --
    /// the operator surface is where a *signed write* is sent -- and it is
    /// the only test here that exercises the second dial at all, since
    /// nothing but [`send`] makes it.
    #[tokio::test]
    async fn an_onion_operator_traverses_the_proxy_while_the_clearnet_seal_to_does_not() {
        let seal_to = serve_identity(a_real_identity());
        let proxy = Socks5TestServer::spawn_recording_only().await;
        let key_file = operator_key_file();

        let failed = send(&send_options(
            &onion_operator(),
            &seal_to,
            &key_file,
            Some(proxy.proxy_url()),
        ))
        .await;

        let Err(SendError::Transport { .. }) = failed else {
            panic!("the proxy routes nothing, so the operator dial cannot succeed: {failed:?}");
        };
        assert_eq!(
            proxy.targets(),
            vec![onion_target()],
            "the operator dial reached the proxy as a name, and the clearnet --seal-to fetch \
             that preceded it never did"
        );
    }

    /// The same invocation the other way round: an onion `--seal-to` is
    /// fetched through the proxy, and the clearnet `--operator` beside it is
    /// dialed **direct** -- and really answered.
    ///
    /// Both halves are asserted because a test in which neither dial reached
    /// anything could be green for the wrong reason. The REJECT is the
    /// clearnet listener's own answer, which nothing but a completed direct
    /// round trip could have produced.
    #[tokio::test]
    async fn a_clearnet_operator_is_dialed_direct_while_the_onion_seal_to_traverses_the_proxy() {
        let upstream = serve_identity_at(a_real_identity());
        let proxy = Socks5TestServer::spawn(HashMap::from([(onion_target(), upstream)])).await;
        let operator = serve_operator_reject();
        let key_file = operator_key_file();

        let outcome = send(&send_options(
            &format!("http://{operator}"),
            &onion_seal_to(),
            &key_file,
            Some(proxy.proxy_url()),
        ))
        .await
        .expect("the seal-to resolves through the proxy and the operator answers direct");

        let Outcome::Rejected { ref code, .. } = outcome.outcome else {
            panic!("the clearnet operator surface answered a REJECT: {outcome:?}");
        };
        assert_eq!(code, "F02");
        assert_eq!(
            proxy.targets(),
            vec![onion_target()],
            "only the onion --seal-to traversed the proxy"
        );
        assert!(
            !proxy
                .targets()
                .iter()
                .any(|target| target.contains(&operator.to_string())),
            "the clearnet operator surface at {operator} must never appear as a CONNECT target"
        );
    }
}
