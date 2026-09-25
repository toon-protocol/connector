//! solana-foundation `payment-channels`' own wire, spoken directly: the
//! channel account's layout, its PDA, the distribution commitment and the
//! instructions a batch-settlement channel's lifecycle needs (ADR 0074).
//!
//! Pinned at payment-channels `3ffa4d67` (ADR 0074's _Sources_), cited as
//! **PC** plus a path under `program/payment_channels/src/`. That program
//! ships a generated client, but it builds against a newer Solana SDK line
//! than this workspace pins, so -- exactly as [`crate::wire`] does for TOON's
//! own program -- the bytes are written here and pinned by this module's
//! tests and by the tier-3 tests that run them against the deployed binary.
//!
//! Nothing here signs. An instruction builder names the accounts that must
//! sign; whoever submits the transaction supplies the signatures. That is
//! what lets the sponsor endpoint (issue #1346) co-sign an `open` the client
//! built with [`OpenChannel::instruction`].

use solana_sdk::hash::hashv;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;

/// payment-channels' program id, the same on devnet and mainnet-beta (ADR
/// 0074 _Sources_). The program is compiled against this id (`crate::ID`),
/// so a local validator must load it here too.
pub const PAYMENT_CHANNELS_PROGRAM_ID: &str = "CHNLxYvVA28MJP9PrFuDXccuoGXAx7jBacfLEkahyGsX";

/// Instruction discriminators, one byte each (PC `instructions/mod.rs`,
/// `PaymentChannelsInstruction`).
pub const OPEN: u8 = 1;
pub const SETTLE: u8 = 2;
pub const TOP_UP: u8 = 3;
pub const SETTLE_AND_SEAL: u8 = 4;
pub const REQUEST_CLOSE: u8 = 5;
pub const SEAL: u8 = 6;
pub const DISTRIBUTE: u8 = 7;
pub const RECLAIM: u8 = 9;

/// The slot window `reclaim` waits out: a `Distributed` channel's rent can
/// come home only once `clock.slot > open_slot + OPEN_SLOT_WINDOW` (PC
/// `constants.rs`, `OPEN_SLOT_WINDOW`). The program may only ever lower it.
pub const OPEN_SLOT_WINDOW: u64 = 1_500;

/// The owner of the treasury token account a sealed `distribute` sweeps
/// rounding residue into, as the mainnet-beta build fixes it (PC
/// `constants.rs`, `TREASURY_OWNER`, feature `mainnet-beta`). The program
/// checks the account passed is `ATA(TREASURY_OWNER, mint, token_program)`,
/// so a caller must name the one its deployment was built with.
pub const TREASURY_OWNER_MAINNET: &str = "Cs2zdfUNonRdRGsiZUQQLdTxzxVvJZmgiX2mpLYKuEqP";

/// The placeholder `TREASURY_OWNER` a build without `mainnet-beta` ships
/// with, `0xBE 0xEF` sixteen times (PC `constants.rs`,
/// `TREASURY_OWNER_SENTINEL`) -- which the devnet deployment carries (see
/// `test_support::payment_channels_fixture`).
pub const TREASURY_OWNER_PLACEHOLDER: [u8; 32] = [
    0xBE, 0xEF, 0xBE, 0xEF, 0xBE, 0xEF, 0xBE, 0xEF, 0xBE, 0xEF, 0xBE, 0xEF, 0xBE, 0xEF, 0xBE, 0xEF,
    0xBE, 0xEF, 0xBE, 0xEF, 0xBE, 0xEF, 0xBE, 0xEF, 0xBE, 0xEF, 0xBE, 0xEF, 0xBE, 0xEF, 0xBE, 0xEF,
];

/// Every `TREASURY_OWNER` a deployment of the pinned program is known to be
/// built with, in the order a caller should try them: the program id is the
/// same on every cluster, so which one a deployment holds is not derivable
/// from anything a caller has.
pub fn treasury_owner_candidates() -> [Pubkey; 2] {
    [
        TREASURY_OWNER_MAINNET
            .parse()
            .expect("a literal base58 address"),
        Pubkey::new_from_array(TREASURY_OWNER_PLACEHOLDER),
    ]
}

/// The channel PDA's seed prefix (PC `state/channel.rs`, `CHANNEL_SEED`).
pub const CHANNEL_SEED: &[u8] = b"channel";
/// The self-CPI event signer's seed (PC `event_engine.rs`,
/// `EVENT_AUTHORITY_SEED`). `open` takes the PDA it derives.
pub const EVENT_AUTHORITY_SEED: &[u8] = b"event_authority";

/// A distribution share's denominator: 10000 bps is everything (PC
/// `constants.rs`, `BPS_DENOMINATOR`).
pub const BPS_DENOMINATOR: u16 = 10_000;

