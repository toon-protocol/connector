//! The x402 v2 `payment-required` greeting: **one** wire shape, written by
//! the client edge and read by whoever dials one.
//!
//! These types were the client edge's own private structs until issue #874.
//! They live here now for the reason ADR 0027 gives for the BTP frame
//! grammar living in one codec crate: the greeting is emitted by
//! `connector-client-edge` (`x402_terms_body`, served as an HTTP 402 body
//! and, on the BTP carriage, as `payment-required` protocolData on a REJECT)
//! and read by the peer carriages, which sit *below* the client edge in the
//! crate graph and so cannot import it. A reader that re-declared the shape
//! would be a second wire definition free to drift from the emitter, which
//! is exactly the fork `connector-btp`'s shared entry names exist to
//! prevent. `connector-domain` is the one crate both sides already depend
//! on, and a wire shape with no I/O in it is domain by ADR 0001's own test.
//!
//! [`parse_greeting`] is the reader. Its contract is the part worth being
//! careful about: a greeting that is **present but unreadable** must never
//! degrade into "no payment required" -- that would turn a refusal to pay
//! into a silent free ride. So absence is the caller's business (there is
//! simply no greeting entry), and everything else is either terms or a
//! typed [`GreetingError`].

use serde::{Deserialize, Serialize};

use crate::node::NodeFacts;
use crate::Price;

/// The x402 version this connector emits and reads.
pub const X402_VERSION: u32 = 2;

/// The x402 v2 payment-required greeting (client-edge-spec.md §1.4): the
/// terms of the one payment method this connector's client edge actually
/// understands -- a TOON payment channel claim, over this same `/ilp`
/// endpoint. `accepts` is a list (ADR 0022's fourth acceptance criterion)
/// so a later method can be offered alongside this one without changing
/// the answer's shape; only one entry exists today because on-chain
/// settlement addresses (the `exact` x402 scheme's `asset`/`payTo`) are not
/// yet configured anywhere in this connector (issue #526 is answering
/// terms, not adding that config).
///
/// # What is required on the way in
///
/// Deserialization is deliberately more forgiving than serialization is
/// exact. Every field this connector emits is always written, but only the
/// ones a payer must have to act -- the version, the resource, and an
/// offer's `amount`/`payTo` -- are required to read one back. The rest
/// default, so a greeting from an older or a differently-implemented edge
/// (the TypeScript fleet's, say) is read as terms rather than rejected as
/// garbage over a field a payer never consults. What a payer *cannot* do
/// without is checked in [`parse_greeting`], not silently defaulted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct X402PaymentRequired {
    #[serde(rename = "x402Version")]
    pub x402_version: u32,
    pub resource: X402Resource,
    /// What a client should send to use the addressed route (issue #1210):
    /// the matching `[[routes]] request` table, converted to JSON verbatim.
    /// Sits beside `resource` rather than inside `accepts[]` -- it describes
    /// the *resource*, not a payment option, and applies whichever payment
    /// method a payer ends up satisfying. Absent -- not `null` -- when the
    /// route configured none, so this greeting is byte-identical to what it
    /// was before this issue; a reader that predates the field ignores it.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub request: Option<serde_json::Value>,
    /// One `toon-channel` entry (always first, and the only entry before
    /// ADR 0074) plus one x402-valid `batch-settlement` entry per chain this
    /// node has opted into accepting a batch-settlement channel on (ADR 0074
    /// decision 8). [`X402AcceptOption`] is `#[serde(untagged)]` rather than
    /// this staying `Vec<X402PaymentOption>` so the two shapes can share one
    /// wire array without either describing the other's fields.
    pub accepts: Vec<X402AcceptOption>,
}

impl X402PaymentRequired {
    /// The `toon-channel` offer a payer would satisfy -- still the answer
    /// this method gives even when `accepts` also carries batch-settlement
    /// entries (ADR 0074 decision 8), because every other reader of this
    /// greeting (nonce/claim covering, EVM domain recovery, the schedule and
    /// `payTo` a `connector send` quotes) is about TOON's own claim scheme,
    /// never about a channel this connector never itself opens.
    pub fn offer(&self) -> Option<&X402PaymentOption> {
        self.accepts.iter().find_map(|option| match option {
            X402AcceptOption::Channel(channel) => Some(channel.as_ref()),
            X402AcceptOption::BatchSettlement(_) => None,
        })
    }

    /// [`Self::offer`]'s mutable twin, used only where a caller needs to
    /// adjust the `toon-channel` offer's own `extra` in place (this module's
    /// own tests, and a covering claim's domain-recovery tests elsewhere) --
    /// production code only ever builds a greeting fresh, through
    /// [`terms_body`].
    pub fn offer_mut(&mut self) -> Option<&mut X402PaymentOption> {
        self.accepts.iter_mut().find_map(|option| match option {
            X402AcceptOption::Channel(channel) => Some(channel.as_mut()),
            X402AcceptOption::BatchSettlement(_) => None,
        })
    }

    /// Every `batch-settlement` entry this greeting offers (ADR 0074
    /// decision 8), in the order the emitting node's own opt-ins were
    /// walked -- empty on a node that has opted into neither chain, which is
    /// every node before this record and every node after it that has not
    /// written a `batch_settlement` table.
    pub fn batch_settlement_offers(&self) -> impl Iterator<Item = &X402BatchSettlementOption> {
        self.accepts.iter().filter_map(|option| match option {
            X402AcceptOption::BatchSettlement(batch) => Some(batch),
            X402AcceptOption::Channel(_) => None,
        })
    }

    /// What the **greeted packet** costs, in the asset's base units. `None`
    /// when there is no offer or its `amount` is not a decimal uint64 -- a
    /// greeting [`parse_greeting`] accepted always answers `Some`.
    ///
    /// For a flat route this is the route's whole price and always was. For
    /// a route priced by size (ADR 0065) it is that schedule evaluated at the
    /// payload length of the request being answered, so it is what *this*
    /// request would have cost. To learn what a differently sized packet
    /// costs, read [`Self::schedule`] instead of re-greeting.
    pub fn price(&self) -> Option<u64> {
        self.offer()?.amount.parse().ok()
    }

