//! Issue #876 AC2: the two books a connector keeps after issue #873 stay
//! two books.
//!
//! `crates/connector-runtime/src/outbound_client.rs`'s header states the
//! rule as a table -- the INBOUND `ClaimBook` journal, where this node is
//! the authority on what it accepted, against the OUTBOUND client ledger,
//! whose authority is the RECEIVER, asked over `ClaimStateSource` every
//! time. This file is that rule as an executable claim, driven through the
//! public API only and with both books on disk in ONE `state_dir`, which is
//! the arrangement a deployed node actually has and the arrangement in
//! which a merge would go unnoticed.
//!
//! Three separations are asserted, each one a way the books could merge:
//!
//!   1. an inbound claim accepted at nonce 9 does not move the outbound
//!      client ledger's floor, and the next outbound claim's nonce and
//!      cumulative amount come from the RECEIVER (nonce 1, not 10; the
//!      amount, not the inbound cumulative);
//!   2. signing an outbound client claim appends nothing to the inbound
//!      journal -- its entries are unchanged, byte for byte;
//!   3. the outbound book REFUSES the journal's own file rather than
//!      reading `JournalEntry` lines as a nonce floor. This is the merge
//!      that could actually be configured by hand, and `open`'s refusal of
//!      an unreadable file is the only thing standing in front of it.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use connector_domain::JournalEntry;
use connector_runtime::{
    ChannelDomain, ClaimAckOutcome, ClaimBook, ClaimSignature, ClaimStateDomain, ClaimStateSource,
    ClaimWatermark, EvmDomain, FileJournal, Journal, OutboundClaimBinding, OutboundClientError,
    OutboundClientLedger, WireClaim,
};
use connector_signer::{
    derive_evm_address, evm_balance_proof_digest, EvmBalanceProof, LocalSigner, Signer,
};

/// The one channel both books are pointed at. Sharing the channel is
/// deliberate: two books keyed by different things (the journal by channel,
/// the outbound ledger by next hop) are trivially separate when they are
/// looking at different money, and the interesting case is when they are
/// looking at the same.
const CHANNEL_HEX: &str = "0x5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c";
const CHANNEL: [u8; 32] = [0x5cu8; 32];
const CHAIN_ID: u64 = 84_532;
const TOKEN_NETWORK: [u8; 20] = [0x1eu8; 20];
/// The next hop the outbound client ledger's nonce line belongs to -- a
/// real peer id from this fleet, so nothing here reads as a placeholder.
const NEXT_HOP: &str = "apex-store";

/// The nonce and cumulative amount of the INBOUND claim. Both are chosen to
/// be unmistakable if they ever leak into the outbound line: an outbound
/// nonce of 10 or an outbound cumulative of 5_000-and-up could only have
/// come from here.
const INBOUND_NONCE: u64 = 9;
const INBOUND_CUMULATIVE: u64 = 5_000;
const OUTBOUND_AMOUNT: u64 = 1_002;

/// A receiver that answers the outbound ledger's watermark question the way
/// a next hop with no record of this node's claims answers it: nonce 0,
/// nothing claimed, plenty of headroom.
///
/// A fake in ADR 0007's sense -- real behaviour, no network. `HttpClaimState`
/// against a live server is already covered by `outbound_client.rs`'s own
/// tests; what this file needs is control over the number the receiver
/// reports, so that the number the ledger uses is provably that one and not
/// the journal's.
struct ReceiverSaying {
    nonce: u64,
    cumulative: u128,
}

#[async_trait]
impl ClaimStateSource for ReceiverSaying {
    async fn watermark(
        &self,
        _channel: &[u8; 32],
        _domain: &ClaimStateDomain,
    ) -> Result<ClaimWatermark, OutboundClientError> {
        Ok(ClaimWatermark {
            nonce: self.nonce,
            cumulative: self.cumulative,
            available: Some(1_000_000),
        })
    }
}

