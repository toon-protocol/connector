//! The Solana implementation of both halves of the batch-settlement port
//! (ADR 0074 decisions 2, 5 and 9; ADR 0075 decisions 2 and 3; issues #1343
//! and #1375): x402 `batch-settlement` channels on solana-foundation's
//! `payment-channels` program.
//!
//! - **Receiving** ([`BatchSettlementBackend`]): a payer opens a channel
//!   toward this node, this node's sponsor key co-signs it, and this node
//!   admits, reads and lands vouchers on it.
//! - **Paying** ([`BatchSettlementPayer`](connector_settlement::batch::BatchSettlementPayer),
//!   the `pay` module): this node opens a channel toward a counterparty by
//!   posting a payer-signed `open` to the counterparty's sponsor endpoint,
//!   tops it up, signs vouchers on it and winds it down with
//!   `request_close` and `distribute`.
//!
//! It uses this crate's transaction submission and confirm loop, the
//! table's one [`RpcTransport`] (ADR 0073), and the voucher message and
//! verifier in `connector-signer` (issue #1341).
//!
//! Watching for Closing, `distribute`, `reclaim` and `getProgramAccounts`
//! rediscovery are [`SolanaBatchWatcher`]'s (issue #1344, the `sweep`
//! module).
//!
//! **What is not here.** The public sponsor
//! endpoint is issue #1346's, and builds on [`wire::OpenChannel`]. The
//! runtime builds this backend from every `[settlement.solana]` table and
//! hands it to the client edge's claim gate
//! (`connector-cli`'s `batch_settlement` module).

mod pay;
pub mod sponsor;
mod sweep;
pub mod wire;

pub use sweep::{
    next_step, SolanaBatchWatcher, SponsoredChannel, Step, CLOSING_WATCH_INTERVAL,
    OPEN_SETTLE_INTERVAL,
};

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::{Mutex, MutexGuard, OnceLock};

use async_trait::async_trait;
use connector_chain_rpc::{retry_read, solana::rpc_client, RpcTransport};
use connector_settlement::batch::{
    AdmissionRefusal, BatchChannelState, BatchChannelStatus, BatchSettlementBackend,
    BatchSettlementError, ChannelPresentation, Voucher, VoucherSigner,
};
use connector_settlement::ChannelId;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_rpc_client::rpc_client::RpcClientConfig;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::instruction::Instruction;
use solana_sdk::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::transaction::Transaction;

use crate::submit::{send_and_confirm, ConfirmPolicy};

/// The chain this backend answers for, in the port's spelling.
const CHAIN: &str = "solana";

/// Both halves of the batch-settlement port over a real `payment-channels`
/// deployment: a [`BatchSettlementBackend`] for the channels this node
/// receives on, and a [`BatchSettlementPayer`](connector_settlement::batch::BatchSettlementPayer)
/// for the ones it pays on (the `pay` module).
///
/// **One key, three seats as receiver.** The sponsor key is this node's
/// `[settlement.solana]` settlement key. An admitted channel must name it as
/// `payee` and as `rent_payer` (ADR 0074 decision 5: the only configuration
/// in which this node can always land its latest voucher), and its
/// distribution must send everything to the **receiver** -- the owner of
/// this node's receiving account, which is the same key, since the
/// settlement table names no other. The receiver is what the greeting
/// publishes as `payTo` (decision 8). As payer, the same key is `payer` and
/// `authorized_signer` of every channel this node opens (ADR 0075 decision
/// 3).
///
/// Holds no channel ledger on the receiving side. Every answer is read from
/// the chain; the only local memory is which channels have been admitted or
/// restored, because the port requires one of the two before
/// [`channel_state`](BatchSettlementBackend::channel_state) and
/// [`land`](BatchSettlementBackend::land). The paying side remembers what it
/// opened and signed, for the process lifetime.
pub struct SolanaBatchSettlement {
    rpc: RpcClient,
    confirm: ConfirmPolicy,
    program_id: Pubkey,
    sponsor: Keypair,
    mint: Pubkey,
    min_grace_period_secs: u64,
    /// The smallest opening deposit the sponsor co-signs an `open` for:
    /// bounds only the public sponsor endpoint, and is published beside
    /// `min_grace_period_secs` (ADR 0074 decision 5).
    min_sponsored_deposit: u64,
    /// The chain's own genesis hash, read at connect: what
    /// [`Self::caip2_network`] names the network by.
    genesis_hash: solana_sdk::hash::Hash,
    admitted: Mutex<HashSet<Pubkey>>,
    /// The cluster's Rent sysvar, read on the first sponsored `open`
    /// ([`sponsor`]'s rent check, issue #1356) or the first `open` this node
    /// pays for.
    cluster_rent: OnceLock<solana_sdk::rent::Rent>,
    /// The channels this node opened as payer, and what it has signed on
    /// each: the paying half's memory, for the process lifetime (ADR 0075
    /// decision 2).
    outbound: Mutex<HashMap<Pubkey, pay::Outbound>>,
    /// Posts a payer-signed `open` to a counterparty's sponsor endpoint --
    /// direct, or through `socks_proxy` for an onion sponsor (ADR 0070). Not
    /// the settlement `rpc_url`'s transport: the endpoint is a peer's, not
    /// the chain's (ADR 0073 governs only the latter).
    sponsor_http: pay::SponsorClients,
    /// The treasury owner this deployment's `distribute` accepted, once one
    /// has, for the paying half's own distributions.
    treasury: Mutex<Option<Pubkey>>,
}