    /// The addressed route's whole price schedule (ADR 0065): its base, and
    /// what each started kibibyte of payload adds. `None` when there is no
    /// offer or either figure is not a decimal uint64.
    ///
    /// A greeting from a node that predates schedules carries no
    /// `pricePerKib`, which reads back as a slope of zero -- a flat price,
    /// which is exactly what such a node charges.
    pub fn schedule(&self) -> Option<Price> {
        let extra = &self.offer()?.extra;
        let base = extra.price.parse().ok()?;
        let per_kib = match extra.price_per_kib.as_deref() {
            None => 0,
            Some(text) => text.parse().ok()?,
        };
        Some(Price::scheduled(base, per_kib))
    }

    /// Who the payment is addressed to (the `exact` scheme's `payTo`).
    pub fn pay_to(&self) -> Option<&str> {
        self.offer().map(|offer| offer.pay_to.as_str())
    }

    /// `"http"` or `"btp"` when this greeting answers a request that
    /// arrived over a transport the route does not accept (issue #701),
    /// naming the transport the route actually requires. `None` on an
    /// ordinary unpaid-request greeting.
    pub fn required_transport(&self) -> Option<&str> {
        self.offer()?.extra.required_transport.as_deref()
    }

    /// The EVM channel-opening facts a payer signs an EIP-712 claim under,
    /// taken from the legacy `extra.settlement` object when present and
    /// otherwise from the first EVM entry of the per-chain
    /// `extra.settlements` list (issue #632) -- the two carry the same
    /// facts on a node that has both, so either answers.
    ///
    /// This is the *receiver's* domain, and it is the only correct one: a
    /// claim signed under the payer's own idea of the `TokenNetwork`
    /// recovers to a different address and is refused.
    pub fn evm_settlement(&self) -> Option<&X402SettlementTerms> {
        let extra = &self.offer()?.extra;
        extra.settlement.as_ref().or_else(|| {
            extra.settlements.iter().find_map(|entry| match entry {
                X402ChainSettlementTerms::Evm(evm) => Some(evm),
                X402ChainSettlementTerms::Solana(_) => None,
            })
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct X402Resource {
    pub url: String,
}

/// One entry of [`X402PaymentRequired::accepts`]: either the `toon-channel`
/// scheme every node has always offered, or an x402-valid `batch-settlement`
/// entry (ADR 0074 decision 8). `#[serde(untagged)]`, tried in this
/// declaration order, so the two can share one wire array without either
/// naming which it is: [`X402BatchSettlementOption`] requires `asset`, which
/// a `toon-channel` entry never carries, so an object lacking it falls
/// through to [`X402PaymentOption`] -- the same structural-mismatch
/// disambiguation [`X402ChainSettlementTerms`] already uses.
/// `Channel` is boxed only to keep this enum's own size down to its
/// smaller variant's -- `X402PaymentOption` carries the whole
/// `X402ChannelExtra` bag, which nothing about a `batch-settlement` entry
/// needs. Serde boxes and unboxes it transparently, so the wire shape is
/// unaffected.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum X402AcceptOption {
    BatchSettlement(X402BatchSettlementOption),
    Channel(Box<X402PaymentOption>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct X402PaymentOption {
    #[serde(default)]
    pub scheme: String,
    #[serde(default)]
    pub network: String,
    /// The price, in base units, as a decimal string. Required: it is the
    /// term a payer has to satisfy.
    pub amount: String,
    /// Required, for the same reason `amount` is -- a payer cannot act on
    /// an offer that names no payee.
    #[serde(rename = "payTo")]
    pub pay_to: String,
    #[serde(rename = "maxTimeoutSeconds", default)]
    pub max_timeout_seconds: u64,
    #[serde(rename = "httpEndpoint", default)]
    pub http_endpoint: String,
    #[serde(default)]
    pub extra: X402ChannelExtra,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct X402ChannelExtra {
    #[serde(rename = "ilpAddress", default)]
    pub ilp_address: String,
    #[serde(default)]
    pub endpoint: String,
    /// The **base** of the addressed route's price schedule: what a packet
    /// of any size to this destination costs before its payload is counted
    /// (ADR 0065). Equal to `amount` above for a flat route, which is every
    /// route that existed before schedules did -- so a reader written
    /// against the flat greeting reads the same number it always did.
    #[serde(default)]
    pub price: String,
    /// The **slope** of that schedule: what each started kibibyte of payload
    /// adds (ADR 0065, issue #984). Absent -- not `"0"` -- on a flat route,
    /// so a flat greeting is byte-identical to what it was before schedules
    /// existed and a parser written before this field is unaffected.
    ///
    /// This field is what keeps ADR 0011's cacheability property true under
    /// a schedule. `amount` answers only for a packet the size of the one
    /// that was greeted; `price` and this together answer for **every**
    /// size, so one greeting still tells a sender what any packet it might
    /// send will cost, and it does not have to probe per size.
    #[serde(
        rename = "pricePerKib",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub price_per_kib: Option<String>,
    /// The emitting node's own ILP address(es) (issue #807) -- the
    /// authoritative list from `[announce]`, never an echo of the probed
    /// `destination` the way `ilp_address` above is. Present exactly when
    /// the emitter has a bootstrap identity configured; empty (and absent
    /// on the wire) otherwise, so a parser written before this field
    /// existed is unaffected.
    #[serde(
        rename = "ilpAddresses",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub ilp_addresses: Vec<String>,
    /// Where clients pay the emitting node over BTP (issue #807) -- the
    /// same fact a kind:10032 announce carries as `btpEndpoint`. Present
    /// exactly when a bootstrap identity is configured; `None` (and absent
    /// on the wire) otherwise, same treatment as `settlement`/`settlements`
    /// below.
    #[serde(
        rename = "btpEndpoint",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub btp_endpoint: Option<String>,
    /// The channel-opening facts (issue #617), present exactly when the
    /// emitting node has a settlement backend. `None` (and absent on the
    /// wire) on a settlement-less node -- the terms shape is otherwise
    /// unchanged, so a parser written before this field existed is
    /// unaffected.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub settlement: Option<X402SettlementTerms>,
    /// Every configured chain's channel-opening facts (issue #632), additive
    /// beside [`settlement`](Self::settlement): a node settling on N chains
    /// (epic #627) lists all N here, including the same EVM entry
    /// `settlement` already carries verbatim. Absent -- not an empty array
    /// -- on a node with no settlement backend at all, so the pre-#632
    /// shape (and the pre-#617 shape beneath it) stays byte-identical for a
    /// settlement-less node; a parser written before either field existed
    /// is unaffected either way.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub settlements: Vec<X402ChainSettlementTerms>,
    /// Present, and self-diagnosing, exactly when this greeting answers a
    /// request that arrived over a transport its route's policy does not
    /// accept (issue #701, toon-meta#262 decision 11): `"http"` or `"btp"`,
    /// naming the transport the route actually requires. Absent -- not
    /// `null` -- on every other greeting, so the pre-#701 shape is
    /// unchanged for a route with no transport restriction.
    #[serde(
        rename = "requiredTransport",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub required_transport: Option<String>,
    /// The session lease backstop TTL the emitting node's client session
    /// registry actually enforces (issue #722, toon-meta#262 decision 12's
    /// cross-plane invariant), in milliseconds -- always emitted, unlike
    /// `settlement`/`settlements`/`requiredTransport`, since every node has
    /// a session registry regardless of settlement backend. Always the same
    /// value `connector_client_edge::session_registry`'s
    /// `SESSION_LEASE_BACKSTOP_TTL` enforces, never a second literal typed
    /// nearby: a client (buzz#84's relay-side freshness window among them)
    /// reads this instead of hardcoding a guessed millisecond count.
    #[serde(rename = "sessionLeaseTtlMs", default)]
    pub session_lease_ttl_ms: u64,
}

/// What an unaffiliated buyer needs to OPEN a channel with the emitting
/// node, carried in the x402 greeting's `extra` (issue #617). This is ADR
/// 0022's "answers when asked" applied to channel establishment: the
/// TypeScript fleet distributes these same facts in a kind:10032 announce,
/// which this fleet will never make -- the greeting is the ask that
/// replaces it.
///
/// Every field is a fact the node already proved at startup:
/// `EvmSettlementBackend::connect` resolved `token_network` through the
/// registry and refused to boot on a `decimals` disagreement, so nothing
/// here can drift from the deployment without the node failing to start.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct X402SettlementTerms {
    /// `evm:<chainId>`, the chain the backend read at connect time.
    pub chain: String,
    /// The on-chain counterparty a buyer opens a channel WITH -- the
    /// settlement backend's own signing address.
    #[serde(rename = "settlementAddress")]
    pub settlement_address: String,
    /// The stable operator-facing factory address (issue #576).
    #[serde(rename = "tokenNetworkRegistry")]
    pub token_network_registry: String,
    /// The resolved `TokenNetwork` -- the EIP-712 `verifyingContract` a
    /// claim on any of its channels is signed under.
    #[serde(rename = "tokenNetwork")]
    pub token_network: String,
    #[serde(rename = "tokenAddress")]
    pub token_address: String,
    /// The token's own reported scale -- informational (claims are already
    /// in base units), verified against the chain at startup (issue #564).
    pub decimals: u8,
}

/// One configured chain's entry in the x402 greeting's `extra.settlements`
/// list (issue #632, epic #627's per-chain expansion of the single EVM
/// [`X402SettlementTerms`] issue #617 shipped). Untagged: serde tries each
/// variant in declaration order and keeps the first one whose required
/// fields all deserialize, so as long as every variant has at least one
/// field the others lack -- `tokenNetworkRegistry` for EVM, `programId` for
/// Solana -- that structural mismatch alone disambiguates them; no explicit
/// tag is needed on the wire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum X402ChainSettlementTerms {
    /// Exactly the same facts, in the same shape, the legacy `extra.settlement`
    /// object carries -- a two-chain node's `settlements` entry for its EVM
    /// leg is byte-identical to its legacy `settlement` object.
    Evm(X402SettlementTerms),
    /// See [`X402SolanaSettlementTerms`] for what each field means.
    Solana(X402SolanaSettlementTerms),
}

/// The Solana twin of [`X402SettlementTerms`] (issue #632): what an
/// unaffiliated buyer needs to open a channel against the emitting node's
/// deployed `payment-channel` program instance. Every field is a fact
/// `SolanaSettlementBackend::connect` already proved at startup (issue
/// #630) -- the program is reachable, executable and proven to behave like
/// the deployed payment-channel program, and the configured `decimals`
/// agrees with the mint's own.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct X402SolanaSettlementTerms {
    /// Always `"solana"` -- unlike EVM, a Solana backend has no chain id to
    /// append: the program id already names exactly one deployed instance.
    pub chain: String,
    /// The on-chain counterparty a buyer opens a channel WITH -- the
    /// settlement backend's own signing pubkey, base58-encoded.
    #[serde(rename = "settlementAddress")]
    pub settlement_address: String,
    /// The deployed `payment-channel` program instance, base58-encoded.
    #[serde(rename = "programId")]
    pub program_id: String,
    /// The SPL mint every channel this backend opens settles in,
    /// base58-encoded.
    #[serde(rename = "tokenAddress")]
    pub token_address: String,
    /// The mint's own reported scale -- informational (claims are already
    /// in base units), verified against the chain at startup (issue #630).
    pub decimals: u8,
}

/// One chain's x402 `batch-settlement` facts (ADR 0074 decision 8): present
/// only for a chain this node has opted into accepting a batch-settlement
/// channel on. The greeting's own `batch-settlement` `accepts[]` entry and
/// the self-description's `batchSettlements` list are both projections of
/// this one value (ND-11) -- `terms_body` and
/// [`crate::node::NodeSelfDescription::describe`] each read it off
/// [`crate::node::NodeFacts::batch_settlements`], and neither assembles a
/// second copy of it.
///
/// `#[serde(untagged)]`: [`X402BatchSettlementEvmTerms`] requires
/// `receiverAuthorizer`/`name`/`version`, which
/// [`X402BatchSettlementSolanaTerms`] never carries, so the two disambiguate
/// structurally, the same way [`X402ChainSettlementTerms`] does.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum X402BatchSettlementTerms {
    Evm(X402BatchSettlementEvmTerms),
    Solana(X402BatchSettlementSolanaTerms),
}

/// What a stock `@x402/evm` client needs to build a `ChannelConfig` and
/// deposit into a channel this connector will admit (ADR 0074 decisions 2
/// and 8; x402 `specs/schemes/batch-settlement/scheme_batch_settlement_evm.md`
/// at commit `0cb1a1f0`). `network`, `asset` and `payTo` are this fact's own
/// copies of the same-named top-level fields the greeting's
/// [`X402BatchSettlementOption`] carries; `receiver_authorizer` and
/// `min_withdraw_delay_secs` are `extra.receiverAuthorizer`/
/// `extra.withdrawDelay` there.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct X402BatchSettlementEvmTerms {
    /// `eip155:<chainId>` (CAIP-2).
    pub network: String,
    /// The ERC-20 this node accepts a deposit in -- `[settlement.evm]
    /// token_address`, `0x`-prefixed, never declared a second time (CF-26).
    pub asset: String,
    /// The receiver a payer's `ChannelConfig.receiver` must name -- this
    /// node's EVM settlement address.
    #[serde(rename = "payTo")]
    pub pay_to: String,
    /// `ChannelConfig.receiverAuthorizer`. Always the same address as
    /// `pay_to`: decision 5 never delegates it, so a facilitator can never
    /// refund a voucher this connector has not yet claimed.
    #[serde(rename = "receiverAuthorizer")]
    pub receiver_authorizer: String,
    /// The shortest `withdrawDelay` a channel may carry and still be
    /// admitted, in seconds -- `[settlement.evm.batch_settlement]
    /// min_withdraw_delay_secs`.
    #[serde(rename = "withdrawDelay")]
    pub min_withdraw_delay_secs: u64,
    /// The EIP-712 domain `name` of `asset` -- `"USDC"` for the devnet's
    /// Circle FiatToken v2.2. Configured (`asset_eip712_name`), not read off
    /// the chain: an arbitrary ERC-20 need not expose one, and this
    /// connector never itself signs or verifies under it (only a payer's
    /// deposit does), so there is nothing here to prove against a live
    /// contract the way `decimals` is (issue #1345).
    pub name: String,
    /// The EIP-712 domain `version` of `asset` -- `"2"` for the devnet's
    /// Circle FiatToken v2.2. Configured (`asset_eip712_version`), for the
    /// same reason `name` is.
    pub version: String,
}

/// The Solana twin of [`X402BatchSettlementEvmTerms`] (ADR 0074 decisions 2,
/// 5 and 8; x402 `specs/schemes/batch-settlement/scheme_batch_settlement_svm.md`
/// at commit `0cb1a1f0`). The SVM spec's wire vocabulary is network-neutral:
/// its own `extra.withdrawDelay` carries what this connector's config and
/// the `payment-channels` program itself both call `grace_period`, so
/// [`Self::min_grace_period_secs`] is exactly that value under the EVM
/// leg's own wire name -- there is no second key called `gracePeriod`
/// anywhere on the wire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct X402BatchSettlementSolanaTerms {
    /// `solana:<genesis-hash-first-32-base58-chars>` (CAIP-2) -- the chain
    /// this node's settlement RPC actually reached, read at connect time
    /// (`connector_settlement_solana::SolanaSettlementBackend::caip2_network`,
    /// issue #1131's own precedent), never guessed from the RPC URL.
    pub network: String,
    /// The SPL/Token-2022 mint this node accepts a deposit in, base58 --
    /// `[settlement.solana] token_address`, never declared a second time
    /// (CF-26).
    pub asset: String,
    /// The owner of this node's receiving token account -- the x402 SVM
    /// scheme's single 10000bps distribution recipient (decision 2), which
    /// is this node's Solana settlement pubkey.
    #[serde(rename = "payTo")]
    pub pay_to: String,
    /// The sponsor: `rent_payer` and the zero-share `payee` a payer's
    /// `open` transaction must name (decision 5) -- this node's Solana
    /// settlement pubkey, the same key as `pay_to`.
    #[serde(rename = "feePayer")]
    pub fee_payer: String,
    /// The shortest `grace_period` a channel may carry and still be
    /// admitted, in seconds -- `[settlement.solana.batch_settlement]
    /// min_grace_period_secs` -- carried on the wire as `withdrawDelay` (see
    /// this type's own doc).
    #[serde(rename = "withdrawDelay")]
    pub min_grace_period_secs: u64,
}

