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

/// The x402 v2 payment-required greeting (client-edge-spec.md §1.4).
///
/// **Every entry of `accepts` is an x402-valid `batch-settlement` entry**,
/// one per chain this node settles on (ADR 0075 decision 10, issue #1384):
/// the `toon-channel` entry that led the list until then is gone with its
/// scheme, so a stock x402 client can read every offer here. What that entry
/// also carried -- the facts about this route and this node that are TOON's
/// rather than x402's (the quoted amount, the price schedule, the addresses
/// and endpoints, the transport a route requires, the session lease) -- ride
/// in x402 v2's own slot for exactly that, `extensions`, under the key
/// `toon` ([`X402ToonExtension`]). They are there on every greeting, even
/// one from a node that settles on no chain and so offers no `accepts[]`
/// entry at all, which is what a peer carriage's greeting is.
///
/// # What is required on the way in
///
/// Deserialization is deliberately more forgiving than serialization is
/// exact: only what a payer must have to act -- the version, the resource,
/// and the `toon` extension's `amount` -- is required to read one back, and
/// what a payer cannot do without is checked in [`parse_greeting`], not
/// silently defaulted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct X402PaymentRequired {
    #[serde(rename = "x402Version")]
    pub x402_version: u32,
    pub resource: X402Resource,
    /// What a client should send to use the addressed route (issue #1210):
    /// the matching `[[routes]] request` table, converted to JSON verbatim.
    /// Sits beside `resource` rather than inside `accepts[]` -- it describes
    /// the *resource*, not a payment option. Absent -- not `null` -- when
    /// the route configured none.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub request: Option<serde_json::Value>,
    /// One x402-valid `batch-settlement` entry per chain this node settles
    /// on (ADR 0074 decision 8, ADR 0075 decision 10) -- the whole list.
    /// Empty on a node that settles on no chain.
    #[serde(default)]
    pub accepts: Vec<X402BatchSettlementOption>,
    /// x402 v2's extension slot, carrying TOON's own terms under `toon`.
    /// Always written by [`terms_body`]; a greeting without it is refused by
    /// [`parse_greeting`] as [`GreetingError::NoOffer`], never read as free.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub extensions: Option<X402Extensions>,
}

impl X402PaymentRequired {
    /// TOON's own terms for the greeted request: `extensions.toon.info`.
    /// `None` only for a greeting [`parse_greeting`] would have refused.
    pub fn toon(&self) -> Option<&X402ToonTerms> {
        self.extensions
            .as_ref()
            .map(|extensions| &extensions.toon.info)
    }

    /// Every `batch-settlement` entry this greeting offers, in the order the
    /// emitting node's chains were walked.
    pub fn batch_settlement_offers(&self) -> impl Iterator<Item = &X402BatchSettlementOption> {
        self.accepts.iter()
    }

    /// What the **greeted packet** costs, in the asset's base units: the
    /// `toon` extension's `amount`, the same figure every `accepts[]`
    /// entry's own `amount` quotes. `None` when there is no `toon`
    /// extension or its `amount` is not a decimal uint64 -- a greeting
    /// [`parse_greeting`] accepted always answers `Some`.
    ///
    /// For a flat route this is the route's whole price. For a route priced
    /// by size (ADR 0065) it is that schedule evaluated at the payload length
    /// of the request being answered; read [`Self::schedule`] to price
    /// another size.
    pub fn price(&self) -> Option<u64> {
        self.toon()?.amount.parse().ok()
    }

    /// The addressed route's whole price schedule (ADR 0065): its base, and
    /// what each started kibibyte of payload adds. No `pricePerKib` reads as
    /// a slope of zero -- a flat price.
    pub fn schedule(&self) -> Option<Price> {
        let toon = self.toon()?;
        let base = toon.price.parse().ok()?;
        let per_kib = match toon.price_per_kib.as_deref() {
            None => 0,
            Some(text) => text.parse().ok()?,
        };
        Some(Price::scheduled(base, per_kib))
    }

    /// The ILP address the greeted request was addressed to.
    pub fn ilp_address(&self) -> Option<&str> {
        self.toon().map(|toon| toon.ilp_address.as_str())
    }