impl SolanaBatchSettlement {
    /// Bind to the `payment-channels` program at the one id the record
    /// fixes ([`wire::PAYMENT_CHANNELS_PROGRAM_ID`]), admitting channels in
    /// `mint` (`[settlement.solana] token_address`) whose `grace_period` is
    /// at least `min_grace_period_secs`, under the sponsor key
    /// `sponsor_seed` derives (the `[settlement.solana]` key file's 32-byte
    /// ed25519 seed), sponsoring an `open` only for a deposit of at least
    /// `min_sponsored_deposit`.
    ///
    /// Refuses, in order:
    ///
    /// * [`BatchSettlementError::NotDeployed`] when no account lives at that
    ///   id, and by name when the account there is not an executable
    ///   program: a chain `payment-channels` has not deployed to (ADR 0075
    ///   decision 1);
    /// * a `mint` the SPL Token program does not own (Token-2022 stays
    ///   refused, ADR 0075 decision 1), or whose own `decimals` disagree with
    ///   `expected_decimals` (issue #564): nothing scales by it, so it is
    ///   checked rather than applied;
    /// * a sponsor key holding no lamports, which could pay for nothing.
    ///
    /// It also reads the chain's genesis hash, which names the network the
    /// greeting publishes ([`Self::caip2_network`], issue #1131). Every read
    /// is retried with backoff before it fails the node (ADR 0073 decision
    /// 5).
    pub async fn connect(
        transport: &RpcTransport,
        sponsor_seed: &[u8; 32],
        mint: Pubkey,
        expected_decimals: u8,
        min_grace_period_secs: u64,
        min_sponsored_deposit: u64,
    ) -> Result<Self, BatchSettlementError> {
        let program_id = Pubkey::from_str(wire::PAYMENT_CHANNELS_PROGRAM_ID)
            .expect("PAYMENT_CHANNELS_PROGRAM_ID is a base58 program id");
        let sponsor =
            solana_sdk::signer::keypair::keypair_from_seed(sponsor_seed).map_err(backend_error)?;
        let rpc = rpc_client(
            transport,
            RpcClientConfig::with_commitment(CommitmentConfig::confirmed()),
        );
        let commitment = CommitmentConfig::confirmed();
        let program = retry_read(|| rpc.get_account_with_commitment(&program_id, commitment))
            .await
            .map_err(|error| {
                BatchSettlementError::Backend(format!(
                    "payment-channels ({program_id}) could not be read: {error}"
                ))
            })?
            .value;
        let Some(program) = program else {
            return Err(BatchSettlementError::NotDeployed(format!(
                "payment-channels ({program_id})"
            )));
        };
        if !program.executable {
            return Err(BatchSettlementError::NotDeployed(format!(
                "payment-channels ({program_id}): the account there is not an executable program"
            )));
        }
        let mint_account = retry_read(|| rpc.get_account(&mint))
            .await
            .map_err(|error| {
                BatchSettlementError::Backend(format!(
                    "[settlement.solana] token_address {mint} could not be read: {error}"
                ))
            })?;
        if mint_account.owner != spl_token::id() {
            return Err(BatchSettlementError::Backend(format!(
                "[settlement.solana] token_address {mint} is not owned by the SPL Token program"
            )));
        }
        let decimals = spl_token::state::Mint::unpack(&mint_account.data)
            .map_err(backend_error)?
            .decimals;
        if decimals != expected_decimals {
            return Err(BatchSettlementError::Backend(format!(
                "[settlement.solana] decimals is {expected_decimals}, but mint {mint} reports \
                 decimals = {decimals}"
            )));
        }
        let genesis_hash = retry_read(|| rpc.get_genesis_hash())
            .await
            .map_err(|error| {
                BatchSettlementError::Backend(format!(
                    "could not read the cluster's genesis hash: {error}"
                ))
            })?;
        let sponsor_pubkey = sponsor.pubkey();
        let lamports = retry_read(|| rpc.get_balance(&sponsor_pubkey))
            .await
            .map_err(backend_error)?;
        if lamports == 0 {
            return Err(BatchSettlementError::Backend(format!(
                "[settlement.solana] key {sponsor_pubkey} holds no lamports, so it cannot pay for \
                 any settlement transaction; fund it before starting the node"
            )));
        }
        let backend = SolanaBatchSettlement {
            rpc,
            confirm: ConfirmPolicy::for_transport(transport),
            program_id,
            sponsor,
            mint,
            min_grace_period_secs,
            min_sponsored_deposit,
            genesis_hash,
            admitted: Mutex::new(HashSet::new()),
            cluster_rent: OnceLock::new(),
            outbound: Mutex::new(HashMap::new()),
            sponsor_http: pay::SponsorClients::new(None)?,
            treasury: Mutex::new(None),
        };
        backend.ensure_receiving_account().await?;
        Ok(backend)
    }