/// The greeting's own `batch-settlement` `accepts[]` entry (ADR 0074
/// decision 8): [`X402BatchSettlementTerms`] plus the fields that describe
/// *this* request rather than this node's standing terms -- `scheme`,
/// `amount` (the same charge the `toon-channel` entry quotes) and
/// `maxTimeoutSeconds`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct X402BatchSettlementOption {
    pub scheme: String,
    pub network: String,
    /// The addressed route's charge, in the asset's base units, as a decimal
    /// string -- identical to the `toon-channel` entry's own `amount`: both
    /// are alternative ways to pay the same charge.
    pub amount: String,
    pub asset: String,
    #[serde(rename = "payTo")]
    pub pay_to: String,
    #[serde(rename = "maxTimeoutSeconds")]
    pub max_timeout_seconds: u64,
    pub extra: X402BatchSettlementExtra,
}

/// [`X402BatchSettlementOption::extra`]: either chain's facts, spelled the
/// way the corresponding x402 scheme spec names them. Untagged for the same
/// structural reason [`X402BatchSettlementTerms`] is.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum X402BatchSettlementExtra {
    Evm(X402BatchSettlementEvmExtra),
    Solana(X402BatchSettlementSolanaExtra),
}

/// x402 EVM batch-settlement spec `#L81-L97`: `receiverAuthorizer` and
/// `withdrawDelay` are required so a client can build a `ChannelConfig`;
/// `name`/`version` are required so it can build the deposit's own
/// ERC-3009/permit2 signature under the asset's real EIP-712 domain.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct X402BatchSettlementEvmExtra {
    #[serde(rename = "receiverAuthorizer")]
    pub receiver_authorizer: String,
    #[serde(rename = "withdrawDelay")]
    pub withdraw_delay: u64,
    pub name: String,
    pub version: String,
}