    /// `"http"` or `"btp"` when this greeting answers a request that
    /// arrived over a transport the route does not accept (issue #701),
    /// naming the transport the route actually requires. `None` on an
    /// ordinary unpaid-request greeting.
    pub fn required_transport(&self) -> Option<&str> {
        self.toon()?.required_transport.as_deref()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct X402Resource {
    pub url: String,
}

/// The greeting's `extensions` object (x402 v2): TOON's one extension.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct X402Extensions {
    pub toon: X402ToonExtension,
}

/// The `toon` extension, in x402 v2's extension shape: the data in `info`
/// and a JSON Schema describing it in `schema` ([`toon_extension_schema`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct X402ToonExtension {
    pub info: X402ToonTerms,
    #[serde(default)]
    pub schema: serde_json::Value,
}

/// TOON's own terms for one greeted request (ADR 0075 decision 10): the
/// facts the retired `toon-channel` `accepts[]` entry used to carry that are
/// not a payment option -- moved here unchanged in name and meaning, less
/// that entry's `settlement`/`settlements` (the `toon-channel` channel
/// terms, deleted with the scheme) and its x402 envelope (`scheme`,
/// `network`, `payTo`, `maxTimeoutSeconds`, `httpEndpoint`), which were
/// only ever the address and path again.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct X402ToonTerms {
    /// The address the greeted request was sent to -- `resource.url` again,
    /// kept under the name the retired entry's `extra` used.
    #[serde(rename = "ilpAddress", default)]
    pub ilp_address: String,
    /// What the greeted request costs, in the asset's base units, as a
    /// decimal string. Required: it is the term a payer has to satisfy, and
    /// it is quoted here even on a greeting with no `accepts[]` entry.
    pub amount: String,
    /// The path a client posts ILP packets to, `/ilp`.
    #[serde(default)]
    pub endpoint: String,
    /// The **base** of the addressed route's price schedule (ADR 0065):
    /// equal to `amount` for a flat route.
    #[serde(default)]
    pub price: String,
    /// The **slope** of that schedule: what each started kibibyte of payload
    /// adds (ADR 0065, issue #984). Absent -- not `"0"` -- on a flat route.
    #[serde(
        rename = "pricePerKib",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub price_per_kib: Option<String>,
    /// The emitting node's own ILP address(es) (issue #807) -- never an echo
    /// of the probed destination the way `ilpAddress` is. Absent when the
    /// emitter configured no `[node] addresses`.
    #[serde(
        rename = "ilpAddresses",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub ilp_addresses: Vec<String>,
    /// Where clients pay the emitting node over BTP (issue #807). Absent
    /// when not configured.
    #[serde(
        rename = "btpEndpoint",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub btp_endpoint: Option<String>,
    /// Present, and self-diagnosing, exactly when this greeting answers a
    /// request that arrived over a transport its route's policy does not
    /// accept (issue #701): `"http"` or `"btp"`.
    #[serde(
        rename = "requiredTransport",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub required_transport: Option<String>,
    /// The session lease backstop TTL the emitting node's client session
    /// registry enforces (issue #722), in milliseconds; `0` from a carriage
    /// with no session registry of its own (the peer carriages).
    #[serde(rename = "sessionLeaseTtlMs", default)]
    pub session_lease_ttl_ms: u64,
}

/// The JSON Schema x402 v2 asks an extension to carry beside its `info`:
/// what [`X402ToonTerms`] serializes to. Informational -- this connector
/// never validates against it -- and constant, so every greeting carries the
/// same object.
pub fn toon_extension_schema() -> serde_json::Value {
    let string = serde_json::json!({ "type": "string" });
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "required": ["amount"],
        "properties": {
            "ilpAddress": string,
            "amount": string,
            "endpoint": string,
            "price": string,
            "pricePerKib": string,
            "ilpAddresses": { "type": "array", "items": string },
            "btpEndpoint": string,
            "requiredTransport": { "enum": ["http", "btp"] },
            "sessionLeaseTtlMs": { "type": "integer", "minimum": 0 }
        }
    })
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
/// structurally.
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
    /// How a payer's deposit moves `asset` into the channel -- x402's own
    /// `extra.assetTransferMethod` (EVM spec), configured as
    /// `[settlement.evm] asset_transfer_method`. **Always published**, even
    /// at its default (toon-client#695, ADR 0074 decision 8): x402 reads an
    /// absent value as `eip3009`, so the explicit default means the same to
    /// a stock client and leaves nothing for a TOON client to infer. Read
    /// as `eip3009` when absent, for the same reason -- a node that predates
    /// the field deposits by ERC-3009.
    #[serde(rename = "assetTransferMethod", default)]
    pub asset_transfer_method: X402AssetTransferMethod,
    /// The URL of the x402 facilitator this node relays deposits through
    /// and pays the gas of -- `[settlement.evm] facilitator_url`. TOON's own
    /// addition, as the Solana leg's `sponsorEndpoint` is: a stock x402
    /// seller calls its facilitator itself, but in TOON the deposit precedes
    /// the channel, so the payer calls it and the seller must name it
    /// (toon-client#695). This connector never calls it. Absent from the
    /// wire when the operator names none.
    #[serde(
        rename = "facilitator",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub facilitator: Option<String>,
}