    /// Create this node's receiving account -- the sponsor key's associated
    /// token account for the mint -- if it does not exist yet. Every channel
    /// this node admits distributes to it, and the sponsor endpoint refuses
    /// an `open` by name while it is missing, since a payout to a missing
    /// account forfeits to the program's treasury (Cantina 3.1.4). So a node
    /// that booted without one could never be opened toward.
    ///
    /// Read before it transacts (ADR 0073 decision 5): a restart whose
    /// account already exists sends nothing, and the create is idempotent
    /// for the one that races it.
    async fn ensure_receiving_account(&self) -> Result<(), BatchSettlementError> {
        let owner = self.sponsor.pubkey();
        let receiving =
            spl_associated_token_account::get_associated_token_address(&owner, &self.mint);
        let existing = retry_read(|| {
            self.rpc
                .get_account_with_commitment(&receiving, CommitmentConfig::confirmed())
        })
        .await
        .map_err(backend_error)?;
        if existing.value.is_some() {
            return Ok(());
        }
        self.submit(&[
            spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                &owner,
                &owner,
                &self.mint,
                &spl_token::id(),
            ),
        ])
        .await
        .map_err(|error| {
            BatchSettlementError::Backend(format!(
                "[settlement.solana] could not create this node's receiving account {receiving} \
                 for mint {}: {error}",
                self.mint
            ))
        })
    }

    /// The CAIP-2 network id of the chain this node connected to (ADR 0074
    /// decision 8): the greeting's `network`, read off the chain's own
    /// genesis hash rather than guessed from the RPC URL.
    pub fn caip2_network(&self) -> String {
        crate::caip2_solana_network(&self.genesis_hash)
    }

    /// The public cluster the chain's genesis hash names, or `None` for any
    /// other chain -- every `solana-test-validator` among them (issue
    /// #1131).
    pub fn cluster(&self) -> Option<&'static str> {
        crate::cluster_for_genesis_hash(&self.genesis_hash)
    }

    /// The token program that owns [`Self::mint`]: SPL Token, always,
    /// because [`Self::connect`] refuses a mint any other program owns. The
    /// greeting's x402 `extra.tokenProgram` (ADR 0074 decision 8, issue
    /// #1357) is read from here.
    pub fn token_program(&self) -> Pubkey {
        spl_token::id()
    }

    /// Post every `open` toward a counterparty whose sponsor endpoint is an
    /// onion host through `socks_proxy` (ADR 0070): the node's one
    /// root-level proxy, chosen per post by
    /// `connector_config::is_onion_endpoint`. A sponsor on any other host is
    /// still posted to direct. Without this, an onion sponsor is refused by
    /// name at the post, before anything is dialed.
    pub fn with_socks_proxy(
        mut self,
        socks_proxy: &url::Url,
    ) -> Result<Self, BatchSettlementError> {
        self.sponsor_http = pay::SponsorClients::new(Some(socks_proxy))?;
        Ok(self)
    }

    /// The sponsor key: `payee` and `rent_payer` of every channel this node
    /// admits, and the `feePayer` the greeting publishes (ADR 0074 decision 8).
    pub fn sponsor(&self) -> Pubkey {
        self.sponsor.pubkey()
    }

    /// The receiver: the one distribution recipient an admitted channel may
    /// name, at 10000 bps, and the greeting's `payTo`. The sponsor key, as
    /// this type's own documentation explains.
    pub fn receiver(&self) -> Pubkey {
        self.sponsor.pubkey()
    }

    /// The mint this node settles in.
    pub fn mint(&self) -> Pubkey {
        self.mint
    }

    /// The `payment-channels` program this backend reads.
    pub fn program_id(&self) -> Pubkey {
        self.program_id
    }

    /// The shortest `grace_period` admitted, in seconds.
    pub fn min_grace_period_secs(&self) -> u64 {
        self.min_grace_period_secs
    }

    /// The smallest opening deposit, in the mint's base units, the sponsor
    /// co-signs an `open` for, and the `minDeposit` the greeting publishes.
    pub fn min_sponsored_deposit(&self) -> u64 {
        self.min_sponsored_deposit
    }

    fn admitted(&self) -> MutexGuard<'_, HashSet<Pubkey>> {
        self.admitted
            .lock()
            .expect("SolanaBatchSettlement admitted-set lock poisoned")
    }

    /// Read the channel at `address`, or `None` when no `Channel` of this
    /// program lives there. `Err` only when the chain could not be read.
    async fn read(
        &self,
        address: &Pubkey,
    ) -> Result<Option<wire::ChannelAccount>, BatchSettlementError> {
        let response = retry_read(|| {
            self.rpc
                .get_account_with_commitment(address, CommitmentConfig::confirmed())
        })
        .await
        .map_err(backend_error)?;
        Ok(response
            .value
            .filter(|account| account.owner == self.program_id)
            .and_then(|account| wire::ChannelAccount::parse(&account.data)))
    }

    /// [`read`](Self::read), with "nothing there" as the port's
    /// [`ChannelNotFound`](BatchSettlementError::ChannelNotFound).
    async fn read_existing(
        &self,
        channel: &ChannelId,
        address: &Pubkey,
    ) -> Result<wire::ChannelAccount, BatchSettlementError> {
        self.read(address)
            .await?
            .ok_or_else(|| BatchSettlementError::ChannelNotFound(channel.clone()))
    }

    /// Judge the account read at `address` for admission: first that it is
    /// the channel it describes, then every rule of ADR 0074 decision 2.
    fn vet(
        &self,
        channel: &ChannelId,
        address: &Pubkey,
        account: &wire::ChannelAccount,
    ) -> Result<BatchChannelState, BatchSettlementError> {
        vet(
            channel,
            address,
            account,
            &self.program_id,
            &self.sponsor.pubkey(),
            &self.mint,
            self.min_grace_period_secs,
        )
    }

    fn require_admitted(&self, channel: &ChannelId) -> Result<Pubkey, BatchSettlementError> {
        Pubkey::from_str(&channel.0)
            .ok()
            .filter(|address| self.admitted().contains(address))
            .ok_or_else(|| BatchSettlementError::ChannelNotAdmitted(channel.clone()))
    }

    /// Sign `instructions` with the sponsor key, as fee payer and as every
    /// signer they name, and wait for their outcome.
    async fn submit(&self, instructions: &[Instruction]) -> Result<(), BatchSettlementError> {
        let (blockhash, last_valid_block_height) = retry_read(|| {
            self.rpc
                .get_latest_blockhash_with_commitment(CommitmentConfig::confirmed())
        })
        .await
        .map_err(backend_error)?;
        let transaction = Transaction::new_signed_with_payer(
            instructions,
            Some(&self.sponsor.pubkey()),
            &[&self.sponsor],
            blockhash,
        );
        send_and_confirm(
            &self.rpc,
            &transaction,
            last_valid_block_height,
            self.confirm,
        )
        .await
        .map(|_signature| ())
        .map_err(backend_error)
    }

    /// The cluster's Rent sysvar, read once per process: a cluster's rent
    /// does not change under a running node. `Err` names why it could not be
    /// read.
    async fn cluster_rent(&self) -> Result<solana_sdk::rent::Rent, String> {
        if let Some(rent) = self.cluster_rent.get() {
            return Ok(rent.clone());
        }
        let sysvar = solana_sdk::sysvar::rent::id();
        let account = retry_read(|| self.rpc.get_account(&sysvar))
            .await
            .map_err(|error| error.to_string())?;
        let rent: solana_sdk::rent::Rent = bincode::deserialize(&account.data)
            .map_err(|error| format!("the Rent sysvar does not decode: {error}"))?;
        // A racing request may have set it first, to the same value.
        let _ = self.cluster_rent.set(rent.clone());
        Ok(rent)
    }
}