/// x402 SVM batch-settlement spec `#L163-L178`: `feePayer` and
/// `withdrawDelay` (the program's `grace_period`) are required so a client
/// can build the channel account `open` names.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct X402BatchSettlementSolanaExtra {
    #[serde(rename = "feePayer")]
    pub fee_payer: String,
    #[serde(rename = "withdrawDelay")]
    pub withdraw_delay: u64,
}

/// Project [`X402BatchSettlementTerms`] into the greeting's own
/// `accepts[]` entry shape, quoting `amount` for the request being
/// answered. The one place that builds an [`X402BatchSettlementOption`], so
/// the self-description's facts and the greeting's own copy of them can
/// never drift into two shapes (ND-11).
fn batch_settlement_accept(
    fact: &X402BatchSettlementTerms,
    amount: String,
) -> X402BatchSettlementOption {
    match fact {
        X402BatchSettlementTerms::Evm(evm) => X402BatchSettlementOption {
            scheme: "batch-settlement".to_string(),
            network: evm.network.clone(),
            amount,
            asset: evm.asset.clone(),
            pay_to: evm.pay_to.clone(),
            max_timeout_seconds: X402_MAX_TIMEOUT_SECONDS,
            extra: X402BatchSettlementExtra::Evm(X402BatchSettlementEvmExtra {
                receiver_authorizer: evm.receiver_authorizer.clone(),
                withdraw_delay: evm.min_withdraw_delay_secs,
                name: evm.name.clone(),
                version: evm.version.clone(),
            }),
        },
        X402BatchSettlementTerms::Solana(solana) => X402BatchSettlementOption {
            scheme: "batch-settlement".to_string(),
            network: solana.network.clone(),
            amount,
            asset: solana.asset.clone(),
            pay_to: solana.pay_to.clone(),
            max_timeout_seconds: X402_MAX_TIMEOUT_SECONDS,
            extra: X402BatchSettlementExtra::Solana(X402BatchSettlementSolanaExtra {
                fee_payer: solana.fee_payer.clone(),
                withdraw_delay: solana.min_grace_period_secs,
            }),
        },
    }
}

