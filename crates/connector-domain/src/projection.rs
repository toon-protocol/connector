//! Balances as a projection over a durable journal (ADR 0005, issue #424).
//! Pure, no I/O -- the journal itself (a file, in `connector-runtime`) is an
//! infrastructure concern; folding its entries into a balance figure is not,
//! so it lives here where it can be property-tested without a filesystem.
//!
//! [`JournalEntry`] is deliberately the exact alphabet named in the issue's
//! own acceptance criteria -- "claims sent, claims received with their
//! watermarks, and fulfilments not yet covered by a claim" -- and nothing
//! more. The last of those, `InboundFulfillmentRecorded`, backed the
//! exposure/ceiling accounting ADR 0033 (issue #882) retired; it stays in
//! the alphabet only so a pre-#882 journal still decodes. Balances are
//! never themselves an entry: per ADR 0005 they are arithmetic on the
//! entries above, recomputed by [`Projection::apply`] rather than stored.

use std::collections::BTreeMap;

/// One durably-recorded fact about money state (ADR 0005). Everything else
/// -- a peer's owed balance -- is re-derived by folding a sequence of these
/// with [`Projection::apply`], never stored directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalEntry {
    /// This connector signed an outbound claim owed to `peer_id` on
    /// `channel_id`, whose cumulative amount now stands at
    /// `cumulative_amount` (`ClaimBook::record_fulfillment`, ADR 0004's
    /// "sending connector pays the next hop"). Superseding rather than
    /// additive: the latest entry for a `peer_id` is the whole of what is
    /// owed, exactly like the claim it was signed from.
    OutboundClaimSigned {
        peer_id: String,
        channel_id: String,
        nonce: u64,
        cumulative_amount: u64,
    },
    /// A signed claim on `channel_id` was verified and accepted, advancing
    /// that channel's watermark to `nonce`/`cumulative_amount`
    /// (`ClaimBook::accept_inbound`, peer-semantics-pre-868.md §3.4). `signature` is
    /// carried through opaque (chain- and scheme-specific verification
    /// already happened before this entry is ever appended) and durably
    /// retained rather than discarded once accepted: on-chain redemption
    /// (issue #425) needs the actual claim, not just its watermark, and per
    /// ADR 0005 what is signed is exactly what this journal exists to keep.
    InboundClaimAccepted {
        channel_id: String,
        nonce: u64,
        cumulative_amount: u64,
        signature: Vec<u8>,
    },
    /// Historical entry kind, no longer produced (ADR 0031, ADR 0033, issue
    /// #882): a packet arriving on `channel_id` fulfilled for `amount`,
    /// extending this connector's exposure to that channel's counterparty
    /// until a covering claim was accepted -- the credit-window accounting
    /// this connector kept before every peer PREPARE carried its own
    /// covering claim. Kept in the alphabet, not removed, so a journal a
    /// pre-#882 build already wrote still decodes; [`Projection::apply`]
    /// folds it into nothing.
    InboundFulfillmentRecorded { channel_id: String, amount: u64 },
    /// `channel_id`'s watermark was durably reset because this connector
    /// discovered the chain no longer vouches for it -- settled,
    /// deallocated, or otherwise gone (issue #977). Written only by
    /// `connector_client_edge::ClientClaimGate::reset_watermark`, into the
    /// client edge's own journal -- a channel's deterministic on-chain
    /// address means a reopened channel reuses the exact key its settled
    /// predecessor's watermark was filed under, and without this entry a
    /// reopened channel would inherit that predecessor's watermark forever,
    /// charging its payer again for units already settled on chain (or, at
    /// the limit, refusing every claim it could ever present). Folds into
    /// nothing here: [`Projection`] tracks the peer semantics's own book, which
    /// this entry kind is never written to (see the client edge's own
    /// journal file, kept separate from the peer semantics's for exactly this
    /// reason) -- it is in this shared alphabet only so both journals'
    /// entries decode through one enum, matching every other entry kind
    /// here.
    InboundClaimWatermarkReset { channel_id: String },
    /// `channel_id`'s watermark was durably rolled back to `nonce`/
    /// `cumulative_amount` because the PREPARE the claim that reached that
    /// watermark covered is now known never to have been carried: a
    /// client-priced forwarded route (ADR 0028) admits the client's claim
    /// before learning whether the next hop will fulfil it, and the next
    /// hop's own terminal reject (F06 after a covered retry, T01
    /// unreachable) is discoverable only after admission (issue #1012).
    /// Written only by `connector_client_edge::ClientClaimGate::roll_back`,
    /// into the client edge's own journal, immediately after the
    /// `InboundClaimAccepted` entry it undoes -- never speculatively, and
    /// never for a claim a later admission has already superseded.
    ///
    /// Unlike `InboundClaimAccepted`, whose replay folds by componentwise
    /// max (this module's own doc), a replay of this entry SETS the
    /// watermark directly, matching `InboundClaimWatermarkReset`: it exists
    /// specifically to move a watermark down, which a legitimately
    /// advancing claim never does, so folding it by max would silently
    /// undo the very thing it records. Folds into nothing here: like
    /// `InboundClaimWatermarkReset`, this entry kind is written only to the
    /// client edge's own journal, and [`Projection`] tracks the peer
    /// wire's own book.
    InboundClaimRolledBack {
        channel_id: String,
        nonce: u64,
        cumulative_amount: u64,
    },
    /// The client edge accepted its first voucher on the x402
    /// `batch-settlement` channel `channel_id` (ADR 0074): the canonical
    /// `evm:0x…` or `solana:…` key its watermark is filed under.
    /// `presentation` is what re-admits the channel to its settlement
    /// backend after a restart, carried opaque as `InboundClaimAccepted`'s
    /// signature is: on EVM the channel's `ChannelConfig`, which the
    /// contract stores only as a hash and a voucher signs only as that
    /// hash, so without it a voucher already accepted could never be
    /// `claim`ed; on Solana nothing, since the channel account holds every
    /// field.
    ///
    /// Written only by `connector_client_edge::ClientClaimGate`, into the
    /// client edge's own journal, in the same batch as -- and immediately
    /// before -- the `InboundClaimAccepted` of the channel's first accepted
    /// voucher. It also marks the key as a batch-settlement channel's, which
    /// the gate's `TokenNetwork`-shaped sweep must never judge. Folds into
    /// nothing here, like the other client-edge-only kinds.
    BatchChannelAdmitted {
        channel_id: String,
        presentation: Vec<u8>,
    },
}