/// The byte-0 tag of a `Channel` account (PC `state/common.rs`,
/// `AccountDiscriminator::Channel`).
pub const CHANNEL_DISCRIMINATOR: u8 = 1;
/// The only layout version this module reads (PC `state/common.rs`,
/// `CURRENT_CHANNEL_VERSION`). The program itself refuses any other on
/// load, so a later layout is not guessed at here either.
pub const CHANNEL_VERSION: u8 = 1;

/// A `Channel` account is exactly 256 bytes (PC `state/channel.rs#L81-L156`,
/// `const _: () = assert!(Channel::LEN == 256)`). Also the `dataSize` filter
/// a sponsor's `getProgramAccounts` rediscovery uses (ADR 0074 decision 5).
pub const CHANNEL_ACCOUNT_LEN: usize = 256;

// Field offsets inside the 256-byte `repr(C)`, alignment-1 `Channel`.
const STATUS_OFFSET: usize = 3;
const SALT_OFFSET: usize = 4;
const DEPOSIT_OFFSET: usize = 12;
const SETTLED_OFFSET: usize = 20;
const PAYOUT_WATERMARK_OFFSET: usize = 28;
const CLOSURE_STARTED_AT_OFFSET: usize = 36;
const PAYER_WITHDRAWN_AT_OFFSET: usize = 44;
const GRACE_PERIOD_OFFSET: usize = 52;
const DISTRIBUTION_HASH_OFFSET: usize = 56;
const PAYER_OFFSET: usize = 88;
/// `payee`: a PDA seed, and the only signer of `settle_and_seal`.
pub const PAYEE_OFFSET: usize = 120;
/// `authorized_signer`: the key every voucher on the channel is signed by.
pub const AUTHORIZED_SIGNER_OFFSET: usize = 152;
const MINT_OFFSET: usize = 184;
/// `rent_payer`: the `memcmp` offset a sponsor's `getProgramAccounts`
/// rediscovery filters on (ADR 0074 decision 5).
pub const RENT_PAYER_OFFSET: usize = 216;
const OPEN_SLOT_OFFSET: usize = 248;

/// A channel's position in the program's state machine (PC
/// `state/channel.rs`, `ChannelStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelStatus {
    Open,
    /// The watermark is locked; `distribute` pays out what it holds.
    Sealed,
    /// The payer called `request_close`. Only the payee's
    /// `settle_and_seal` can still land a voucher, and only before
    /// `closure_started_at + grace_period`.
    Closing,
    /// Terminal: paid out and drained, awaiting `reclaim`.
    Distributed,
}

impl ChannelStatus {
    fn from_byte(byte: u8) -> Option<ChannelStatus> {
        match byte {
            0 => Some(ChannelStatus::Open),
            1 => Some(ChannelStatus::Sealed),
            2 => Some(ChannelStatus::Closing),
            3 => Some(ChannelStatus::Distributed),
            _ => None,
        }
    }
}

/// A payment-channels `Channel` account, decoded. Every field the program
/// stores, so a caller can re-derive the PDA from the account's own seeds
/// ([`derive_address`](Self::derive_address)) before trusting any of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelAccount {
    pub bump: u8,
    pub status: ChannelStatus,
    pub salt: u64,
    /// The escrow, and the ceiling on `settled`. Raised only by `top_up`,
    /// and only while Open.
    pub deposit: u64,
    /// The cumulative amount landed by vouchers: the watermark a new
    /// voucher must strictly exceed.
    pub settled: u64,
    pub payout_watermark: u64,
    pub closure_started_at: i64,
    pub payer_withdrawn_at: i64,
    pub grace_period: u32,
    pub distribution_hash: [u8; 32],
    pub payer: Pubkey,
    pub payee: Pubkey,
    pub authorized_signer: Pubkey,
    pub mint: Pubkey,
    pub rent_payer: Pubkey,
    pub open_slot: u64,
}