/// A `payment-required` greeting that was **there and unreadable** (issue
/// #874).
///
/// Every variant means the same operationally -- this connector cannot
/// learn what it owes -- and none of them may ever be collapsed into "no
/// terms were offered". They are told apart because the reason is what a
/// human debugging a link needs: a truncated frame, a non-x402 body, and a
/// future x402 version are three different bugs with three different
/// fixes, and only one of them is the far side's.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GreetingError {
    /// The bytes are not JSON at all.
    #[error("the payment-required greeting is not JSON: {0}")]
    NotJson(String),
    /// JSON, but not the x402 v2 terms shape -- a required field is
    /// missing or has the wrong type.
    #[error("the payment-required greeting is not x402 terms: {0}")]
    NotTerms(String),
    /// A version this connector has no reader for. Deliberately not
    /// best-effort parsed: a v3 greeting may price things differently, and
    /// paying against a misread offer is worse than not paying.
    #[error("x402 version {0} is not understood (this connector reads v{X402_VERSION})")]
    UnsupportedVersion(u32),
    /// Well-formed, but it offers nothing -- `accepts` is empty. There is
    /// no payment that would satisfy it, which is not the same as there
    /// being no payment required.
    #[error("the payment-required greeting offers no payment method")]
    NoOffer,
    /// The offer's price is not a decimal uint64, so there is no amount to
    /// cover.
    #[error("the offered amount '{0}' is not a decimal uint64")]
    UnreadableAmount(String),
    /// The offer names no payee.
    #[error("the payment-required greeting names no payTo")]
    NoPayee,
}

/// The x402 greeting's own `maxTimeoutSeconds` -- one figure, shared by
/// every emitter (issue #880: the peer carriages are a second emitter as of
/// this issue, and must not mint a second constant to drift from this one).
const X402_MAX_TIMEOUT_SECONDS: u64 = 60;

/// Build and serialize a `payment-required` greeting (client-edge-spec.md
/// §1.4) -- **the** emitter, called by every carriage that answers an
/// unpaid or under-covering request with x402 terms rather than doing the
/// work: the client edge's HTTP carriage (a `402` body), its BTP carriage
/// (an `F06` REJECT's `payment-required` protocolData), and -- as of issue
/// #880 -- the peer carriages' own `F06` REJECT for a peer PREPARE whose
/// claim does not cover its route's price (`peer-carriage-spec.md` §3.1).
/// One construction, in the one crate every emitter and every reader
/// already depends on, so a change to the shape cannot happen in one
/// carriage and not the others -- the same reasoning this module's own doc
/// comment gives for [`parse_greeting`] living here rather than being
/// re-declared per reader.
///
/// Every emitter passes [`GreetingTerms`] rather than a row of positional
/// arguments, most of which are empty on a carriage carrying neither
/// identity nor settlement terms: named at the call site, two of them
/// cannot be transposed without anyone noticing.
pub fn terms_body(terms: &GreetingTerms<'_>) -> Vec<u8> {
    let GreetingTerms {
        destination,
        price,
        payload_len,
        node,
        required_transport,
        session_lease_ttl_ms,
        request,
    } = *terms;
    // ND-11: every node fact in `extra` is read off the SAME value the node
    // self-description is projected from. There is no second assembly of
    // these fields, so the greeting cannot fall behind the document -- which
    // is the whole of what "the greeting is a projection" buys, and the
    // structural end of the `requiredTransport` defect.
    let ilp_addresses: &[String] = node
        .map(|node| node.ilp_addresses.as_slice())
        .unwrap_or(&[]);
    let btp_endpoint: Option<&str> = node.and_then(|node| node.btp_endpoint.as_deref());
    let settlement: Option<&X402SettlementTerms> = node.and_then(NodeFacts::evm_settlement);
    let settlements: &[X402ChainSettlementTerms] =
        node.map(|node| node.settlements.as_slice()).unwrap_or(&[]);
    // ADR 0074 decision 8: one `batch-settlement` entry per chain this node
    // has opted into, read off the same `NodeFacts` the self-description
    // publishes (ND-11) -- never a second declaration of these facts.
    let batch_settlements: &[X402BatchSettlementTerms] = node
        .map(|node| node.batch_settlements.as_slice())
        .unwrap_or(&[]);
    let amount = price.charge(payload_len).to_string();
    let mut accepts = vec![X402AcceptOption::Channel(Box::new(X402PaymentOption {
        scheme: "toon-channel".to_string(),
        network: destination.to_string(),
        amount: amount.clone(),
        pay_to: destination.to_string(),
        max_timeout_seconds: X402_MAX_TIMEOUT_SECONDS,
        http_endpoint: "/ilp".to_string(),
        extra: X402ChannelExtra {
            ilp_address: destination.to_string(),
            endpoint: "/ilp".to_string(),
            price: price.base().to_string(),
            price_per_kib: (!price.is_flat()).then(|| price.per_kib().to_string()),
            ilp_addresses: ilp_addresses.to_vec(),
            btp_endpoint: btp_endpoint.map(str::to_string),
            settlement: settlement.cloned(),
            settlements: settlements.to_vec(),
            required_transport: required_transport.map(str::to_string),
            session_lease_ttl_ms,
        },
    }))];
    accepts.extend(batch_settlements.iter().map(|fact| {
        X402AcceptOption::BatchSettlement(batch_settlement_accept(fact, amount.clone()))
    }));
    let terms = X402PaymentRequired {
        x402_version: X402_VERSION,
        resource: X402Resource {
            url: destination.to_string(),
        },
        request: request.cloned(),
        accepts,
    };
    serde_json::to_vec(&terms).expect("x402 terms always serialize")
}