/// x402's EVM `extra.assetTransferMethod`: how a `batch-settlement` deposit
/// moves the token (toon-client#695). Only the two x402 names; anything else
/// is refused rather than read as the default.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum X402AssetTransferMethod {
    /// ERC-3009 `receiveWithAuthorization` -- the token itself must
    /// implement ERC-3009 (Circle's USDC does). x402's default.
    #[default]
    #[serde(rename = "eip3009")]
    Eip3009,
    /// A Permit2 witness transfer, for any ERC-20 without ERC-3009. The
    /// payer's one-time Permit2 approval is gasless only when the named
    /// facilitator offers x402's `eip2612GasSponsoring` (a permit token) or
    /// `erc20ApprovalGasSponsoring` (a plain ERC-20).
    #[serde(rename = "permit2")]
    Permit2,
}

impl X402AssetTransferMethod {
    /// The wire (and config) spelling: `"eip3009"` or `"permit2"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Eip3009 => "eip3009",
            Self::Permit2 => "permit2",
        }
    }

    /// The inverse of [`Self::as_str`]; `None` for any other spelling.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "eip3009" => Some(Self::Eip3009),
            "permit2" => Some(Self::Permit2),
            _ => None,
        }
    }
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
    /// The token program that owns `asset`, base58 -- x402's **required**
    /// SVM `extra.tokenProgram` (SVM spec `#L178`), which a stock client
    /// passes as the `open`'s `token_program` account and derives both
    /// canonical ATAs under. Always SPL Token: the connected backend refuses
    /// to boot on a mint any other program owns, and the sponsor refuses a
    /// Token-2022 `open` as `token_program_unsupported`, so this is the one
    /// value an `open` it co-signs can name (issue #1357). A client still
    /// checks it against the mint's on-chain owner, as x402 requires.
    #[serde(rename = "tokenProgram")]
    pub token_program: String,
    /// The smallest opening deposit, in the mint's base units, this node's
    /// sponsor will co-sign an `open` for -- `[settlement.solana.batch_settlement]
    /// min_sponsored_deposit`. Published because ADR 0074 decision 5 has
    /// the sponsor refuse "below a *published* minimum deposit": a client
    /// must be able to read the bound before it builds an `open` the public
    /// sponsor endpoint would refuse. A decimal string, as every token
    /// amount on this greeting is.
    #[serde(rename = "minDeposit")]
    pub min_deposit: String,
    /// Where the payer-signed `open` is posted for this node to co-sign and
    /// submit: the sponsor endpoint's path, `/ilp/batch-settlement/solana/open`,
    /// served on the same client-edge listener as `POST /ilp`
    /// (client-edge-spec §1.11, issue #1357). A path, as the `toon`
    /// extension's `endpoint` is, and resolved the same way.
    /// This connector's own addition, like `minDeposit`: x402 hands a
    /// `deposit` to the server with the paid request and never names a
    /// facilitator to the client, and here the facilitator is this node.
    #[serde(rename = "sponsorEndpoint")]
    pub sponsor_endpoint: String,
}