impl ChannelAccount {
    /// Decode `data`, or `None` if it is not a `Channel` this module reads:
    /// the wrong length, the wrong discriminator, another layout version, or
    /// a status byte the program never writes. The same header checks the
    /// program makes on load (PC `state/channel.rs`, `validate_header`).
    pub fn parse(data: &[u8]) -> Option<ChannelAccount> {
        if data.len() != CHANNEL_ACCOUNT_LEN
            || data[0] != CHANNEL_DISCRIMINATOR
            || data[1] != CHANNEL_VERSION
        {
            return None;
        }
        Some(ChannelAccount {
            bump: data[2],
            status: ChannelStatus::from_byte(data[STATUS_OFFSET])?,
            salt: read_u64(data, SALT_OFFSET),
            deposit: read_u64(data, DEPOSIT_OFFSET),
            settled: read_u64(data, SETTLED_OFFSET),
            payout_watermark: read_u64(data, PAYOUT_WATERMARK_OFFSET),
            closure_started_at: read_u64(data, CLOSURE_STARTED_AT_OFFSET) as i64,
            payer_withdrawn_at: read_u64(data, PAYER_WITHDRAWN_AT_OFFSET) as i64,
            grace_period: u32::from_le_bytes(
                data[GRACE_PERIOD_OFFSET..GRACE_PERIOD_OFFSET + 4]
                    .try_into()
                    .expect("four bytes"),
            ),
            distribution_hash: data[DISTRIBUTION_HASH_OFFSET..DISTRIBUTION_HASH_OFFSET + 32]
                .try_into()
                .expect("32 bytes"),
            payer: read_pubkey(data, PAYER_OFFSET),
            payee: read_pubkey(data, PAYEE_OFFSET),
            authorized_signer: read_pubkey(data, AUTHORIZED_SIGNER_OFFSET),
            mint: read_pubkey(data, MINT_OFFSET),
            rent_payer: read_pubkey(data, RENT_PAYER_OFFSET),
            open_slot: read_u64(data, OPEN_SLOT_OFFSET),
        })
    }

    /// The address this account's own seed fields derive under
    /// `program_id` ([`channel_pda`]). An account living anywhere else is not
    /// the channel it describes, whatever its bytes say (X402 SVM spec
    /// `#L1379-L1385`).
    pub fn derive_address(&self, program_id: &Pubkey) -> Pubkey {
        channel_pda(
            &self.payer,
            &self.payee,
            &self.mint,
            &self.authorized_signer,
            self.salt,
            self.open_slot,
            program_id,
        )
        .0
    }
}

fn read_u64(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(data[offset..offset + 8].try_into().expect("eight bytes"))
}

fn read_pubkey(data: &[u8], offset: usize) -> Pubkey {
    Pubkey::new_from_array(data[offset..offset + 32].try_into().expect("32 bytes"))
}

/// The channel PDA: `[b"channel", payer, payee, mint, authorized_signer,
/// salt LE, open_slot LE]` (PC `state/channel.rs`, `Channel::find_pda`).
/// `open_slot` is a seed, so the address is per incarnation and cannot be
/// predicted before the client picks its slot.
pub fn channel_pda(
    payer: &Pubkey,
    payee: &Pubkey,
    mint: &Pubkey,
    authorized_signer: &Pubkey,
    salt: u64,
    open_slot: u64,
    program_id: &Pubkey,
) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            CHANNEL_SEED,
            payer.as_ref(),
            payee.as_ref(),
            mint.as_ref(),
            authorized_signer.as_ref(),
            &salt.to_le_bytes(),
            &open_slot.to_le_bytes(),
        ],
        program_id,
    )
}

/// The PDA that signs the program's self-CPI events, which `open` names.
pub fn event_authority_pda(program_id: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[EVENT_AUTHORITY_SEED], program_id).0
}

/// The channel's escrow: its canonical associated token account.
pub fn escrow_token_account(channel: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    spl_associated_token_account::get_associated_token_address_with_program_id(
        channel,
        mint,
        token_program,
    )
}

/// One distribution entry: a recipient **wallet** (its canonical ATA is
/// what `distribute` pays) and its share in bps (PC
/// `instructions/helpers/distribution.rs`, `DistributionEntry`, 34 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DistributionEntry {
    pub recipient: Pubkey,
    pub bps: u16,
}

/// The distribution preimage `open` takes and `distribute` re-presents:
/// `count u32 LE ‖ (recipient ‖ bps u16 LE) × count` (PC
/// `instructions/helpers/distribution.rs`, `DistributionPreimage`).
pub fn distribution_preimage(entries: &[DistributionEntry]) -> Vec<u8> {
    let mut preimage = Vec::with_capacity(4 + entries.len() * 34);
    preimage.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for entry in entries {
        preimage.extend_from_slice(entry.recipient.as_ref());
        preimage.extend_from_slice(&entry.bps.to_le_bytes());
    }
    preimage
}

/// `distribution_hash`: SHA-256 of [`distribution_preimage`], which `open`
/// stores and `distribute` checks.
pub fn distribution_hash(entries: &[DistributionEntry]) -> [u8; 32] {
    hashv(&[&distribution_preimage(entries)]).to_bytes()
}

/// The one distribution ADR 0074 decision 2 admits: everything to
/// `receiver`, as a single explicit 10000 bps entry, leaving the payee's
/// implicit remainder at zero. x402 requires the explicit entry even when
/// the receiver is the payee (X402 SVM spec `#L199-L206`).
pub fn sole_recipient(receiver: &Pubkey) -> [DistributionEntry; 1] {
    [DistributionEntry {
        recipient: *receiver,
        bps: BPS_DENOMINATOR,
    }]
}