/// What [`terms_body`] needs to know to quote one offer.
///
/// Everything but `destination` and `price` has a meaningful empty value,
/// so a carriage that carries none of it writes
/// `GreetingTerms { destination, price, ..Default::default() }` and says so
/// by omission rather than by a row of `None`s.
#[derive(Debug, Clone, Copy, Default)]
pub struct GreetingTerms<'a> {
    /// Doubles as `resource.url`, the offer's `network`, `payTo` and
    /// `extra.ilpAddress` -- there is exactly one payment method and one
    /// party to pay, so all four name the same address.
    pub destination: &'a str,
    /// What that address charges: the whole schedule (ADR 0065), quoted as
    /// `extra.price` (its base) and `extra.pricePerKib` (its slope).
    pub price: Price,
    /// The payload length of the request being answered, in bytes -- what
    /// `amount` is quoted for.
    ///
    /// x402's `amount` is what *this* request costs, so it is the schedule
    /// evaluated here rather than the schedule's base. A carriage greeting a
    /// request it has a `Prepare` for passes `prepare.data.len()`; one
    /// greeting a request that never became a packet passes `0`, and gets
    /// the base -- the cheapest true answer, and the exact figure a flat
    /// route quotes either way.
    pub payload_len: usize,
    /// The emitting node's own facts -- its addresses, its BTP endpoint and
    /// the chains it settles on ([`crate::node::NodeFacts`], ADR 0050).
    ///
    /// **The same value the node self-description is projected from**, which
    /// is what ND-11 requires: the greeting is a projection of that
    /// document's source, never a second description assembled beside it.
    /// `None` for a carriage that describes no node at all -- the peer
    /// carriages, whose counterparty already knows this node and needs only
    /// the figure quoted.
    pub node: Option<&'a NodeFacts>,
    /// `Some("http" | "btp")` only when this same shape is reused to tell a
    /// client it used the wrong transport entirely (issue #701).
    ///
    /// Deliberately **not** the self-description's own `requiredTransport`,
    /// which is a standing fact about this node's routes. This one is
    /// self-diagnosing: present only on the greeting answering a request that
    /// arrived over the wrong carriage (ND-12 -- the greeting keeps its own
    /// job).
    pub required_transport: Option<&'a str>,
    /// The emitting node's client session lease backstop (issue #722); a
    /// carriage with no client session registry of its own (the peer
    /// carriages) leaves it `0`, which is otherwise never a real
    /// deployment's value.
    pub session_lease_ttl_ms: u64,
    /// What a client should send to use the addressed route (issue #1210):
    /// the matching `[[routes]] request` table, converted to JSON at config
    /// load and handed in by reference here so quoting one costs a clone
    /// only when there is something to clone. `None` when the route
    /// configured none, or for a carriage addressing no configured route at
    /// all.
    pub request: Option<&'a serde_json::Value>,
}

