//! What a terminating connector tells the app about the payment that
//! brought a packet to it (ADR 0040, issue #994): request headers on the
//! delivery -- three from ADR 0040 and `X-TOON-Network` from ADR 0086 -- and only when this connector itself verified the payment
//! they describe.
//!
//! The TypeScript prototype sent the same three header names from sources
//! that could not carry their meaning -- `X-TOON-Payer` was the immediate
//! previous hop, `X-TOON-Chain` was the second label of the destination
//! address, i.e. chosen by whoever addressed the packet. ADR 0017 found
//! both wrong by construction and [ADR
//! 0036](../../../docs/adr/0036-a-paid-deliverys-attribution-stays-on-the-connector.md)
//! concluded from that they had no successor at all. ADR 0040 supersedes
//! that conclusion, not its reasoning: the names come back, bound to a
//! source that cannot name the wrong party.
//!
//! **The source is the admitted claim, and nothing else.** The only value
//! this module will ever put on the wire is the chain-namespaced client
//! channel key a covering claim was accepted under at this connector's own
//! client edge -- the key whose signature was checked against the
//! counterparty this node records for that channel (client-edge-spec.md
//! §1.3), the same key ADR 0036 put on the `"packet"` span and the
//! client-edge claim journal writes its `InboundClaimAccepted` entries
//! under. There is no other input:
//!
//! - **A longer path emits nothing.** A peer-role arrival, a forwarded
//!   packet, an unclaimed request -- none of them admitted a client claim
//!   here, so none of them carry a payer, and the header is absent rather
//!   than guessed. The one exception is a peer arrival at a **payment
//!   route** (ADR 0083), whose voucher this connector accepted and whose
//!   advance it checked against the packet's own `amount`: it states that
//!   `amount`, and names the last hop's peering channel -- never the
//!   person who paid. ADR 0017's "on any path longer than one hop the header
//!   names the wrong party" is therefore not merely avoided by convention;
//!   there is no code path that can produce it.
//! - **A free route emits nothing.** Attribution describes a payment; a
//!   route priced at zero took none.
//! - **The caller's own headers never survive.** Whatever the sender put
//!   inside the sealed envelope under one of these three names is removed
//!   before anything is injected -- on *every* delivery, including the ones
//!   that then inject nothing. An app reading `X-TOON-Payer` is reading
//!   this connector or reading nothing.

use connector_domain::EnvelopeRequest;

/// The client channel a covering claim was accepted on -- "who paid",
/// as the only form of it this connector can honestly assert.
pub const PAYER_HEADER: &str = "X-TOON-Payer";

/// What that claim had to advance by for this delivery: what this connector
/// charged for *this packet*, in the settlement asset's base units.
///
/// That is the route's price schedule evaluated at this packet's own payload
/// length (ADR 0065) -- which for a flat route is the flat price ADR 0020
/// fixed and ADR 0040 named here, unchanged. Never the arriving packet's
/// `amount`, which is what the sender chose to carry rather than what this
/// connector took.
pub const AMOUNT_HEADER: &str = "X-TOON-Amount";

/// The settlement chain the paying channel lives on -- read off the
/// channel key's own namespace, never off the destination address.
pub const CHAIN_HEADER: &str = "X-TOON-Chain";

/// The CAIP-2 network the paying channel was verified on (ADR 0086 decision
/// 14, issue #1600): `eip155:<chainId>` or `solana:<genesis prefix>`. What
/// tells an app on a node settling on several EVM networks whose base units
/// [`AMOUNT_HEADER`] is in, which [`CHAIN_HEADER`]'s two values cannot.
/// Read off the network this node verified the channel's claim on, never
/// off anything the sender wrote.
pub const NETWORK_HEADER: &str = "X-TOON-Network";

/// Every header name this module owns. One list, used both to strip a
/// caller's spelling of these names and to inject this connector's own, so
/// the two can never drift apart: a name that can be injected is a name
/// that is always stripped first.
const ATTRIBUTION_HEADERS: [&str; 4] = [PAYER_HEADER, AMOUNT_HEADER, CHAIN_HEADER, NETWORK_HEADER];

/// A payment this connector verified for itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PaymentAttribution<'a> {
    /// The chain-namespaced client channel key the covering claim was
    /// admitted under (`evm:0x<64 hex>` or `solana:<base58>`).
    pub channel_key: &'a str,
    /// What this connector charged for this packet -- the route's schedule
    /// at this packet's own payload length -- which is exactly what that
    /// claim had to cover (ADR 0020, ADR 0028, ADR 0065). At a payment
    /// route reached from a peer, the packet's own `amount`, which the
    /// voucher's advance had to cover instead (ADR 0083).
    pub charge: u64,
    /// The CAIP-2 network this node verified that channel's claim on, or
    /// `None` when it holds no network for the channel's chain -- in which
    /// case [`NETWORK_HEADER`] is omitted rather than guessed.
    pub network: Option<&'a str>,
}

/// The settlement chain a channel key names: the namespace ahead of its
/// `:` (`evm`, `solana`). A key with no namespace names no chain, and the
/// chain header is then simply omitted rather than filled with the key
/// itself -- an unrecognisable value is worse than an absent one.
fn chain_of(channel_key: &str) -> Option<&str> {
    match channel_key.split_once(':') {
        Some((chain, _)) if !chain.is_empty() => Some(chain),
        _ => None,
    }
}