/// A client's `open`: everything the instruction carries (PC
/// `instructions/open.rs`). Built by the client; submitted with the
/// payer's and the rent payer's signatures, which under ADR 0074 is the
/// client and this node's sponsor key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenChannel {
    /// Funds the deposit and signs; the refund destination.
    pub payer: Pubkey,
    /// Funds the PDA and escrow rent, and signs. This node's sponsor key.
    pub rent_payer: Pubkey,
    /// A PDA seed and the only signer of `settle_and_seal`. This node's
    /// sponsor key.
    pub payee: Pubkey,
    pub mint: Pubkey,
    /// The SPL Token or Token-2022 program that owns `mint`.
    pub token_program: Pubkey,
    /// The key that signs vouchers; the payer's or a session key.
    pub authorized_signer: Pubkey,
    pub salt: u64,
    pub deposit: u64,
    /// Seconds. The program's only bound is `>= 1`.
    pub grace_period: u32,
    /// The client's chosen epoch: at most the current slot and at most
    /// 1,500 slots behind it when `open` executes (PC `constants.rs`,
    /// `OPEN_SLOT_WINDOW`).
    pub open_slot: u64,
    pub recipients: Vec<DistributionEntry>,
}

impl OpenChannel {
    /// The channel account this `open` creates.
    pub fn channel(&self, program_id: &Pubkey) -> Pubkey {
        channel_pda(
            &self.payer,
            &self.payee,
            &self.mint,
            &self.authorized_signer,
            self.salt,
            self.open_slot,
            program_id,
        )
        .0
    }

    /// The `open` instruction: data `0x01 ‖ salt u64 ‖ deposit u64 ‖
    /// grace_period u32 ‖ open_slot u64 ‖ preimage`, and the fourteen
    /// accounts in the program's order.
    pub fn instruction(&self, program_id: &Pubkey) -> Instruction {
        let channel = self.channel(program_id);
        let mut data = Vec::with_capacity(1 + 28 + 4 + self.recipients.len() * 34);
        data.push(OPEN);
        data.extend_from_slice(&self.salt.to_le_bytes());
        data.extend_from_slice(&self.deposit.to_le_bytes());
        data.extend_from_slice(&self.grace_period.to_le_bytes());
        data.extend_from_slice(&self.open_slot.to_le_bytes());
        data.extend_from_slice(&distribution_preimage(&self.recipients));
        Instruction {
            program_id: *program_id,
            accounts: vec![
                AccountMeta::new(self.payer, true),
                AccountMeta::new(self.rent_payer, true),
                AccountMeta::new_readonly(self.payee, false),
                AccountMeta::new_readonly(self.mint, false),
                AccountMeta::new_readonly(self.authorized_signer, false),
                AccountMeta::new(channel, false),
                AccountMeta::new(
                    spl_associated_token_account::get_associated_token_address_with_program_id(
                        &self.payer,
                        &self.mint,
                        &self.token_program,
                    ),
                    false,
                ),
                AccountMeta::new(
                    escrow_token_account(&channel, &self.mint, &self.token_program),
                    false,
                ),
                AccountMeta::new_readonly(self.token_program, false),
                AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
                AccountMeta::new_readonly(solana_sdk::sysvar::rent::id(), false),
                AccountMeta::new_readonly(spl_associated_token_account::id(), false),
                AccountMeta::new_readonly(event_authority_pda(program_id), false),
                AccountMeta::new_readonly(*program_id, false),
            ],
            data,
        }
    }
}

/// The payer's `top_up`: `amount` more into the escrow, only while Open
/// (PC `instructions/top_up.rs`). Signed by the payer.
pub fn top_up_instruction(
    program_id: &Pubkey,
    payer: &Pubkey,
    channel: &Pubkey,
    mint: &Pubkey,
    token_program: &Pubkey,
    amount: u64,
) -> Instruction {
    let mut data = Vec::with_capacity(9);
    data.push(TOP_UP);
    data.extend_from_slice(&amount.to_le_bytes());
    Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(*channel, false),
            AccountMeta::new(
                spl_associated_token_account::get_associated_token_address_with_program_id(
                    payer,
                    mint,
                    token_program,
                ),
                false,
            ),
            AccountMeta::new(escrow_token_account(channel, mint, token_program), false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(*token_program, false),
        ],
        data,
    }
}

/// The payer's `request_close`: Open to Closing, starting the grace period
/// (PC `instructions/request_close.rs`). Signed by the payer.
pub fn request_close_instruction(
    program_id: &Pubkey,
    payer: &Pubkey,
    channel: &Pubkey,
) -> Instruction {
    Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new_readonly(*payer, true),
            AccountMeta::new(*channel, false),
        ],
        data: vec![REQUEST_CLOSE],
    }
}