/// [`SolanaBatchSettlement::vet`], with the backend's facts as arguments so
/// every branch can be exercised on a fabricated account.
fn vet(
    channel: &ChannelId,
    address: &Pubkey,
    account: &wire::ChannelAccount,
    program_id: &Pubkey,
    sponsor: &Pubkey,
    mint: &Pubkey,
    min_grace_period_secs: u64,
) -> Result<BatchChannelState, BatchSettlementError> {
    at_its_own_address(channel, address, account, program_id)?;
    if let Some(refusal) = admission_refusal(account, sponsor, mint, min_grace_period_secs) {
        return Err(BatchSettlementError::NotAdmissible {
            channel: channel.clone(),
            refusal,
        });
    }
    Ok(state_of(channel, account))
}

/// Refuse `account` unless its own seeds derive `address`: its bytes are
/// only the channel's word for itself until they do (X402 SVM spec
/// `#L1379-L1385`). Checked by admission and by restoring alike.
fn at_its_own_address(
    channel: &ChannelId,
    address: &Pubkey,
    account: &wire::ChannelAccount,
    program_id: &Pubkey,
) -> Result<(), BatchSettlementError> {
    let derived = account.derive_address(program_id);
    if derived != *address {
        return Err(BatchSettlementError::ChannelIdMismatch {
            presented: channel.clone(),
            derived: ChannelId(derived.to_string()),
        });
    }
    Ok(())
}