/// Sign a claim on [`CHANNEL`] exactly as a peer signs one
/// would, so `accept_inbound` verifies a real signature rather than being
/// handed a pre-accepted stub.
fn signed_inbound_claim(
    counterparty: &LocalSigner,
    nonce: u64,
    cumulative_amount: u64,
) -> WireClaim {
    let proof = EvmBalanceProof {
        channel_id: CHANNEL,
        nonce,
        transferred_amount: u128::from(cumulative_amount),
        locked_amount: 0,
        locks_root: [0u8; 32],
        chain_id: CHAIN_ID,
        token_network_address: TOKEN_NETWORK,
    };
    WireClaim {
        channel_id: CHANNEL_HEX.to_string(),
        nonce,
        cumulative_amount,
        signature: ClaimSignature::Evm(
            counterparty
                .sign(&evm_balance_proof_digest(&proof))
                .expect("sign the inbound claim"),
        ),
    }
}

/// A `ClaimBook` that will accept claims on [`CHANNEL`] from
/// `counterparty`, journaling to `path`.
fn book_journaling_to(
    counterparty: &LocalSigner,
    path: &std::path::Path,
) -> (ClaimBook, Arc<FileJournal>) {
    let address = derive_evm_address(&counterparty.public_key().expect("counterparty public key"));
    let mut counterparties = HashMap::new();
    counterparties.insert(CHANNEL_HEX.to_string(), address);
    let mut book = ClaimBook::new(None, HashMap::new(), counterparties);
    book.set_channel_domain(
        CHANNEL_HEX,
        ChannelDomain {
            chain_id: CHAIN_ID,
            token_network_address: TOKEN_NETWORK,
        },
    )
    .expect("the test channel id is a real bytes32");
    let journal = Arc::new(FileJournal::open(path).expect("open the inbound journal"));
    book.set_journal(Arc::clone(&journal) as Arc<dyn Journal>)
        .expect("an empty journal replays clean");
    (book, journal)
}