/// The greeting's own `batch-settlement` `accepts[]` entry (ADR 0074
/// decision 8): [`X402BatchSettlementTerms`] plus the fields that describe
/// *this* request rather than this node's standing terms -- `scheme`,
/// `amount` (the same charge the `toon` extension quotes) and
/// `maxTimeoutSeconds`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct X402BatchSettlementOption {
    pub scheme: String,
    pub network: String,
    /// The addressed route's charge, in the asset's base units, as a decimal
    /// string -- identical to the `toon` extension's own `amount` and to
    /// every other entry's: each is an alternative way to pay the same
    /// charge.
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
/// `assetTransferMethod` is x402's own optional field, always written here;
/// `facilitator` is this connector's addition (toon-client#695), written
/// only when the operator names one -- see
/// [`X402BatchSettlementEvmTerms::asset_transfer_method`] and
/// [`X402BatchSettlementEvmTerms::facilitator`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct X402BatchSettlementEvmExtra {
    #[serde(rename = "receiverAuthorizer")]
    pub receiver_authorizer: String,
    #[serde(rename = "withdrawDelay")]
    pub withdraw_delay: u64,
    pub name: String,
    pub version: String,
    #[serde(rename = "assetTransferMethod", default)]
    pub asset_transfer_method: X402AssetTransferMethod,
    #[serde(
        rename = "facilitator",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub facilitator: Option<String>,
}

/// x402 SVM batch-settlement spec `#L170-L183`: `feePayer`, `withdrawDelay`
/// (the program's `grace_period`) and `tokenProgram` are the three it
/// requires, and together with `payTo` and `asset` they are every account
/// and field of the `open` a client builds. `minDeposit` and
/// `sponsorEndpoint` are this connector's own additions: the published
/// minimum its sponsor co-signs an `open` for (ADR 0074 decision 5), and
/// where to post it (decision 9).
///
/// x402's optional `recentBlockhash` and `recentSlot` are deliberately
/// **absent** (issue #1357). The spec calls them transaction-construction
/// hints a client MAY ignore and MUST refresh when stale (`#L180-L188`,
/// `#L997-L1001`); a blockhash lapses in about a minute, and this greeting
/// is a projection of standing node facts (ND-11) answered to anyone
/// without a chain read. The client needs an RPC regardless -- x402 has it
/// verify `tokenProgram` against the mint's on-chain owner (`#L273-L275`)
/// -- and fetches both from there.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct X402BatchSettlementSolanaExtra {
    #[serde(rename = "feePayer")]
    pub fee_payer: String,
    #[serde(rename = "withdrawDelay")]
    pub withdraw_delay: u64,
    #[serde(rename = "tokenProgram")]
    pub token_program: String,
    #[serde(rename = "minDeposit")]
    pub min_deposit: String,
    #[serde(rename = "sponsorEndpoint")]
    pub sponsor_endpoint: String,
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
                asset_transfer_method: evm.asset_transfer_method,
                facilitator: evm.facilitator.clone(),
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
                token_program: solana.token_program.clone(),
                min_deposit: solana.min_deposit.clone(),
                sponsor_endpoint: solana.sponsor_endpoint.clone(),
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
/// human debugging a link needs.
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
    /// Well-formed x402, but it carries no `toon` extension, so it quotes
    /// no amount -- which is not the same as there being nothing to pay. A
    /// greeting from a node that predates ADR 0075, whose amount rode a
    /// `toon-channel` `accepts[]` entry, reads this way too.
    #[error("the payment-required greeting quotes no TOON terms (no 'extensions.toon')")]
    NoOffer,
    /// The quoted amount is not a decimal uint64, so there is no amount to
    /// cover.
    #[error("the offered amount '{0}' is not a decimal uint64")]
    UnreadableAmount(String),
}

/// The x402 greeting's own `maxTimeoutSeconds` -- one figure, shared by
/// every emitter (issue #880).
const X402_MAX_TIMEOUT_SECONDS: u64 = 60;