/// Read a `payment-required` greeting's terms.
///
/// The bytes are whatever carried the greeting -- an HTTP 402 body, or the
/// `payment-required` protocolData entry of a BTP REJECT; both carriages
/// carry the identical bytes by construction (`x402_terms_body` is shared),
/// which is why one reader serves both.
///
/// Everything this returns `Err` for is a greeting that *was present*. A
/// caller that found no greeting at all must not route through here: it has
/// an ordinary answer, not a malformed one. That distinction is the whole
/// point -- see [`GreetingError`].
pub fn parse_greeting(bytes: &[u8]) -> Result<X402PaymentRequired, GreetingError> {
    // Two steps rather than one `from_slice::<X402PaymentRequired>` so the
    // "not JSON" and "not terms" cases stay distinguishable: serde reports
    // both through one error type, and the operator reading the log needs
    // to know whether the far side sent rubbish or sent a shape.
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| GreetingError::NotJson(error.to_string()))?;
    let terms: X402PaymentRequired = serde_json::from_value(value)
        .map_err(|error| GreetingError::NotTerms(error.to_string()))?;

    if terms.x402_version != X402_VERSION {
        return Err(GreetingError::UnsupportedVersion(terms.x402_version));
    }
    let offer = terms.offer().ok_or(GreetingError::NoOffer)?;
    if offer.pay_to.is_empty() {
        return Err(GreetingError::NoPayee);
    }
    if offer.amount.parse::<u64>().is_err() {
        return Err(GreetingError::UnreadableAmount(offer.amount.clone()));
    }
    Ok(terms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn well_formed() -> String {
        serde_json::json!({
            "x402Version": 2,
            "resource": { "url": "g.toon.relay" },
            "accepts": [{
                "scheme": "toon-channel",
                "network": "g.toon.relay",
                "amount": "1000",
                "payTo": "g.toon.relay",
                "maxTimeoutSeconds": 60,
                "httpEndpoint": "/ilp",
                "extra": {
                    "ilpAddress": "g.toon.relay",
                    "endpoint": "/ilp",
                    "price": "1000",
                    "sessionLeaseTtlMs": 300000
                }
            }]
        })
        .to_string()
    }

    #[test]
    fn a_well_formed_greeting_yields_its_terms() {
        let terms = parse_greeting(well_formed().as_bytes()).expect("well-formed terms");
        assert_eq!(terms.price(), Some(1000));
        assert_eq!(terms.pay_to(), Some("g.toon.relay"));
        assert_eq!(terms.required_transport(), None);
        assert_eq!(terms.offer().unwrap().extra.session_lease_ttl_ms, 300_000);
    }

    /// ADR 0065: a flat route's greeting is byte-identical to what it was
    /// before schedules existed. This is the compatibility claim the record
    /// makes, and it is the one every existing reader depends on.
    #[test]
    fn a_flat_routes_greeting_carries_no_slope_at_all() {
        let body = terms_body(&GreetingTerms {
            destination: "g.toon.relay",
            price: Price::flat(1000),
            payload_len: 4096,
            ..Default::default()
        });
        let text = String::from_utf8(body.clone()).unwrap();
        assert!(
            !text.contains("pricePerKib"),
            "a flat greeting must not carry the field at all, got: {text}"
        );
        let terms = parse_greeting(&body).expect("well-formed");
        // The payload length changes nothing for a flat route.
        assert_eq!(terms.price(), Some(1000));
        assert_eq!(terms.schedule(), Some(Price::flat(1000)));
    }

    /// Issue #1210: a route's `request` table rides at the top level of the
    /// greeting, beside `resource` -- it describes the resource, not one of
    /// the payment options in `accepts[]`.
    #[test]
    fn a_routes_request_table_rides_beside_resource() {
        let request = serde_json::json!({"protocol": "nip90", "kinds": [5096, 5098]});
        let body = terms_body(&GreetingTerms {
            destination: "g.toon.gas",
            price: Price::flat(1000),
            payload_len: 0,
            request: Some(&request),
            ..Default::default()
        });
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["request"], request);
        assert!(
            value["accepts"][0].get("request").is_none(),
            "request describes the resource, not a payment option"
        );

        let terms = parse_greeting(&body).expect("well-formed");
        assert_eq!(terms.request, Some(request));
    }

    /// A route with no `request` table publishes a greeting with no
    /// `request` key at all -- byte-identical to what it was before this
    /// issue, and a reader written before the field existed is unaffected.
    #[test]
    fn a_route_with_no_request_table_greets_with_no_request_key() {
        let body = terms_body(&GreetingTerms {
            destination: "g.toon.relay",
            price: Price::flat(1000),
            payload_len: 0,
            ..Default::default()
        });
        let text = String::from_utf8(body).unwrap();
        assert!(
            !text.contains("\"request\""),
            "a route with no request table must not carry the key at all, got: {text}"
        );
    }

    /// A schedule route's greeting answers both questions: what THIS request
    /// costs (`amount`), and what any request would cost (the schedule).
    #[test]
    fn a_schedule_greeting_quotes_this_packet_and_publishes_the_rule() {
        let price = Price::scheduled(1000, 30);
        let body = terms_body(&GreetingTerms {
            destination: "g.toon.ario",
            price,
            payload_len: 100 * 1024,
            ..Default::default()
        });
        let terms = parse_greeting(&body).expect("well-formed");

        // `amount` is what the greeted request costs.
        assert_eq!(terms.price(), Some(4_000));
        // ...and the schedule rides beside it, so a reader can price a
        // packet it has not sent yet without greeting again. This is what
        // keeps ADR 0011's cacheability true under a slope.
        let schedule = terms
            .schedule()
            .expect("a schedule route publishes its schedule");
        assert_eq!(schedule, price);
        assert_eq!(schedule.charge(2 * 1024 * 1024), 62_440);
        assert_eq!(terms.offer().unwrap().extra.price, "1000");
        assert_eq!(
            terms.offer().unwrap().extra.price_per_kib.as_deref(),
            Some("30")
        );
    }

    /// A greeting from a node that predates schedules reads back as the flat
    /// price it is, rather than failing to parse.
    #[test]
    fn a_pre_schedule_greeting_reads_as_a_flat_schedule() {
        let terms = parse_greeting(well_formed().as_bytes()).expect("well-formed");
        assert_eq!(terms.schedule(), Some(Price::flat(1000)));
        assert!(terms.schedule().unwrap().is_flat());
    }

    #[test]
    fn bytes_that_are_not_json_are_their_own_error() {
        let error = parse_greeting(b"\xff\xfe not json").expect_err("unreadable");
        assert!(matches!(error, GreetingError::NotJson(_)), "{error:?}");
    }

    /// The case the whole reader exists for: something plausible-looking
    /// arrived and must not be mistaken for "nothing to pay".
    #[test]
    fn json_that_is_not_terms_is_a_distinct_error_from_well_formed_terms() {
        let error = parse_greeting(br#"{"error":"no route"}"#).expect_err("not terms");
        assert!(matches!(error, GreetingError::NotTerms(_)), "{error:?}");
    }

    #[test]
    fn an_offerless_greeting_is_not_a_free_ride() {
        let body = br#"{"x402Version":2,"resource":{"url":"g.toon.relay"},"accepts":[]}"#;
        assert_eq!(parse_greeting(body), Err(GreetingError::NoOffer));
    }

    #[test]
    fn an_unreadable_amount_is_refused_rather_than_rounded_to_zero() {
        let body = well_formed().replace(r#""amount":"1000""#, r#""amount":"lots""#);
        assert_ne!(
            body,
            well_formed(),
            "the fixture must actually have changed"
        );
        assert_eq!(
            parse_greeting(body.as_bytes()),
            Err(GreetingError::UnreadableAmount("lots".to_string()))
        );
    }

    #[test]
    fn a_future_x402_version_is_refused_rather_than_best_effort_parsed() {
        let body = well_formed().replace(r#""x402Version":2"#, r#""x402Version":3"#);
        assert_ne!(
            body,
            well_formed(),
            "the fixture must actually have changed"
        );
        assert_eq!(
            parse_greeting(body.as_bytes()),
            Err(GreetingError::UnsupportedVersion(3))
        );
    }

    /// A greeting from an edge that writes fewer decorative fields, or more
    /// of them, is still terms: only what a payer must act on is required.
    #[test]
    fn a_leaner_or_richer_greeting_still_reads_as_terms() {
        let lean = br#"{"x402Version":2,"resource":{"url":"g.toon.relay"},
            "accepts":[{"amount":"7","payTo":"g.toon.relay","futureField":true}]}"#;
        let terms = parse_greeting(lean).expect("the essentials are all there");
        assert_eq!(terms.price(), Some(7));
        assert_eq!(terms.offer().unwrap().extra, X402ChannelExtra::default());
    }

    #[test]
    fn the_evm_domain_is_read_from_either_settlement_shape() {
        let evm = X402SettlementTerms {
            chain: "evm:31337".to_string(),
            settlement_address: "0x1".to_string(),
            token_network_registry: "0x2".to_string(),
            token_network: "0x3".to_string(),
            token_address: "0x4".to_string(),
            decimals: 6,
        };
        let solana = X402SolanaSettlementTerms {
            chain: "solana".to_string(),
            settlement_address: "Sett".to_string(),
            program_id: "Prog".to_string(),
            token_address: "Mint".to_string(),
            decimals: 6,
        };

        let mut terms = parse_greeting(well_formed().as_bytes()).unwrap();
        assert_eq!(terms.evm_settlement(), None);

        // The per-chain list alone answers, Solana entry and all...
        terms.offer_mut().unwrap().extra.settlements = vec![
            X402ChainSettlementTerms::Solana(solana),
            X402ChainSettlementTerms::Evm(evm.clone()),
        ];
        assert_eq!(terms.evm_settlement(), Some(&evm));

        // ...as does the legacy single object on a pre-#632 greeting.
        terms.offer_mut().unwrap().extra.settlements.clear();
        terms.offer_mut().unwrap().extra.settlement = Some(evm.clone());
        assert_eq!(terms.evm_settlement(), Some(&evm));
    }

    /// The untagged enum's disambiguation is structural, so it has to
    /// survive a round trip rather than merely compile.
    #[test]
    fn a_settlements_list_round_trips_through_json() {
        let terms: X402PaymentRequired = serde_json::from_str(&well_formed()).unwrap();
        let json = serde_json::to_vec(&terms).unwrap();
        assert_eq!(parse_greeting(&json), Ok(terms));
    }

    // -- ADR 0074 decision 8: the greeting's batch-settlement accepts[] entries --

    fn evm_batch_settlement_fact() -> X402BatchSettlementTerms {
        X402BatchSettlementTerms::Evm(X402BatchSettlementEvmTerms {
            network: "eip155:84532".to_string(),
            asset: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".to_string(),
            pay_to: "0xf29fd62c4848b9573c9b90adbf61b664f386d9cf".to_string(),
            receiver_authorizer: "0xf29fd62c4848b9573c9b90adbf61b664f386d9cf".to_string(),
            min_withdraw_delay_secs: 86_400,
            name: "USDC".to_string(),
            version: "2".to_string(),
        })
    }

    fn solana_batch_settlement_fact() -> X402BatchSettlementTerms {
        X402BatchSettlementTerms::Solana(X402BatchSettlementSolanaTerms {
            network: "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1".to_string(),
            asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            pay_to: "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin".to_string(),
            fee_payer: "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin".to_string(),
            min_grace_period_secs: 86_400,
        })
    }

    /// The greeting keeps its `toon-channel` entry first and unchanged, and
    /// gains one x402-valid `batch-settlement` entry per opted-in chain --
    /// the exact shape a stock `@x402/evm` client builds a `ChannelConfig`
    /// from (issue #1345's "done when").
    #[test]
    fn a_node_opted_into_both_chains_greets_with_a_toon_channel_entry_and_two_batch_settlement_entries(
    ) {
        let facts = NodeFacts {
            batch_settlements: vec![evm_batch_settlement_fact(), solana_batch_settlement_fact()],
            ..Default::default()
        };
        let body = terms_body(&GreetingTerms {
            destination: "g.toon.ario",
            price: Price::flat(1000),
            payload_len: 0,
            node: Some(&facts),
            ..Default::default()
        });
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(value["accepts"].as_array().unwrap().len(), 3);
        assert_eq!(value["accepts"][0]["scheme"], "toon-channel");

        assert_eq!(
            value["accepts"][1],
            serde_json::json!({
                "scheme": "batch-settlement",
                "network": "eip155:84532",
                "amount": "1000",
                "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                "payTo": "0xf29fd62c4848b9573c9b90adbf61b664f386d9cf",
                "maxTimeoutSeconds": 60,
                "extra": {
                    "receiverAuthorizer": "0xf29fd62c4848b9573c9b90adbf61b664f386d9cf",
                    "withdrawDelay": 86400,
                    "name": "USDC",
                    "version": "2"
                }
            }),
            "a stock @x402/evm client builds its ChannelConfig from payTo and extra alone"
        );
        assert_eq!(
            value["accepts"][2],
            serde_json::json!({
                "scheme": "batch-settlement",
                "network": "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1",
                "amount": "1000",
                "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                "payTo": "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin",
                "maxTimeoutSeconds": 60,
                "extra": {
                    "feePayer": "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin",
                    "withdrawDelay": 86400
                }
            })
        );

        // The greeting still reads as ordinary toon-channel terms: `.offer()`
        // and every accessor built on it see straight through the new
        // entries to the one they have always described.
        let terms = parse_greeting(&body).expect("well-formed");
        assert_eq!(terms.price(), Some(1000));
        assert_eq!(terms.pay_to(), Some("g.toon.ario"));
        assert_eq!(terms.batch_settlement_offers().count(), 2);
    }

    /// A node that has opted into neither chain greets exactly as it did
    /// before ADR 0074 -- one entry, no `batchSettlements` anywhere. This is
    /// the compatibility claim every existing greeting test in this
    /// workspace already relies on.
    #[test]
    fn a_node_with_no_batch_settlement_opt_in_greets_with_one_entry_only() {
        let body = terms_body(&GreetingTerms {
            destination: "g.toon.relay",
            price: Price::flat(1000),
            payload_len: 0,
            ..Default::default()
        });
        let terms = parse_greeting(&body).expect("well-formed");
        assert_eq!(terms.accepts.len(), 1);
        assert_eq!(terms.batch_settlement_offers().count(), 0);
    }

    /// A batch-settlement entry round-trips through JSON byte for byte --
    /// the untagged enum's disambiguation from a `toon-channel` entry has to
    /// survive parsing, not merely construction.
    #[test]
    fn a_batch_settlement_entry_round_trips_through_json() {
        let facts = NodeFacts {
            batch_settlements: vec![evm_batch_settlement_fact()],
            ..Default::default()
        };
        let body = terms_body(&GreetingTerms {
            destination: "g.toon.ario",
            price: Price::flat(1000),
            payload_len: 0,
            node: Some(&facts),
            ..Default::default()
        });
        let terms = parse_greeting(&body).expect("well-formed");
        let reserialized = serde_json::to_vec(&terms).unwrap();
        assert_eq!(parse_greeting(&reserialized), Ok(terms));
    }
}