/// Balances, derived in memory by folding a journal (ADR 0005). Never a
/// source of truth, always rebuildable from [`Projection::from_entries`] --
/// the same result whether folded incrementally as entries occur or all at
/// once after a restart.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Projection {
    /// `peer_id` -> the cumulative amount this connector's latest signed
    /// claim owes that peer.
    outbound_owed: BTreeMap<String, u64>,
    /// `channel_id` -> the cumulative amount of the highest accepted claim
    /// on that channel.
    inbound_claimed: BTreeMap<String, u64>,
    /// `channel_id` -> the nonce of that same highest accepted claim, kept
    /// alongside `inbound_claimed` for the same reason `inbound_claim_signature`
    /// is: written from, and only from, the same `InboundClaimAccepted`
    /// entry. A redemption submitted without it is not redeemable on any
    /// real chain (issue #573) -- every chain this port settles on hashes
    /// the nonce into the signed material and enforces it on chain.
    inbound_claim_nonce: BTreeMap<String, u64>,
    /// `channel_id` -> the signature of that same highest accepted claim,
    /// kept alongside `inbound_claimed` rather than as a separate source of
    /// truth -- both fields are written from, and only from, the same
    /// `InboundClaimAccepted` entry (issue #425: what a redemption submits).
    inbound_claim_signature: BTreeMap<String, Vec<u8>>,
}