/// The first rule of ADR 0074 decision 2 that `account` breaks, in the
/// record's order, or `None` if it breaks none. `sponsor` must be `payee`,
/// `rent_payer` and the one distribution recipient, at 10000 bps.
fn admission_refusal(
    account: &wire::ChannelAccount,
    sponsor: &Pubkey,
    mint: &Pubkey,
    min_grace_period_secs: u64,
) -> Option<AdmissionRefusal> {
    if account.status != wire::ChannelStatus::Open {
        return Some(AdmissionRefusal::NotOpen);
    }
    if account.payee != *sponsor {
        return Some(AdmissionRefusal::NotPayableToThisNode { field: "payee" });
    }
    if account.rent_payer != *sponsor {
        return Some(AdmissionRefusal::NotPayableToThisNode {
            field: "rent_payer",
        });
    }
    if account.mint != *mint {
        return Some(AdmissionRefusal::TokenNotSettled);
    }
    if account.distribution_hash != wire::distribution_hash(&wire::sole_recipient(sponsor)) {
        return Some(AdmissionRefusal::NotPayableToThisNode {
            field: "distribution_hash",
        });
    }
    let grace_period = u64::from(account.grace_period);
    if grace_period < min_grace_period_secs {
        return Some(AdmissionRefusal::DelayBelowMinimum {
            delay_secs: grace_period,
            minimum_secs: min_grace_period_secs,
        });
    }
    None
}

/// The port's view of a `Channel`. Collateral is `deposit − settled` while
/// Open and zero otherwise (the port's rule, from #1340): a Closing channel
/// backs no new voucher, and a sealed one backs nothing at all.
fn state_of(id: &ChannelId, account: &wire::ChannelAccount) -> BatchChannelState {
    let status = match account.status {
        wire::ChannelStatus::Open => BatchChannelStatus::Open,
        wire::ChannelStatus::Closing => BatchChannelStatus::Closing,
        wire::ChannelStatus::Sealed | wire::ChannelStatus::Distributed => {
            BatchChannelStatus::Sealed
        }
    };
    let collateral = if status == BatchChannelStatus::Open {
        u128::from(account.deposit.saturating_sub(account.settled))
    } else {
        0
    };
    BatchChannelState {
        id: id.clone(),
        status,
        voucher_signer: VoucherSigner::Solana(account.authorized_signer.to_bytes()),
        landed: u128::from(account.settled),
        collateral,
    }
}

