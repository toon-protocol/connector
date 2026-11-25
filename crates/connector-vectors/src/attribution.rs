//! Schema 15, issue #1708 (ADR 0040, ADR 0080 decision 6, ADR 0086): the
//! `X-TOON-*` headers an app receives, pinned.
//!
//! Every case runs the connector's own
//! [`connector_runtime::apply_payment_attribution`] over a fixture envelope
//! request and the payment (or none) the connector verified for it, and
//! records the headers going in and the headers the app receives. Nothing
//! here is written by hand: a change to what the connector emits moves this
//! group, and `vectors_up_to_date` fails until the file is regenerated.
//!
//! A replaying SDK or app reads the contract off the cases: `received` is
//! `request`'s headers with every `X-TOON-*` of the sender's removed, then
//! the connector's own appended -- in the order `payer`, `amount`, `chain`,
//! `network`, each only when it is known.

use connector_domain::EnvelopeRequest;
use connector_runtime::{apply_payment_attribution, PaymentAttribution};
use serde::Serialize;

/// The payment a connector verified for a delivery, as the case states it.
#[derive(Debug, Clone, Serialize)]
pub struct VerifiedPayment {
    /// The claim key the covering claim was admitted under.
    pub claim_key: String,
    /// What the connector charged for the packet, base units, as a decimal
    /// string like every amount in this file.
    pub charge: String,
    /// The CAIP-2 network this node verified the channel on, or `null` when
    /// it holds none for the channel's chain.
    pub network: Option<String>,
    /// The Mina network of the zk edge's vault, when this node is one.
    pub zk_note_network: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AttributionCase {
    pub name: String,
    pub description: String,
    /// The envelope request's own headers, as the sender sealed them.
    pub request_headers: Vec<(String, String)>,
    /// What the connector verified, or `null`: a forwarded, unclaimed or
    /// free-route delivery has no payment to state.
    pub payment: Option<VerifiedPayment>,
    /// The headers the app receives.
    pub received_headers: Vec<(String, String)>,
}

#[derive(Debug, Serialize)]
pub struct AttributionVectors {
    pub cases: Vec<AttributionCase>,
}

const EVM_KEY: &str = "evm:0x1111111111111111111111111111111111111111111111111111111111111111";
const SOLANA_KEY: &str = "solana:Fg6PaFpoGXkYsidMpWTK6W2BeZ7FEfcYkg476zPFsLnS";
const XRPL_KEY: &str = "xrpl:2222222222222222222222222222222222222222222222222222222222222222";
const ALLOWANCE_KEY: &str = "zk-note:00112233445566778899aabbccddeeff";
const PEERING_KEY: &str = "evm:0x3333333333333333333333333333333333333333333333333333333333333333";

fn payment(
    claim_key: &str,
    charge: u64,
    network: Option<&str>,
    zk_note_network: Option<&str>,
) -> VerifiedPayment {
    VerifiedPayment {
        claim_key: claim_key.to_string(),
        charge: charge.to_string(),
        network: network.map(str::to_string),
        zk_note_network: zk_note_network.map(str::to_string),
    }
}

fn plain_headers() -> Vec<(String, String)> {
    vec![("Content-Type".to_string(), "application/json".to_string())]
}

/// A sender's attempt to name its own payer, amount, chain and network, in
/// the spellings a case-insensitive header match must still catch.
fn spoofed_headers() -> Vec<(String, String)> {
    let mut headers = plain_headers();
    headers.extend([
        ("x-toon-payer".to_string(), "evm:0xvictim".to_string()),
        ("X-TOON-AMOUNT".to_string(), "1".to_string()),
        ("X-Toon-Chain".to_string(), "solana".to_string()),
        ("x-toon-network".to_string(), "eip155:1".to_string()),
    ]);
    headers
}

fn case(
    name: &str,
    description: &str,
    request_headers: Vec<(String, String)>,
    payment: Option<VerifiedPayment>,
) -> AttributionCase {
    let mut request = EnvelopeRequest {
        method: "POST".to_string(),
        target: "/".to_string(),
        headers: request_headers.clone(),
        body: b"body".to_vec(),
    };
    apply_payment_attribution(
        &mut request,
        payment.as_ref().map(|p| PaymentAttribution {
            claim_key: &p.claim_key,
            charge: p.charge.parse().expect("a fixture charge is a u64"),
            network: p.network.as_deref(),
            zk_note_network: p.zk_note_network.as_deref(),
        }),
    );
    AttributionCase {
        name: name.to_string(),
        description: description.to_string(),
        request_headers,
        payment,
        received_headers: request.headers,
    }
}

pub fn generate_attribution_vectors() -> AttributionVectors {
    let mut cases = vec![
        case(
            "evm_voucher",
            "An EVM voucher at the client edge: payer, amount, chain and network.",
            plain_headers(),
            Some(payment(EVM_KEY, 1000, Some("eip155:8453"), None)),
        ),
        case(
            "solana_voucher",
            "A Solana voucher at the client edge.",
            plain_headers(),
            Some(payment(
                SOLANA_KEY,
                7,
                Some("solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp"),
                None,
            )),
        ),
        case(
            "xrpl_voucher",
            "An XRPL voucher at the client edge.",
            plain_headers(),
            Some(payment(XRPL_KEY, 25, Some("xrpl:0"), None)),
        ),
        case(
            "allowance_claim",
            "An allowance claim: no payer, and the Mina network as both chain and network.",
            plain_headers(),
            Some(payment(ALLOWANCE_KEY, 250, None, Some("mina:devnet"))),
        ),
        case(
            "peer_arrival_at_a_payment_route",
            "A peer arrival at a payment route (ADR 0083): the packet's own amount, \
             and the last hop's peering channel as payer.",
            plain_headers(),
            Some(payment(PEERING_KEY, 40, Some("eip155:8453"), None)),
        ),
        case(
            "free_route",
            "A route priced at zero took no payment, even with a claim riding along.",
            plain_headers(),
            Some(payment(EVM_KEY, 0, Some("eip155:8453"), None)),
        ),
        case(
            "forwarded_or_unclaimed",
            "A forwarded or unclaimed packet admitted no claim here: no headers.",
            plain_headers(),
            None,
        ),
        case(
            "network_not_held",
            "A node that holds no network for the channel's chain omits X-TOON-Network.",
            plain_headers(),
            Some(payment(SOLANA_KEY, 7, None, None)),
        ),
    ];

    // The sender's own spelling of these names never survives, in any case
    // above: rerun each with spoofed headers added.
    let spoofed: Vec<AttributionCase> = cases
        .iter()
        .map(|c| {
            case(
                &format!("{}_with_spoofed_headers", c.name),
                &format!(
                    "{} The sender's own X-TOON-* headers are stripped.",
                    c.description
                ),
                spoofed_headers(),
                c.payment.clone(),
            )
        })
        .collect();
    cases.extend(spoofed);

    AttributionVectors { cases }
}