/// The Ed25519 precompile instruction that carries a voucher, in the one
/// layout the program parses: a single signature, every offset inline and
/// canonical, the 50-byte voucher as its message -- 162 bytes (PC
/// `instructions/helpers/ed25519`). It must sit at `current_index − 1`,
/// immediately before the `settle` or `settle_and_seal` it authorises (PC
/// `instructions/helpers/voucher.rs`, `verify_voucher`).
///
/// `expires_at` is always zero: ADR 0074 decision 3 refuses any other
/// value before a voucher is accepted.
pub fn voucher_verify_instruction(
    channel: &Pubkey,
    authorized_signer: &Pubkey,
    cumulative_amount: u64,
    signature: &[u8; 64],
) -> Instruction {
    let message =
        connector_signer::solana_voucher_message(&channel.to_bytes(), cumulative_amount, 0);
    crate::wire::ed25519_verify_instruction(authorized_signer, signature, &message)
}

/// `settle`: advance `settled` to the voucher's amount, only while Open. A
/// permissionless crank: the voucher is its authority, so any fee payer may
/// submit it. Returns the precompile instruction and `settle`, in the order
/// the transaction must carry them.
pub fn settle_instructions(
    program_id: &Pubkey,
    channel: &Pubkey,
    authorized_signer: &Pubkey,
    cumulative_amount: u64,
    signature: &[u8; 64],
) -> [Instruction; 2] {
    [
        voucher_verify_instruction(channel, authorized_signer, cumulative_amount, signature),
        Instruction {
            program_id: *program_id,
            accounts: vec![
                AccountMeta::new(*channel, false),
                AccountMeta::new_readonly(solana_sdk::sysvar::instructions::id(), false),
            ],
            data: vec![SETTLE],
        },
    ]
}

/// `settle_and_seal` with a voucher: land it and seal the channel (PC
/// `instructions/settle_and_seal.rs`). Signed by the payee. From Open it may
/// run at any time; from Closing only before `closure_started_at +
/// grace_period`. The one way to land a voucher once a payer has asked to
/// close.
pub fn settle_and_seal_instructions(
    program_id: &Pubkey,
    payee: &Pubkey,
    channel: &Pubkey,
    authorized_signer: &Pubkey,
    cumulative_amount: u64,
    signature: &[u8; 64],
) -> [Instruction; 2] {
    [
        voucher_verify_instruction(channel, authorized_signer, cumulative_amount, signature),
        Instruction {
            program_id: *program_id,
            accounts: vec![
                AccountMeta::new_readonly(*payee, true),
                AccountMeta::new(*channel, false),
                AccountMeta::new_readonly(solana_sdk::sysvar::instructions::id(), false),
            ],
            // `has_voucher` = 1: apply the voucher before sealing.
            data: vec![SETTLE_AND_SEAL, 1],
        },
    ]
}

/// `settle_and_seal` with no voucher: seal the channel at what is already
/// settled (PC `instructions/settle_and_seal.rs`, `has_voucher` = 0). Signed
/// by the payee. From Closing only before `closure_started_at +
/// grace_period`; what lets a payee with nothing more to land close a
/// channel its payer asked to close without waiting out the grace period.
pub fn seal_without_voucher_instruction(
    program_id: &Pubkey,
    payee: &Pubkey,
    channel: &Pubkey,
) -> Instruction {
    Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new_readonly(*payee, true),
            AccountMeta::new(*channel, false),
            AccountMeta::new_readonly(solana_sdk::sysvar::instructions::id(), false),
        ],
        data: vec![SETTLE_AND_SEAL, 0],
    }
}

/// `seal`: the permissionless crank that moves a Closing channel to Sealed
/// once its grace period has elapsed (PC `instructions/seal.rs`). What is
/// left when no voucher was landed in time.
pub fn seal_instruction(program_id: &Pubkey, channel: &Pubkey) -> Instruction {
    Instruction {
        program_id: *program_id,
        accounts: vec![AccountMeta::new(*channel, false)],
        data: vec![SEAL],
    }
}