impl Projection {
    /// Fold one more entry into this projection.
    pub fn apply(&mut self, entry: &JournalEntry) {
        match entry {
            JournalEntry::OutboundClaimSigned {
                peer_id,
                cumulative_amount,
                ..
            } => {
                self.outbound_owed
                    .insert(peer_id.clone(), *cumulative_amount);
            }
            JournalEntry::InboundClaimAccepted {
                channel_id,
                nonce,
                cumulative_amount,
                signature,
            } => {
                self.inbound_claimed
                    .insert(channel_id.clone(), *cumulative_amount);
                self.inbound_claim_nonce.insert(channel_id.clone(), *nonce);
                self.inbound_claim_signature
                    .insert(channel_id.clone(), signature.clone());
            }
            // Historical entry kind (ADR 0031, ADR 0033, issue #882): a
            // pre-#882 journal may still carry these, but nothing is
            // tracked from them any more.
            JournalEntry::InboundFulfillmentRecorded { .. } => {}
            // Written only to the client edge's own journal, never this
            // one (issue #977) -- see the variant's own doc.
            JournalEntry::InboundClaimWatermarkReset { .. } => {}
            // Written only to the client edge's own journal, never this
            // one (issue #1012) -- see the variant's own doc.
            JournalEntry::InboundClaimRolledBack { .. } => {}
            // Written only to the client edge's own journal, never this
            // one (ADR 0074) -- see the variant's own doc.
            JournalEntry::BatchChannelAdmitted { .. } => {}
        }
    }