/// Build and serialize a `payment-required` greeting (client-edge-spec.md
/// §1.4) -- **the** emitter, called by every carriage that answers an
/// unpaid or under-covering request with x402 terms rather than doing the
/// work: the client edge's HTTP carriage (a `402` body), its BTP carriage
/// (an `F06` REJECT's `payment-required` protocolData), and the peer
/// carriages' own `F06` REJECT (`peer-carriage-spec.md` §3.1). One
/// construction, in the one crate every emitter and every reader already
/// depends on.
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
    // ND-11: every node fact here is read off the SAME value the node
    // self-description is projected from -- never a second assembly.
    let ilp_addresses: &[String] = node
        .map(|node| node.ilp_addresses.as_slice())
        .unwrap_or(&[]);
    let btp_endpoint: Option<&str> = node.and_then(|node| node.btp_endpoint.as_deref());
    // ADR 0074 decision 8, ADR 0075 decision 10: one `batch-settlement`
    // entry per chain this node settles on, and nothing else.
    let batch_settlements: &[X402BatchSettlementTerms] = node
        .map(|node| node.batch_settlements.as_slice())
        .unwrap_or(&[]);
    let amount = price.charge(payload_len).to_string();
    let accepts = batch_settlements
        .iter()
        .map(|fact| batch_settlement_accept(fact, amount.clone()))
        .collect();
    let terms = X402PaymentRequired {
        x402_version: X402_VERSION,
        resource: X402Resource {
            url: destination.to_string(),
        },
        request: request.cloned(),
        accepts,
        extensions: Some(X402Extensions {
            toon: X402ToonExtension {
                info: X402ToonTerms {
                    ilp_address: destination.to_string(),
                    amount,
                    endpoint: "/ilp".to_string(),
                    price: price.base().to_string(),
                    price_per_kib: (!price.is_flat()).then(|| price.per_kib().to_string()),
                    ilp_addresses: ilp_addresses.to_vec(),
                    btp_endpoint: btp_endpoint.map(str::to_string),
                    required_transport: required_transport.map(str::to_string),
                    session_lease_ttl_ms,
                },
                schema: toon_extension_schema(),
            },
        }),
    };
    serde_json::to_vec(&terms).expect("x402 terms always serialize")
}

/// What [`terms_body`] needs to know to quote one offer.
///
/// Everything but `destination` and `price` has a meaningful empty value,
/// so a carriage that carries none of it writes
/// `GreetingTerms { destination, price, ..Default::default() }`.
#[derive(Debug, Clone, Copy, Default)]
pub struct GreetingTerms<'a> {
    /// `resource.url`, and the `toon` extension's `ilpAddress`.
    pub destination: &'a str,
    /// What that address charges: the whole schedule (ADR 0065), quoted as
    /// the `toon` extension's `price` (its base) and `pricePerKib` (its
    /// slope).
    pub price: Price,
    /// The payload length of the request being answered, in bytes -- what
    /// `amount` is quoted for. A carriage greeting a request that never
    /// became a packet passes `0`, and gets the base.
    pub payload_len: usize,
    /// The emitting node's own facts -- its addresses, its BTP endpoint and
    /// the chains it settles on ([`crate::node::NodeFacts`], ADR 0050): the
    /// same value the node self-description is projected from (ND-11).
    /// `None` for a carriage that describes no node at all -- the peer
    /// carriages, whose counterparty already knows this node and needs only
    /// the figure quoted -- which greets with no `accepts[]` entry.
    pub node: Option<&'a NodeFacts>,
    /// `Some("http" | "btp")` only when this same shape is reused to tell a
    /// client it used the wrong transport entirely (issue #701).
    pub required_transport: Option<&'a str>,
    /// The emitting node's client session lease backstop (issue #722); `0`
    /// from a carriage with no client session registry of its own.
    pub session_lease_ttl_ms: u64,
    /// What a client should send to use the addressed route (issue #1210).
    pub request: Option<&'a serde_json::Value>,
}