/// A Solana channel id as the address it names. A string that is not one
/// names nothing this program could hold.
fn address_of(channel: &ChannelId) -> Result<Pubkey, BatchSettlementError> {
    Pubkey::from_str(&channel.0).map_err(|_| BatchSettlementError::ChannelNotFound(channel.clone()))
}

fn backend_error(error: impl std::fmt::Display) -> BatchSettlementError {
    BatchSettlementError::Backend(error.to_string())
}

#[async_trait]
impl BatchSettlementBackend for SolanaBatchSettlement {
    async fn admit(
        &self,
        presentation: ChannelPresentation,
    ) -> Result<BatchChannelState, BatchSettlementError> {
        let ChannelPresentation::Solana { channel } = presentation else {
            return Err(BatchSettlementError::WrongChain {
                presented: presentation.chain(),
                backend: CHAIN,
            });
        };
        let address = address_of(&channel)?;
        let account = self.read_existing(&channel, &address).await?;
        let state = self.vet(&channel, &address, &account)?;
        self.admitted().insert(address);
        Ok(state)
    }

    /// [`admit`](BatchSettlementBackend::admit) without the admission rules:
    /// the account is still read and still trusted only at the address its
    /// own seeds derive. A Closing channel restores, since landing on it is
    /// exactly what a held voucher needs.
    async fn restore(
        &self,
        presentation: ChannelPresentation,
    ) -> Result<BatchChannelState, BatchSettlementError> {
        let ChannelPresentation::Solana { channel } = presentation else {
            return Err(BatchSettlementError::WrongChain {
                presented: presentation.chain(),
                backend: CHAIN,
            });
        };
        let address = address_of(&channel)?;
        let account = self.read_existing(&channel, &address).await?;
        at_its_own_address(&channel, &address, &account, &self.program_id)?;
        self.admitted().insert(address);
        Ok(state_of(&channel, &account))
    }

    /// Read from the chain now. A channel whose account `distribute` or
    /// `reclaim` has since deallocated reports
    /// [`ChannelNotFound`](BatchSettlementError::ChannelNotFound): the chain
    /// no longer holds anything to report, and this backend keeps no copy.
    async fn channel_state(
        &self,
        channel: &ChannelId,
    ) -> Result<BatchChannelState, BatchSettlementError> {
        let address = self.require_admitted(channel)?;
        let account = self.read_existing(channel, &address).await?;
        Ok(state_of(channel, &account))
    }