/// Strip any inbound spelling of the attribution headers from `request`,
/// then -- if `attribution` describes a payment this connector verified --
/// state it.
///
/// The strip is unconditional: it runs for a free route and a peer-role
/// arrival exactly as it runs for a paid client-edge delivery, so a sender
/// cannot smuggle an attribution claim into an app by addressing a
/// destination that has none to state.
pub(crate) fn apply_payment_attribution(
    request: &mut EnvelopeRequest,
    attribution: Option<PaymentAttribution<'_>>,
) {
    request.headers.retain(|(name, _)| {
        !ATTRIBUTION_HEADERS
            .iter()
            .any(|owned| owned.eq_ignore_ascii_case(name))
    });

    let Some(attribution) = attribution else {
        return;
    };
    // Charging nothing for this delivery means there is no payment to
    // attribute, even when a claim happened to ride along. A route with a
    // slope and no base charges nothing only for an empty payload, which is
    // not a shape a real sealed envelope has.
    if attribution.charge == 0 {
        return;
    }

    request.headers.push((
        PAYER_HEADER.to_string(),
        attribution.channel_key.to_string(),
    ));
    request
        .headers
        .push((AMOUNT_HEADER.to_string(), attribution.charge.to_string()));
    if let Some(chain) = chain_of(attribution.channel_key) {
        request
            .headers
            .push((CHAIN_HEADER.to_string(), chain.to_string()));
    }
    if let Some(network) = attribution.network {
        request
            .headers
            .push((NETWORK_HEADER.to_string(), network.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_with(headers: Vec<(String, String)>) -> EnvelopeRequest {
        EnvelopeRequest {
            method: "POST".to_string(),
            target: "/".to_string(),
            headers,
            body: b"body".to_vec(),
        }
    }

    fn header<'a>(request: &'a EnvelopeRequest, name: &str) -> Option<&'a str> {
        request
            .headers
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn a_verified_payment_states_payer_amount_and_chain() {
        let mut request = request_with(vec![]);
        apply_payment_attribution(
            &mut request,
            Some(PaymentAttribution {
                channel_key: "evm:0xabc",
                charge: 1000,
                network: Some("eip155:8453"),
            }),
        );

        assert_eq!(header(&request, NETWORK_HEADER), Some("eip155:8453"));
        assert_eq!(header(&request, PAYER_HEADER), Some("evm:0xabc"));
        assert_eq!(header(&request, AMOUNT_HEADER), Some("1000"));
        assert_eq!(header(&request, CHAIN_HEADER), Some("evm"));
    }

    #[test]
    fn a_solana_channel_names_its_own_chain() {
        let mut request = request_with(vec![]);
        apply_payment_attribution(
            &mut request,
            Some(PaymentAttribution {
                channel_key: "solana:9xQeWv",
                charge: 7,
                network: None,
            }),
        );

        assert_eq!(header(&request, PAYER_HEADER), Some("solana:9xQeWv"));
        assert_eq!(header(&request, CHAIN_HEADER), Some("solana"));
        // No network held for the chain: omitted, never guessed.
        assert_eq!(header(&request, NETWORK_HEADER), None);
    }

    #[test]
    fn no_verified_payment_states_nothing() {
        let mut request = request_with(vec![]);
        apply_payment_attribution(&mut request, None);
        assert!(request.headers.is_empty());
    }

    /// A claim can ride along with a request to a route that charges
    /// nothing; there is still no payment to attribute.
    #[test]
    fn a_zero_price_states_nothing() {
        let mut request = request_with(vec![]);
        apply_payment_attribution(
            &mut request,
            Some(PaymentAttribution {
                channel_key: "evm:0xabc",
                charge: 0,
                network: Some("eip155:8453"),
            }),
        );
        assert!(request.headers.is_empty());
    }

    /// The spoof defence, in both directions: a sender's own spelling of
    /// these names is removed whether or not this connector then has
    /// anything of its own to say, and in whatever case it was written.
    #[test]
    fn a_senders_own_attribution_headers_never_survive() {
        let spoofed = vec![
            ("x-toon-payer".to_string(), "evm:0xvictim".to_string()),
            ("X-TOON-AMOUNT".to_string(), "1".to_string()),
            ("X-Toon-Chain".to_string(), "solana".to_string()),
            ("x-toon-network".to_string(), "eip155:1".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];

        let mut overwritten = request_with(spoofed.clone());
        apply_payment_attribution(
            &mut overwritten,
            Some(PaymentAttribution {
                channel_key: "evm:0xreal",
                charge: 1000,
                network: Some("eip155:8453"),
            }),
        );
        assert_eq!(header(&overwritten, PAYER_HEADER), Some("evm:0xreal"));
        assert_eq!(header(&overwritten, AMOUNT_HEADER), Some("1000"));
        assert_eq!(header(&overwritten, CHAIN_HEADER), Some("evm"));
        assert_eq!(header(&overwritten, NETWORK_HEADER), Some("eip155:8453"));
        assert_eq!(
            overwritten
                .headers
                .iter()
                .filter(|(name, _)| name.eq_ignore_ascii_case(NETWORK_HEADER))
                .count(),
            1
        );
        // Exactly one of each -- a stripped name cannot also arrive twice.
        assert_eq!(
            overwritten
                .headers
                .iter()
                .filter(|(name, _)| name.eq_ignore_ascii_case(PAYER_HEADER))
                .count(),
            1
        );

        let mut stripped = request_with(spoofed);
        apply_payment_attribution(&mut stripped, None);
        assert_eq!(header(&stripped, PAYER_HEADER), None);
        assert_eq!(header(&stripped, AMOUNT_HEADER), None);
        assert_eq!(header(&stripped, CHAIN_HEADER), None);
        assert_eq!(header(&stripped, NETWORK_HEADER), None);
        // Everything that is not ours is untouched.
        assert_eq!(header(&stripped, "content-type"), Some("application/json"));
    }
}