/// Read a `payment-required` greeting's terms.
///
/// Everything this returns `Err` for is a greeting that *was present*. A
/// caller that found no greeting at all must not route through here: it has
/// an ordinary answer, not a malformed one -- see [`GreetingError`].
pub fn parse_greeting(bytes: &[u8]) -> Result<X402PaymentRequired, GreetingError> {
    // Two steps rather than one so "not JSON" and "not terms" stay
    // distinguishable.
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| GreetingError::NotJson(error.to_string()))?;
    let terms: X402PaymentRequired = serde_json::from_value(value)
        .map_err(|error| GreetingError::NotTerms(error.to_string()))?;

    if terms.x402_version != X402_VERSION {
        return Err(GreetingError::UnsupportedVersion(terms.x402_version));
    }
    let toon = terms.toon().ok_or(GreetingError::NoOffer)?;
    if toon.amount.parse::<u64>().is_err() {
        return Err(GreetingError::UnreadableAmount(toon.amount.clone()));
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
            "accepts": [],
            "extensions": { "toon": { "info": {
                "ilpAddress": "g.toon.relay",
                "amount": "1000",
                "endpoint": "/ilp",
                "price": "1000",
                "sessionLeaseTtlMs": 300000
            }, "schema": {} } }
        })
        .to_string()
    }

    #[test]
    fn a_well_formed_greeting_yields_its_terms() {
        let terms = parse_greeting(well_formed().as_bytes()).expect("well-formed terms");
        assert_eq!(terms.price(), Some(1000));
        assert_eq!(terms.ilp_address(), Some("g.toon.relay"));
        assert_eq!(terms.required_transport(), None);
        assert_eq!(terms.toon().unwrap().session_lease_ttl_ms, 300_000);
    }

    /// ADR 0065: a flat route's greeting carries no slope at all.
    #[test]
    fn a_flat_routes_greeting_carries_no_slope_at_all() {
        let body = terms_body(&GreetingTerms {
            destination: "g.toon.relay",
            price: Price::flat(1000),
            payload_len: 4096,
            ..Default::default()
        });
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            value["extensions"]["toon"]["info"]
                .get("pricePerKib")
                .is_none(),
            "a flat greeting must not carry the field at all, got: {value}"
        );
        let terms = parse_greeting(&body).expect("well-formed");
        assert_eq!(terms.price(), Some(1000));
        assert_eq!(terms.schedule(), Some(Price::flat(1000)));
    }

    /// Issue #1210: a route's `request` table rides at the top level of the
    /// greeting, beside `resource`.
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
        let terms = parse_greeting(&body).expect("well-formed");
        assert_eq!(terms.request, Some(request));
    }

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
        assert_eq!(terms.price(), Some(4_000));
        let schedule = terms
            .schedule()
            .expect("a schedule route publishes its schedule");
        assert_eq!(schedule, price);
        assert_eq!(schedule.charge(2 * 1024 * 1024), 62_440);
        assert_eq!(terms.toon().unwrap().price, "1000");
        assert_eq!(terms.toon().unwrap().price_per_kib.as_deref(), Some("30"));
    }

    #[test]
    fn a_greeting_with_no_slope_reads_as_a_flat_schedule() {
        let terms = parse_greeting(well_formed().as_bytes()).expect("well-formed");
        assert_eq!(terms.schedule(), Some(Price::flat(1000)));
        assert!(terms.schedule().unwrap().is_flat());
    }

    #[test]
    fn bytes_that_are_not_json_are_their_own_error() {
        let error = parse_greeting(b"\xff\xfe not json").expect_err("unreadable");
        assert!(matches!(error, GreetingError::NotJson(_)), "{error:?}");
    }

    /// Something plausible-looking arrived and must not be mistaken for
    /// "nothing to pay".
    #[test]
    fn json_that_is_not_terms_is_a_distinct_error_from_well_formed_terms() {
        let error = parse_greeting(br#"{"error":"no route"}"#).expect_err("not terms");
        assert!(matches!(error, GreetingError::NotTerms(_)), "{error:?}");
    }

    /// A greeting with no `toon` extension quotes nothing, which is not a
    /// free ride.
    #[test]
    fn a_greeting_quoting_no_toon_terms_is_not_a_free_ride() {
        let body = br#"{"x402Version":2,"resource":{"url":"g.toon.relay"},"accepts":[]}"#;
        assert_eq!(parse_greeting(body), Err(GreetingError::NoOffer));
    }

    /// A greeting from a node that predates ADR 0075 led with a
    /// `toon-channel` `accepts[]` entry. It is not x402 `batch-settlement`
    /// terms, and it is refused, never read as free.
    #[test]
    fn a_pre_adr_0075_toon_channel_greeting_is_refused() {
        let old = serde_json::json!({
            "x402Version": 2,
            "resource": { "url": "g.toon.relay" },
            "accepts": [{
                "scheme": "toon-channel",
                "network": "g.toon.relay",
                "amount": "1000",
                "payTo": "g.toon.relay",
                "maxTimeoutSeconds": 60,
                "extra": { "price": "1000" }
            }]
        })
        .to_string();
        assert!(parse_greeting(old.as_bytes()).is_err());
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

    /// Only what a payer must act on is required of the `toon` extension.
    #[test]
    fn a_leaner_or_richer_greeting_still_reads_as_terms() {
        let lean = br#"{"x402Version":2,"resource":{"url":"g.toon.relay"},
            "extensions":{"toon":{"info":{"amount":"7","futureField":true}}}}"#;
        let terms = parse_greeting(lean).expect("the essentials are all there");
        assert_eq!(terms.price(), Some(7));
        assert!(terms.accepts.is_empty());
    }

    // -- ADR 0074 decision 8 / ADR 0075 decision 10: accepts[] --

    fn evm_batch_settlement_fact() -> X402BatchSettlementTerms {
        X402BatchSettlementTerms::Evm(X402BatchSettlementEvmTerms {
            network: "eip155:84532".to_string(),
            asset: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".to_string(),
            pay_to: "0xf29fd62c4848b9573c9b90adbf61b664f386d9cf".to_string(),
            receiver_authorizer: "0xf29fd62c4848b9573c9b90adbf61b664f386d9cf".to_string(),
            min_withdraw_delay_secs: 86_400,
            name: "USDC".to_string(),
            version: "2".to_string(),
            asset_transfer_method: X402AssetTransferMethod::Eip3009,
            facilitator: None,
        })
    }

    /// The same EVM fact from a node that deposits by Permit2 and names the
    /// facilitator it relays deposits through (toon-client#695).
    fn evm_permit2_batch_settlement_fact() -> X402BatchSettlementTerms {
        X402BatchSettlementTerms::Evm(X402BatchSettlementEvmTerms {
            network: "eip155:84532".to_string(),
            asset: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".to_string(),
            pay_to: "0xf29fd62c4848b9573c9b90adbf61b664f386d9cf".to_string(),
            receiver_authorizer: "0xf29fd62c4848b9573c9b90adbf61b664f386d9cf".to_string(),
            min_withdraw_delay_secs: 86_400,
            name: "USDC".to_string(),
            version: "2".to_string(),
            asset_transfer_method: X402AssetTransferMethod::Permit2,
            facilitator: Some("https://facilitator.example/x402".to_string()),
        })
    }

    fn solana_batch_settlement_fact() -> X402BatchSettlementTerms {
        X402BatchSettlementTerms::Solana(X402BatchSettlementSolanaTerms {
            network: "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1".to_string(),
            asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            pay_to: "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin".to_string(),
            fee_payer: "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin".to_string(),
            min_grace_period_secs: 86_400,
            min_deposit: "1000000".to_string(),
            token_program: "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA".to_string(),
            sponsor_endpoint: "/ilp/batch-settlement/solana/open".to_string(),
        })
    }

    /// Every `accepts[]` entry is an x402-valid `batch-settlement` entry --
    /// there is no `toon-channel` entry any more (issue #1384) -- exactly the
    /// shape a stock x402 client builds a channel from, and TOON's own terms
    /// ride in `extensions.toon`.
    #[test]
    fn a_node_on_both_chains_greets_with_two_batch_settlement_entries_and_nothing_else() {
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

        let accepts = value["accepts"].as_array().unwrap();
        assert_eq!(accepts.len(), 2);
        assert!(
            accepts
                .iter()
                .all(|entry| entry["scheme"] == "batch-settlement"),
            "every entry is x402 batch-settlement: {value}"
        );
        assert!(!body.windows(12).any(|w| w == b"toon-channel"));
        assert_eq!(
            accepts[0],
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
                    "version": "2",
                    "assetTransferMethod": "eip3009"
                }
            }),
            "a stock @x402/evm client builds its ChannelConfig from payTo and extra alone"
        );
        assert_eq!(
            accepts[1],
            serde_json::json!({
                "scheme": "batch-settlement",
                "network": "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1",
                "amount": "1000",
                "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                "payTo": "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin",
                "maxTimeoutSeconds": 60,
                "extra": {
                    "feePayer": "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin",
                    "withdrawDelay": 86400,
                    "tokenProgram": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
                    "minDeposit": "1000000",
                    "sponsorEndpoint": "/ilp/batch-settlement/solana/open"
                }
            })
        );
        assert_eq!(value["extensions"]["toon"]["info"]["amount"], "1000");
        assert_eq!(
            value["extensions"]["toon"]["schema"],
            toon_extension_schema()
        );

        let terms = parse_greeting(&body).expect("well-formed");
        assert_eq!(terms.price(), Some(1000));
        assert_eq!(terms.batch_settlement_offers().count(), 2);
    }

    /// A node that settles on no chain -- and a peer carriage, which
    /// describes no node -- greets with no `accepts[]` entry, and still
    /// quotes its amount.
    #[test]
    fn a_node_on_no_chain_greets_with_no_entry_and_still_quotes() {
        let body = terms_body(&GreetingTerms {
            destination: "g.toon.relay",
            price: Price::flat(1000),
            payload_len: 0,
            ..Default::default()
        });
        let terms = parse_greeting(&body).expect("well-formed");
        assert!(terms.accepts.is_empty());
        assert_eq!(terms.price(), Some(1000));
    }

    /// toon-client#695: a node that deposits by Permit2 and names its
    /// facilitator publishes both in the EVM entry's `extra` -- x402's own
    /// `assetTransferMethod`, and TOON's `facilitator` -- and the Solana
    /// entry is untouched by either.
    #[test]
    fn a_permit2_node_greets_with_its_transfer_method_and_its_facilitator() {
        let facts = NodeFacts {
            batch_settlements: vec![
                evm_permit2_batch_settlement_fact(),
                solana_batch_settlement_fact(),
            ],
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

        assert_eq!(
            value["accepts"][0],
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
                    "version": "2",
                    "assetTransferMethod": "permit2",
                    "facilitator": "https://facilitator.example/x402"
                }
            })
        );
        let solana_extra = value["accepts"][1]["extra"].as_object().unwrap();
        assert!(!solana_extra.contains_key("assetTransferMethod"));
        assert!(!solana_extra.contains_key("facilitator"));

        let terms = parse_greeting(&body).expect("well-formed");
        let reserialized = serde_json::to_vec(&terms).unwrap();
        assert_eq!(parse_greeting(&reserialized), Ok(terms));
    }

    /// An EVM fact from a node that predates toon-client#695 carries
    /// neither field. It still reads as EVM terms -- the untagged enum must
    /// not fall through to Solana -- with x402's own default,
    /// `eip3009`, and no facilitator.
    #[test]
    fn an_evm_fact_without_the_deposit_fields_reads_as_eip3009_and_no_facilitator() {
        let fact: X402BatchSettlementTerms = serde_json::from_value(serde_json::json!({
            "network": "eip155:84532",
            "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            "payTo": "0xf29fd62c4848b9573c9b90adbf61b664f386d9cf",
            "receiverAuthorizer": "0xf29fd62c4848b9573c9b90adbf61b664f386d9cf",
            "withdrawDelay": 86400,
            "name": "USDC",
            "version": "2"
        }))
        .expect("an older node's EVM fact");
        assert_eq!(fact, evm_batch_settlement_fact());

        let extra: X402BatchSettlementExtra = serde_json::from_value(serde_json::json!({
            "receiverAuthorizer": "0xf29fd62c4848b9573c9b90adbf61b664f386d9cf",
            "withdrawDelay": 86400,
            "name": "USDC",
            "version": "2"
        }))
        .expect("an older node's EVM extra");
        match extra {
            X402BatchSettlementExtra::Evm(evm) => {
                assert_eq!(evm.asset_transfer_method, X402AssetTransferMethod::Eip3009);
                assert_eq!(evm.facilitator, None);
            }
            other => panic!("expected EVM extra, got {other:?}"),
        }
    }

    /// An `assetTransferMethod` this connector has no name for is not
    /// silently read as the default: a payer that deposits the wrong way
    /// has a deposit the contract refuses.
    #[test]
    fn an_unknown_asset_transfer_method_does_not_read_as_evm_terms() {
        let result: Result<X402BatchSettlementEvmTerms, _> =
            serde_json::from_value(serde_json::json!({
                "network": "eip155:84532",
                "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                "payTo": "0xf29fd62c4848b9573c9b90adbf61b664f386d9cf",
                "receiverAuthorizer": "0xf29fd62c4848b9573c9b90adbf61b664f386d9cf",
                "withdrawDelay": 86400,
                "name": "USDC",
                "version": "2",
                "assetTransferMethod": "eip2612"
            }));
        assert!(result.is_err());
    }

    /// A greeting round-trips through JSON: the untagged `extra` has to
    /// survive parsing, not merely construction.
    #[test]
    fn a_greeting_round_trips_through_json() {
        let facts = NodeFacts {
            batch_settlements: vec![evm_batch_settlement_fact(), solana_batch_settlement_fact()],
            ..Default::default()
        };
        let body = terms_body(&GreetingTerms {
            destination: "g.toon.ario",
            price: Price::scheduled(1000, 3),
            payload_len: 2048,
            node: Some(&facts),
            ..Default::default()
        });
        let terms = parse_greeting(&body).expect("well-formed");
        let reserialized = serde_json::to_vec(&terms).unwrap();
        assert_eq!(parse_greeting(&reserialized), Ok(terms));
    }
}