    /// Rebuild a projection from scratch by folding `entries` in order --
    /// what a node does with its journal on start (issue #424's "rebuilt
    /// from the journal on start").
    pub fn from_entries<'a>(entries: impl IntoIterator<Item = &'a JournalEntry>) -> Projection {
        let mut projection = Projection::default();
        for entry in entries {
            projection.apply(entry);
        }
        projection
    }

    /// The cumulative amount this connector's latest signed claim owes
    /// `peer_id`, or 0 if none has ever been signed.
    pub fn outbound_owed(&self, peer_id: &str) -> u64 {
        self.outbound_owed.get(peer_id).copied().unwrap_or(0)
    }

    /// The highest-nonce claim ever accepted on `channel_id`, as
    /// `(nonce, cumulative_amount, signature)` -- exactly what an on-chain
    /// redemption submits (issue #425, widened by #573 to carry the nonce
    /// the signature covers, without which no claim is redeemable on any
    /// real chain), and never a superseded one: this projection only ever
    /// retains the latest, so there is nothing else it could return. `None`
    /// before any claim has been accepted on this channel.
    pub fn latest_inbound_claim(&self, channel_id: &str) -> Option<(u64, u64, Vec<u8>)> {
        let nonce = *self.inbound_claim_nonce.get(channel_id)?;
        let cumulative_amount = *self.inbound_claimed.get(channel_id)?;
        let signature = self.inbound_claim_signature.get(channel_id)?.clone();
        Some((nonce, cumulative_amount, signature))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn signed(peer_id: &str, channel_id: &str, nonce: u64, cumulative_amount: u64) -> JournalEntry {
        JournalEntry::OutboundClaimSigned {
            peer_id: peer_id.to_string(),
            channel_id: channel_id.to_string(),
            nonce,
            cumulative_amount,
        }
    }

    fn accepted_with_signature(
        channel_id: &str,
        nonce: u64,
        cumulative_amount: u64,
        signature: &[u8],
    ) -> JournalEntry {
        JournalEntry::InboundClaimAccepted {
            channel_id: channel_id.to_string(),
            nonce,
            cumulative_amount,
            signature: signature.to_vec(),
        }
    }

    fn fulfilled(channel_id: &str, amount: u64) -> JournalEntry {
        JournalEntry::InboundFulfillmentRecorded {
            channel_id: channel_id.to_string(),
            amount,
        }
    }

    #[test]
    fn an_empty_projection_owes_nothing() {
        let projection = Projection::default();
        assert_eq!(projection.outbound_owed("peer-b"), 0);
    }

    #[test]
    fn an_outbound_claim_signed_is_owed_its_cumulative_amount() {
        let projection = Projection::from_entries(&[signed("peer-b", "channel-a", 1, 100)]);
        assert_eq!(projection.outbound_owed("peer-b"), 100);
    }

    #[test]
    fn a_later_outbound_claim_supersedes_rather_than_adds() {
        let projection = Projection::from_entries(&[
            signed("peer-b", "channel-a", 1, 100),
            signed("peer-b", "channel-a", 2, 150),
        ]);
        assert_eq!(projection.outbound_owed("peer-b"), 150);
    }

    /// ADR 0031/ADR 0033, issue #882: a pre-#882 journal may still carry
    /// `InboundFulfillmentRecorded` entries. They must still decode and
    /// replay without error -- they simply contribute nothing.
    #[test]
    fn a_historical_fulfillment_recorded_entry_replays_as_a_no_op() {
        let projection = Projection::from_entries(&[fulfilled("channel-a", 60)]);
        assert_eq!(projection, Projection::default());
    }

    #[test]
    fn different_peers_are_tracked_independently() {
        let projection = Projection::from_entries(&[signed("peer-b", "channel-a", 1, 100)]);
        assert_eq!(projection.outbound_owed("peer-b"), 100);
        assert_eq!(projection.outbound_owed("peer-d"), 0);
    }

    #[test]
    fn latest_inbound_claim_is_none_before_any_claim_is_accepted() {
        let projection = Projection::from_entries(&[fulfilled("channel-a", 100)]);
        assert_eq!(projection.latest_inbound_claim("channel-a"), None);
    }

    #[test]
    fn latest_inbound_claim_reports_the_highest_nonce_claims_amount_and_signature() {
        let projection = Projection::from_entries(&[
            accepted_with_signature("channel-a", 1, 100, &[1, 2, 3]),
            accepted_with_signature("channel-a", 2, 150, &[4, 5, 6]),
        ]);
        assert_eq!(
            projection.latest_inbound_claim("channel-a"),
            Some((2, 150, vec![4, 5, 6]))
        );
    }

    fn arbitrary_entry() -> impl Strategy<Value = JournalEntry> {
        prop_oneof![
            (any::<u64>(), any::<u64>()).prop_map(|(nonce, amount)| signed(
                "peer-b",
                "channel-a",
                nonce,
                amount
            )),
            (
                any::<u64>(),
                any::<u64>(),
                proptest::collection::vec(any::<u8>(), 0..4)
            )
                .prop_map(|(nonce, amount, signature)| accepted_with_signature(
                    "channel-a",
                    nonce,
                    amount,
                    &signature
                )),
            any::<u64>().prop_map(|amount| fulfilled("channel-a", amount)),
        ]
    }

    proptest! {
        /// Issue #424's own acceptance criterion: a projection rebuilt from
        /// a journal equals the projection that produced it -- folding an
        /// arbitrary sequence of entries incrementally, one at a time as
        /// they would occur live, yields exactly the same projection as
        /// folding the same sequence all at once from scratch, the way a
        /// restart replays a journal.
        #[test]
        fn a_projection_rebuilt_from_a_journal_equals_the_projection_that_produced_it(
            entries in proptest::collection::vec(arbitrary_entry(), 0..64)
        ) {
            let mut incremental = Projection::default();
            for entry in &entries {
                incremental.apply(entry);
            }

            let rebuilt = Projection::from_entries(&entries);

            prop_assert_eq!(incremental, rebuilt);
        }

        /// Folding the same journal twice (e.g. a restart that replays an
        /// already-fully-applied journal once more) is idempotent: the
        /// owed figures it reports depend only on the journal content, not
        /// on how many times it has been rebuilt from.
        #[test]
        fn rebuilding_the_same_journal_twice_yields_the_same_projection(
            entries in proptest::collection::vec(arbitrary_entry(), 0..64)
        ) {
            let first = Projection::from_entries(&entries);
            let second = Projection::from_entries(&entries);
            prop_assert_eq!(first, second);
        }
    }
}