#[tokio::test]
async fn an_inbound_claim_and_an_outbound_client_claim_never_move_each_others_ledger() {
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let journal_path = state_dir.path().join("journal.ndjson");
    let ledger_path = state_dir.path().join("outbound-client.ndjson");

    let counterparty = LocalSigner::from_secret_bytes("inbound-counterparty", [11u8; 32])
        .expect("counterparty signer");
    let (book, journal) = book_journaling_to(&counterparty, &journal_path);
    let ledger = OutboundClientLedger::open(&ledger_path).expect("open the outbound client ledger");

    // ── the inbound side moves ───────────────────────────────────────────
    assert_eq!(
        book.accept_inbound(&signed_inbound_claim(
            &counterparty,
            INBOUND_NONCE,
            INBOUND_CUMULATIVE
        )),
        ClaimAckOutcome::Accepted,
        "the fixture claim has to actually be accepted, or the rest proves nothing"
    );
    let accepted = book
        .inbound_watermark(CHANNEL_HEX)
        .expect("the journal now holds an accepted claim");
    assert_eq!(accepted.nonce, INBOUND_NONCE);
    assert_eq!(accepted.cumulative_amount, INBOUND_CUMULATIVE);

    // ── ...and the outbound side did not ─────────────────────────────────
    assert_eq!(
        ledger.issued_nonce(NEXT_HOP),
        0,
        "an inbound claim is the receiver's business, not this node's nonce line: the outbound \
         client ledger must still have issued nothing"
    );

    // The receiver -- the ONLY authority on the outbound line -- says it
    // has no record of this node at all. The journal, on the same channel,
    // says nonce 9 / 5_000. The claim must follow the receiver.
    let signer =
        LocalSigner::from_secret_bytes("outbound-client", [23u8; 32]).expect("outbound signer");
    let journal_before = journal.read_all().expect("read the journal");
    let journal_bytes_before = std::fs::read(&journal_path).expect("read the journal file");

    let claim = ledger
        .next_claim(
            NEXT_HOP,
            &ReceiverSaying {
                nonce: 0,
                cumulative: 0,
            },
            &CHANNEL,
            &OutboundClaimBinding::Evm {
                domain: EvmDomain {
                    chain_id: CHAIN_ID,
                    token_network: TOKEN_NETWORK,
                },
                signer: &signer,
            },
            OUTBOUND_AMOUNT,
        )
        .await
        .expect("sign an outbound client claim");

    assert_eq!(
        claim.nonce,
        1,
        "the outbound nonce comes from the receiver (0) plus one -- a nonce of {} would mean the \
         inbound journal's watermark had leaked into the outbound line",
        INBOUND_NONCE + 1
    );
    assert_eq!(
        claim.cumulative,
        u128::from(OUTBOUND_AMOUNT),
        "the outbound cumulative amount comes from the receiver (0) plus the packet -- anything \
         at or above {INBOUND_CUMULATIVE} could only have come from the inbound journal"
    );
    assert_eq!(ledger.issued_nonce(NEXT_HOP), 1);

    // ── ...and the reverse: the outbound claim touched no journal ────────
    assert_eq!(
        journal.read_all().expect(
            "the journal must still decode -- an unreadable one means something that is \
                     not a JournalEntry was written into it"
        ),
        journal_before,
        "signing an outbound client claim must append no JournalEntry -- the outbound book is not \
         a journal stream and nothing replaying the journal would understand one"
    );
    assert_eq!(
        std::fs::read(&journal_path).expect("re-read the journal file"),
        journal_bytes_before,
        "the inbound journal file must be byte-identical after an outbound claim"
    );
    assert_eq!(
        book.inbound_watermark(CHANNEL_HEX)
            .expect("the accepted claim is still there")
            .nonce,
        INBOUND_NONCE,
        "the inbound watermark on this very channel must be untouched by the outbound claim"
    );

    // ── and the two files are two files ──────────────────────────────────
    let ledger_bytes = std::fs::read(&ledger_path).expect("read the ledger file");
    let ledger_text = String::from_utf8(ledger_bytes).expect("the ledger file is utf-8");
    assert!(
        ledger_text.contains("\"nextHop\""),
        "the outbound ledger records issued nonces keyed by next hop: {ledger_text}"
    );
    for entry in journal_before {
        assert!(
            matches!(entry, JournalEntry::InboundClaimAccepted { .. }),
            "the journal holds inbound entries only"
        );
    }
    let journal_text = String::from_utf8(journal_bytes_before).expect("the journal file is utf-8");
    assert!(
        !journal_text.contains("nextHop"),
        "no outbound-ledger record may have been written into the journal: {journal_text}"
    );
}

/// The merge an operator could actually configure: pointing the outbound
/// client ledger at the inbound journal's file.
///
/// `OutboundClientLedger::open`'s doc says "`path` must not be either
/// journal file" -- this is that sentence with teeth. A `JournalEntry` line
/// is not an `IssuedNonce` line, so `open` must REFUSE the file outright.
/// The dangerous alternative is not a crash but a shrug: skip what it
/// cannot parse, report a floor of 0, and hand out nonce 1 on a line the
/// node has been paying for months.
#[test]
fn the_outbound_client_ledger_refuses_to_read_the_inbound_journals_file() {
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let journal_path = state_dir.path().join("journal.ndjson");
    let journal = FileJournal::open(&journal_path).expect("open the inbound journal");
    journal
        .append(&JournalEntry::InboundClaimAccepted {
            channel_id: CHANNEL_HEX.to_string(),
            nonce: INBOUND_NONCE,
            cumulative_amount: INBOUND_CUMULATIVE,
            signature: vec![0u8; 65],
        })
        .expect("journal the inbound claim");

    let opened = OutboundClientLedger::open(&journal_path);

    assert!(
        matches!(opened, Err(OutboundClientError::LedgerUnwritable { .. })),
        "the outbound book must refuse the inbound journal's file rather than read it as an empty \
         or merged ledger -- silently skipping those lines would report a nonce floor of 0 on a \
         line that is not 0"
    );
}
