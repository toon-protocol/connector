//! The Solana implementation of the receive-only batch-settlement port
//! (ADR 0074 decisions 2, 5 and 9; issue #1343): x402 `batch-settlement`
//! channels on solana-foundation's `payment-channels` program, which a
//! client opens, this node's sponsor key co-signs, and this node only
//! admits, reads and lands vouchers on.
//!
//! It shares nothing with [`SolanaSettlementBackend`](crate::SolanaSettlementBackend)
//! but the crate: a different program, a different account, a different
//! port. What it does reuse is this crate's transaction submission and
//! confirm loop, the table's one [`RpcTransport`] (ADR 0073), the
//! Ed25519 precompile layout in [`crate::wire`] and the voucher message and
//! verifier in `connector-signer` (issue #1341).
//!
//! **What is not here.** Watching for Closing, `distribute`, `reclaim` and
//! `getProgramAccounts` rediscovery are issue #1344's; the public sponsor
//! endpoint is issue #1346's, and builds on [`wire::OpenChannel`]. The
//! runtime builds this backend when `[settlement.solana.batch_settlement]`
//! is written and hands it to the client edge's claim gate
//! (`connector-cli`'s `batch_settlement` module).

pub mod sponsor;
pub mod wire;

use std::collections::HashSet;
use std::str::FromStr;
use std::sync::{Mutex, MutexGuard};

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
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::transaction::Transaction;

use crate::submit::{send_and_confirm, ConfirmPolicy};

/// The chain this backend answers for, in the port's spelling.
const CHAIN: &str = "solana";

/// A [`BatchSettlementBackend`] over a real `payment-channels` deployment.
///
/// **One key, three seats.** The sponsor key is this node's
/// `[settlement.solana]` settlement key. An admitted channel must name it as
/// `payee` and as `rent_payer` (ADR 0074 decision 5: the only configuration
/// in which this node can always land its latest voucher), and its
/// distribution must send everything to the **receiver** -- the owner of
/// this node's receiving account, which is the same key, since the
/// settlement table names no other. The receiver is what the greeting
/// publishes as `payTo` (decision 8).
///
/// Holds no channel ledger. Every answer is read from the chain; the only
/// local memory is which channels have been admitted, because the port
/// requires admission before [`channel_state`](BatchSettlementBackend::channel_state)
/// and [`land`](BatchSettlementBackend::land).
pub struct SolanaBatchSettlement {
    rpc: RpcClient,
    confirm: ConfirmPolicy,
    program_id: Pubkey,
    sponsor: Keypair,
    mint: Pubkey,
    min_grace_period_secs: u64,
    admitted: Mutex<HashSet<Pubkey>>,
}

impl SolanaBatchSettlement {
    /// Bind to the `payment-channels` program at `program_id`
    /// (`[settlement.solana.batch_settlement] program_id`), admitting
    /// channels in `mint` (`[settlement.solana] token_address`) whose
    /// `grace_period` is at least `min_grace_period_secs`, under the sponsor
    /// key `sponsor_seed` derives (the `[settlement.solana]` key file's
    /// 32-byte ed25519 seed).
    ///
    /// Refuses, naming it, a `program_id` that is not an executable account:
    /// a node that would otherwise admit nothing and say nothing.
    pub async fn connect(
        transport: &RpcTransport,
        sponsor_seed: &[u8; 32],
        program_id: Pubkey,
        mint: Pubkey,
        min_grace_period_secs: u64,
    ) -> Result<Self, BatchSettlementError> {
        let sponsor =
            solana_sdk::signer::keypair::keypair_from_seed(sponsor_seed).map_err(backend_error)?;
        let rpc = rpc_client(
            transport,
            RpcClientConfig::with_commitment(CommitmentConfig::confirmed()),
        );
        let program = retry_read(|| rpc.get_account(&program_id))
            .await
            .map_err(|error| {
                BatchSettlementError::Backend(format!(
                    "[settlement.solana.batch_settlement] program_id {program_id} could not be \
                     read: {error}"
                ))
            })?;
        if !program.executable {
            return Err(BatchSettlementError::Backend(format!(
                "[settlement.solana.batch_settlement] program_id {program_id} is not an \
                 executable program account"
            )));
        }
        Ok(SolanaBatchSettlement {
            rpc,
            confirm: ConfirmPolicy::for_transport(transport),
            program_id,
            sponsor,
            mint,
            min_grace_period_secs,
            admitted: Mutex::new(HashSet::new()),
        })
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
    // The account's bytes are only the channel's word for itself until its
    // own seeds derive the address it lives at (X402 SVM spec
    // `#L1379-L1385`).
    let derived = account.derive_address(program_id);
    if derived != *address {
        return Err(BatchSettlementError::ChannelIdMismatch {
            presented: channel.clone(),
            derived: ChannelId(derived.to_string()),
        });
    }
    if let Some(refusal) = admission_refusal(account, sponsor, mint, min_grace_period_secs) {
        return Err(BatchSettlementError::NotAdmissible {
            channel: channel.clone(),
            refusal,
        });
    }
    Ok(state_of(channel, account))
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
        // At most the deposit, which is a u64.
        let amount = u64::try_from(voucher.cumulative_amount)
            .expect("a voucher no larger than a u64 deposit fits a u64");
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