    /// `settle` while Open; `settle_and_seal` while Closing, which seals the
    /// channel -- it is the only instruction that can land there, and only
    /// this node, as `payee`, can sign it. Either way the precompile
    /// instruction carrying the voucher goes immediately before it.
    ///
    /// The voucher's signature is checked against the channel's
    /// `authorized_signer` before anything is sent, so a voucher the program
    /// would refuse costs no fee. The grace deadline is not: a
    /// `settle_and_seal` sent after `closure_started_at + grace_period` is
    /// refused by the program and reported as
    /// [`Backend`](BatchSettlementError::Backend). Landing before then is the
    /// Closing watcher's job (issue #1344).
    async fn land(
        &self,
        channel: &ChannelId,
        voucher: Voucher,
    ) -> Result<BatchChannelState, BatchSettlementError> {
        let address = self.require_admitted(channel)?;
        let account = self.read_existing(channel, &address).await?;
        let landed = u128::from(account.settled);
        match account.status {
            wire::ChannelStatus::Sealed | wire::ChannelStatus::Distributed => {
                return Err(BatchSettlementError::ChannelSealed(channel.clone()))
            }
            wire::ChannelStatus::Open | wire::ChannelStatus::Closing => {}
        }
        if voucher.cumulative_amount <= landed {
            return Err(BatchSettlementError::StaleVoucher {
                amount: voucher.cumulative_amount,
                landed,
            });
        }
        let deposited = u128::from(account.deposit);
        if voucher.cumulative_amount > deposited {
            return Err(BatchSettlementError::VoucherExceedsDeposit {
                amount: voucher.cumulative_amount,
                deposited,
            });
        }
        // At most the deposit, which is a u64, so this cannot fail; if it
        // ever did it is refused by name rather than panicking the caller.
        let amount = u64::try_from(voucher.cumulative_amount).map_err(|_| {
            BatchSettlementError::VoucherExceedsDeposit {
                amount: voucher.cumulative_amount,
                deposited,
            }
        })?;
        let signature: [u8; 64] = voucher.signature.as_slice().try_into().map_err(|_| {
            BatchSettlementError::InvalidVoucherSignature(format!(
                "a payment-channels voucher signature is 64 bytes of Ed25519, got {}",
                voucher.signature.len()
            ))
        })?;
        if !connector_signer::verify_solana_voucher(
            &address.to_bytes(),
            amount,
            0,
            &signature,
            &account.authorized_signer.to_bytes(),
        ) {
            return Err(BatchSettlementError::InvalidVoucherSignature(format!(
                "not a signature by the channel's authorized_signer {} over a voucher for {amount}",
                account.authorized_signer
            )));
        }

        let instructions = if account.status == wire::ChannelStatus::Open {
            wire::settle_instructions(
                &self.program_id,
                &address,
                &account.authorized_signer,
                amount,
                &signature,
            )
        } else {
            wire::settle_and_seal_instructions(
                &self.program_id,
                &self.sponsor.pubkey(),
                &address,
                &account.authorized_signer,
                amount,
                &signature,
            )
        };
        self.submit(&instructions).await?;

        let account = self.read_existing(channel, &address).await?;
        Ok(state_of(channel, &account))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(status: wire::ChannelStatus, deposit: u64, settled: u64) -> wire::ChannelAccount {
        wire::ChannelAccount {
            bump: 255,
            status,
            salt: 0,
            deposit,
            settled,
            payout_watermark: 0,
            closure_started_at: 0,
            payer_withdrawn_at: 0,
            grace_period: 86_400,
            distribution_hash: [0; 32],
            payer: Pubkey::new_unique(),
            payee: Pubkey::new_unique(),
            authorized_signer: Pubkey::new_from_array([0xdd; 32]),
            mint: Pubkey::new_unique(),
            rent_payer: Pubkey::new_unique(),
            open_slot: 0,
        }
    }

    const ONE_DAY: u64 = 86_400;

    /// A channel that meets every rule for `sponsor` in `mint`.
    fn admissible(sponsor: Pubkey, mint: Pubkey) -> wire::ChannelAccount {
        wire::ChannelAccount {
            payee: sponsor,
            rent_payer: sponsor,
            mint,
            distribution_hash: wire::distribution_hash(&wire::sole_recipient(&sponsor)),
            grace_period: ONE_DAY as u32,
            ..account(wire::ChannelStatus::Open, 1_000, 0)
        }
    }

    /// Each rule ADR 0074 decision 2 fixes refuses by its own name, and a
    /// channel breaking none is admitted. The contract suite reaches the
    /// payee, mint and grace-period rules on a real chain; `rent_payer` and
    /// the distribution are this chain's alone, so they are pinned here.
    #[test]
    fn every_admission_rule_refuses_by_name() {
        let sponsor = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let someone = Pubkey::new_unique();
        let judge =
            |account: wire::ChannelAccount| admission_refusal(&account, &sponsor, &mint, ONE_DAY);
        let good = admissible(sponsor, mint);

        assert_eq!(judge(good.clone()), None);
        assert_eq!(
            judge(wire::ChannelAccount {
                grace_period: ONE_DAY as u32 + 1,
                ..good.clone()
            }),
            None,
            "a grace period above the minimum"
        );

        for status in [
            wire::ChannelStatus::Closing,
            wire::ChannelStatus::Sealed,
            wire::ChannelStatus::Distributed,
        ] {
            assert_eq!(
                judge(wire::ChannelAccount {
                    status,
                    ..good.clone()
                }),
                Some(AdmissionRefusal::NotOpen)
            );
        }
        assert_eq!(
            judge(wire::ChannelAccount {
                payee: someone,
                ..good.clone()
            }),
            Some(AdmissionRefusal::NotPayableToThisNode { field: "payee" })
        );
        assert_eq!(
            judge(wire::ChannelAccount {
                rent_payer: someone,
                ..good.clone()
            }),
            Some(AdmissionRefusal::NotPayableToThisNode {
                field: "rent_payer"
            }),
            "a third-party sponsor can seal before this node settles (ADR 0074 decision 5)"
        );
        assert_eq!(
            judge(wire::ChannelAccount {
                mint: someone,
                ..good.clone()
            }),
            Some(AdmissionRefusal::TokenNotSettled)
        );
        assert_eq!(
            judge(wire::ChannelAccount {
                grace_period: ONE_DAY as u32 - 1,
                ..good.clone()
            }),
            Some(AdmissionRefusal::DelayBelowMinimum {
                delay_secs: ONE_DAY - 1,
                minimum_secs: ONE_DAY,
            })
        );
    }

    /// The distribution must be exactly one entry, this node's receiver, at
    /// 10000 bps. Everything else -- another recipient, a partial share, an
    /// extra entry even at zero net effect, the payee's implicit remainder --
    /// commits to a different hash and is refused.
    #[test]
    fn only_the_sole_recipient_distribution_is_admitted() {
        let sponsor = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let someone = Pubkey::new_unique();
        let entry = |recipient, bps| wire::DistributionEntry { recipient, bps };
        for entries in [
            vec![],
            vec![entry(someone, 10_000)],
            vec![entry(sponsor, 9_999)],
            vec![entry(sponsor, 5_000), entry(someone, 5_000)],
            vec![entry(sponsor, 10_000), entry(someone, 1)],
        ] {
            let account = wire::ChannelAccount {
                distribution_hash: wire::distribution_hash(&entries),
                ..admissible(sponsor, mint)
            };
            assert_eq!(
                admission_refusal(&account, &sponsor, &mint, ONE_DAY),
                Some(AdmissionRefusal::NotPayableToThisNode {
                    field: "distribution_hash"
                }),
                "{entries:?}"
            );
        }
    }

    /// An account is trusted only at the address its own seed fields derive.
    /// Bytes that describe a perfectly admissible channel, found anywhere
    /// else, are refused as a mismatch before any rule is judged -- and the
    /// refusal names where the channel it describes would live.
    #[test]
    fn an_account_not_at_its_own_pda_is_refused_before_it_is_judged() {
        let program_id = Pubkey::from_str(wire::PAYMENT_CHANNELS_PROGRAM_ID).expect("base58");
        let sponsor = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let account = admissible(sponsor, mint);
        let home = account.derive_address(&program_id);
        let vet_at = |address: &Pubkey| {
            vet(
                &ChannelId(address.to_string()),
                address,
                &account,
                &program_id,
                &sponsor,
                &mint,
                ONE_DAY,
            )
        };

        let state = vet_at(&home).expect("admitted at its own address");
        assert_eq!(state.id, ChannelId(home.to_string()));

        let elsewhere = Pubkey::new_unique();
        assert_eq!(
            vet_at(&elsewhere),
            Err(BatchSettlementError::ChannelIdMismatch {
                presented: ChannelId(elsewhere.to_string()),
                derived: ChannelId(home.to_string()),
            })
        );

        // Under another program the same seeds derive another address, so a
        // look-alike program's channel is not this one's.
        let other_program = Pubkey::new_unique();
        assert!(matches!(
            vet(
                &ChannelId(home.to_string()),
                &home,
                &account,
                &other_program,
                &sponsor,
                &mint,
                ONE_DAY,
            ),
            Err(BatchSettlementError::ChannelIdMismatch { .. })
        ));
    }

    /// Collateral is what still backs a new voucher: the unsettled deposit
    /// while Open, nothing once the payer has asked to close or the channel
    /// is sealed -- so `voucher_ceiling` is exactly `landed` there.
    #[test]
    fn only_an_open_channel_has_collateral() {
        let id = ChannelId("c".to_string());
        let open = state_of(&id, &account(wire::ChannelStatus::Open, 1_000, 300));
        assert_eq!(open.status, BatchChannelStatus::Open);
        assert_eq!((open.landed, open.collateral), (300, 700));
        assert_eq!(open.voucher_signer, VoucherSigner::Solana([0xdd; 32]));

        for (status, expected) in [
            (wire::ChannelStatus::Closing, BatchChannelStatus::Closing),
            (wire::ChannelStatus::Sealed, BatchChannelStatus::Sealed),
            (wire::ChannelStatus::Distributed, BatchChannelStatus::Sealed),
        ] {
            let state = state_of(&id, &account(status, 1_000, 300));
            assert_eq!(state.status, expected);
            assert_eq!(state.collateral, 0);
            assert_eq!(state.voucher_ceiling(), 300);
        }
    }
}