/// `distribute`: the permissionless crank that pays a channel out (PC
/// `instructions/distribute.rs`). On a Sealed channel it pays every
/// recipient its share of `settled`, refunds the payer `deposit − settled`,
/// sweeps the rounding residue to the treasury, closes the escrow and then
/// either deallocates the channel or marks it Distributed for
/// [`reclaim_instruction`]; all rent goes to `rent_payer`.
///
/// `recipients` must be the preimage `open` committed to -- the program
/// rehashes it -- and each recipient's canonical ATA is appended, in order,
/// as a trailing account. `treasury_owner` must be the one the deployment
/// was built with (see [`treasury_owner_candidates`]).
pub fn distribute_instruction(
    program_id: &Pubkey,
    channel: &Pubkey,
    account: &ChannelAccount,
    token_program: &Pubkey,
    treasury_owner: &Pubkey,
    recipients: &[DistributionEntry],
) -> Instruction {
    let ata = |owner: &Pubkey| {
        spl_associated_token_account::get_associated_token_address_with_program_id(
            owner,
            &account.mint,
            token_program,
        )
    };
    let mut data = vec![DISTRIBUTE];
    data.extend_from_slice(&distribution_preimage(recipients));
    let mut accounts = vec![
        AccountMeta::new(*channel, false),
        AccountMeta::new(account.payer, false),
        AccountMeta::new(account.rent_payer, false),
        AccountMeta::new(
            escrow_token_account(channel, &account.mint, token_program),
            false,
        ),
        AccountMeta::new(ata(&account.payer), false),
        AccountMeta::new(ata(&account.payee), false),
        AccountMeta::new(ata(treasury_owner), false),
        AccountMeta::new_readonly(account.mint, false),
        AccountMeta::new_readonly(*token_program, false),
        AccountMeta::new_readonly(event_authority_pda(program_id), false),
        AccountMeta::new_readonly(*program_id, false),
    ];
    accounts.extend(
        recipients
            .iter()
            .map(|entry| AccountMeta::new(ata(&entry.recipient), false)),
    );
    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

/// `reclaim`: the permissionless crank that deallocates a Distributed
/// channel and returns its rent to `rent_payer`, once the clock's slot is
/// past `open_slot + OPEN_SLOT_WINDOW` (PC `instructions/reclaim.rs`). Two
/// accounts and no signer, so many fit one transaction.
pub fn reclaim_instruction(
    program_id: &Pubkey,
    channel: &Pubkey,
    rent_payer: &Pubkey,
) -> Instruction {
    Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(*channel, false),
            AccountMeta::new(*rent_payer, false),
        ],
        data: vec![RECLAIM],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn program_id() -> Pubkey {
        Pubkey::from_str(PAYMENT_CHANNELS_PROGRAM_ID).expect("base58")
    }

    /// A `Channel` laid out byte by byte from the program's own `repr(C)`
    /// field order (PC `state/channel.rs#L81-L156`), independently of
    /// [`ChannelAccount::parse`]'s offsets, which this then has to agree
    /// with.
    fn channel_bytes(status: u8) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(CHANNEL_ACCOUNT_LEN);
        bytes.extend_from_slice(&[1, 1, 254, status]); // discriminator, version, bump, status
        bytes.extend_from_slice(&7u64.to_le_bytes()); // salt
        bytes.extend_from_slice(&1_000u64.to_le_bytes()); // deposit
        bytes.extend_from_slice(&300u64.to_le_bytes()); // settled
        bytes.extend_from_slice(&100u64.to_le_bytes()); // payout_watermark
        bytes.extend_from_slice(&(-5i64).to_le_bytes()); // closure_started_at
        bytes.extend_from_slice(&9i64.to_le_bytes()); // payer_withdrawn_at
        bytes.extend_from_slice(&86_400u32.to_le_bytes()); // grace_period
        bytes.extend_from_slice(&[0x33; 32]); // distribution_hash
        for fill in [0x01, 0x02, 0x03, 0x04, 0x05] {
            // payer, payee, authorized_signer, mint, rent_payer
            bytes.extend_from_slice(&[fill; 32]);
        }
        bytes.extend_from_slice(&424_242u64.to_le_bytes()); // open_slot
        assert_eq!(bytes.len(), CHANNEL_ACCOUNT_LEN);
        bytes
    }

    #[test]
    fn a_channel_account_decodes_at_the_programs_offsets() {
        let account = ChannelAccount::parse(&channel_bytes(2)).expect("a channel");
        assert_eq!(
            account,
            ChannelAccount {
                bump: 254,
                status: ChannelStatus::Closing,
                salt: 7,
                deposit: 1_000,
                settled: 300,
                payout_watermark: 100,
                closure_started_at: -5,
                payer_withdrawn_at: 9,
                grace_period: 86_400,
                distribution_hash: [0x33; 32],
                payer: Pubkey::new_from_array([0x01; 32]),
                payee: Pubkey::new_from_array([0x02; 32]),
                authorized_signer: Pubkey::new_from_array([0x03; 32]),
                mint: Pubkey::new_from_array([0x04; 32]),
                rent_payer: Pubkey::new_from_array([0x05; 32]),
                open_slot: 424_242,
            }
        );
    }

    /// ADR 0074 and the ticket state three offsets outright; they are the
    /// ones a `getProgramAccounts` filter and a reader of the record lean on.
    #[test]
    fn the_offsets_the_record_names_are_the_ones_read() {
        assert_eq!(PAYEE_OFFSET, 120);
        assert_eq!(AUTHORIZED_SIGNER_OFFSET, 152);
        assert_eq!(RENT_PAYER_OFFSET, 216);
        let bytes = channel_bytes(0);
        assert_eq!(
            &bytes[RENT_PAYER_OFFSET..RENT_PAYER_OFFSET + 32],
            &[0x05; 32]
        );
    }

    #[test]
    fn anything_but_a_version_1_channel_is_not_decoded() {
        let good = channel_bytes(0);
        assert!(ChannelAccount::parse(&good).is_some());

        assert!(ChannelAccount::parse(&good[..255]).is_none(), "short");
        let mut long = good.clone();
        long.push(0);
        assert!(ChannelAccount::parse(&long).is_none(), "long");
        let mut tombstone = good.clone();
        tombstone[0] = 2; // AccountDiscriminator::ClosedChannel
        assert!(ChannelAccount::parse(&tombstone).is_none(), "discriminator");
        let mut later = good.clone();
        later[1] = 2;
        assert!(ChannelAccount::parse(&later).is_none(), "version");
        assert!(
            ChannelAccount::parse(&channel_bytes(4)).is_none(),
            "a status the program never writes"
        );
    }

    #[test]
    fn a_channel_derives_its_own_address_from_its_seed_fields() {
        let account = ChannelAccount::parse(&channel_bytes(0)).expect("a channel");
        let (expected, _) = Pubkey::find_program_address(
            &[
                b"channel",
                &[0x01; 32],
                &[0x02; 32],
                &[0x04; 32],
                &[0x03; 32],
                &7u64.to_le_bytes(),
                &424_242u64.to_le_bytes(),
            ],
            &program_id(),
        );
        assert_eq!(account.derive_address(&program_id()), expected);

        let mut other_slot = account.clone();
        other_slot.open_slot += 1;
        assert_ne!(
            other_slot.derive_address(&program_id()),
            expected,
            "open_slot is a seed"
        );
    }

    /// The single-recipient preimage is 38 bytes -- a count of one, the
    /// recipient, and 10000 as u16 LE -- and its hash is SHA-256 of exactly
    /// those bytes.
    #[test]
    fn the_sole_recipient_distribution_commits_to_one_entry_at_10000_bps() {
        let receiver = Pubkey::new_from_array([0x42; 32]);
        let preimage = distribution_preimage(&sole_recipient(&receiver));
        let mut expected = vec![1, 0, 0, 0];
        expected.extend_from_slice(&[0x42; 32]);
        expected.extend_from_slice(&[0x10, 0x27]);
        assert_eq!(preimage, expected);
        assert_eq!(
            distribution_hash(&sole_recipient(&receiver)),
            solana_sdk::hash::hash(&expected).to_bytes()
        );
    }

    #[test]
    fn open_carries_its_header_its_preimage_and_fourteen_accounts() {
        let open = OpenChannel {
            payer: Pubkey::new_unique(),
            rent_payer: Pubkey::new_unique(),
            payee: Pubkey::new_unique(),
            mint: Pubkey::new_unique(),
            token_program: spl_token::id(),
            authorized_signer: Pubkey::new_unique(),
            salt: 0x0102,
            deposit: 5_000,
            grace_period: 900,
            open_slot: 77,
            recipients: sole_recipient(&Pubkey::new_from_array([0x42; 32])).to_vec(),
        };
        let instruction = open.instruction(&program_id());
        let mut data = vec![OPEN];
        data.extend_from_slice(&0x0102u64.to_le_bytes());
        data.extend_from_slice(&5_000u64.to_le_bytes());
        data.extend_from_slice(&900u32.to_le_bytes());
        data.extend_from_slice(&77u64.to_le_bytes());
        data.extend_from_slice(&distribution_preimage(&open.recipients));
        assert_eq!(instruction.data, data);
        assert_eq!(instruction.accounts.len(), 14);
        let signers: Vec<Pubkey> = instruction
            .accounts
            .iter()
            .filter(|meta| meta.is_signer)
            .map(|meta| meta.pubkey)
            .collect();
        assert_eq!(signers, vec![open.payer, open.rent_payer]);
        assert_eq!(instruction.accounts[5].pubkey, open.channel(&program_id()));
        assert!(instruction.accounts[5].is_writable);
    }

    /// The canonical inline layout the program pins: one signature, offsets
    /// 16/48/112, a 50-byte message, 162 bytes in all, every instruction
    /// index `u16::MAX`.
    #[test]
    fn the_voucher_precompile_instruction_is_the_canonical_162_bytes() {
        let channel = Pubkey::new_from_array([0x07; 32]);
        let signer = Pubkey::new_from_array([0x42; 32]);
        let [verify, settle] =
            settle_instructions(&program_id(), &channel, &signer, 1_234, &[0x9a; 64]);
        assert_eq!(verify.program_id, solana_sdk::ed25519_program::id());
        assert_eq!(verify.data.len(), 162);
        assert_eq!(&verify.data[..2], &[1, 0]);
        assert_eq!(u16::from_le_bytes([verify.data[2], verify.data[3]]), 48);
        assert_eq!(u16::from_le_bytes([verify.data[6], verify.data[7]]), 16);
        assert_eq!(u16::from_le_bytes([verify.data[10], verify.data[11]]), 112);
        assert_eq!(u16::from_le_bytes([verify.data[12], verify.data[13]]), 50);
        for index in [4, 8, 14] {
            assert_eq!(&verify.data[index..index + 2], &[0xff, 0xff]);
        }
        assert_eq!(&verify.data[16..48], &[0x42; 32]);
        assert_eq!(&verify.data[48..112], &[0x9a; 64]);
        assert_eq!(
            &verify.data[112..],
            &connector_signer::solana_voucher_message(&[0x07; 32], 1_234, 0)
        );

        assert_eq!(settle.data, vec![SETTLE]);
        assert_eq!(settle.accounts.len(), 2);
        assert!(settle.accounts.iter().all(|meta| !meta.is_signer));
    }

    #[test]
    fn settle_and_seal_applies_its_voucher_and_is_signed_by_the_payee() {
        let payee = Pubkey::new_unique();
        let [_, seal] = settle_and_seal_instructions(
            &program_id(),
            &payee,
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            5,
            &[0; 64],
        );
        assert_eq!(seal.data, vec![SETTLE_AND_SEAL, 1]);
        assert_eq!(seal.accounts[0].pubkey, payee);
        assert!(seal.accounts[0].is_signer);
    }

    #[test]
    fn the_treasury_owners_are_the_builds_own() {
        let [mainnet, placeholder] = treasury_owner_candidates();
        assert_eq!(mainnet.to_string(), TREASURY_OWNER_MAINNET);
        assert_eq!(&placeholder.to_bytes()[..4], &[0xBE, 0xEF, 0xBE, 0xEF]);
    }

    #[test]
    fn the_seals_carry_their_discriminators_and_the_payee_signs_the_early_one() {
        let payee = Pubkey::new_unique();
        let channel = Pubkey::new_unique();
        let early = seal_without_voucher_instruction(&program_id(), &payee, &channel);
        assert_eq!(early.data, vec![SETTLE_AND_SEAL, 0]);
        assert!(early.accounts[0].is_signer);
        assert_eq!(early.accounts[0].pubkey, payee);

        let crank = seal_instruction(&program_id(), &channel);
        assert_eq!(crank.data, vec![SEAL]);
        assert_eq!(crank.accounts.len(), 1);
        assert!(crank.accounts.iter().all(|meta| !meta.is_signer));
    }

    /// `distribute`: its eleven fixed accounts in the program's order, then
    /// one ATA per recipient; the preimage as data; no signer at all.
    #[test]
    fn distribute_carries_the_preimage_eleven_accounts_and_the_recipient_tail() {
        let account = ChannelAccount::parse(&channel_bytes(1)).expect("a channel");
        let channel = Pubkey::new_unique();
        let treasury = Pubkey::new_unique();
        let receiver = Pubkey::new_from_array([0x42; 32]);
        let recipients = sole_recipient(&receiver);
        let instruction = distribute_instruction(
            &program_id(),
            &channel,
            &account,
            &spl_token::id(),
            &treasury,
            &recipients,
        );
        let mut data = vec![DISTRIBUTE];
        data.extend_from_slice(&distribution_preimage(&recipients));
        assert_eq!(instruction.data, data);
        assert_eq!(instruction.accounts.len(), 12);
        assert!(instruction.accounts.iter().all(|meta| !meta.is_signer));
        let ata = |owner: &Pubkey| {
            spl_associated_token_account::get_associated_token_address_with_program_id(
                owner,
                &account.mint,
                &spl_token::id(),
            )
        };
        let keys: Vec<Pubkey> = instruction
            .accounts
            .iter()
            .map(|meta| meta.pubkey)
            .collect();
        assert_eq!(
            keys,
            vec![
                channel,
                account.payer,
                account.rent_payer,
                escrow_token_account(&channel, &account.mint, &spl_token::id()),
                ata(&account.payer),
                ata(&account.payee),
                ata(&treasury),
                account.mint,
                spl_token::id(),
                event_authority_pda(&program_id()),
                program_id(),
                ata(&receiver),
            ]
        );
    }

    #[test]
    fn reclaim_names_the_channel_and_its_rent_payer_and_nobody_signs() {
        let channel = Pubkey::new_unique();
        let rent_payer = Pubkey::new_unique();
        let instruction = reclaim_instruction(&program_id(), &channel, &rent_payer);
        assert_eq!(instruction.data, vec![RECLAIM]);
        let keys: Vec<Pubkey> = instruction
            .accounts
            .iter()
            .map(|meta| meta.pubkey)
            .collect();
        assert_eq!(keys, vec![channel, rent_payer]);
        assert!(instruction
            .accounts
            .iter()
            .all(|meta| meta.is_writable && !meta.is_signer));
    }
}
