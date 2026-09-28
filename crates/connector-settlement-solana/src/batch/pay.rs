//! The paying half of the batch-settlement port on `payment-channels` (ADR
//! 0075 decisions 2 and 3, issue #1375): this node as the **payer** on a
//! channel toward a counterparty, where the rest of this module's parent is
//! this node as the receiver.
//!
//! **One key, every seat that is this node's.** The `[settlement.solana]`
//! settlement key is `payer` -- it funds the deposit and every top-up, and
//! takes the refund -- and it is `authorized_signer`, so it signs every
//! voucher: the 50-byte message with `expires_at = 0` (ADR 0074 decision 3).
//! `[signer]` never appears (ADR 0075 decision 3). The same key is this
//! node's sponsor key on channels it receives on; the two roles never meet
//! on one channel, since a channel's payer and payee are different nodes.
//!
//! **Opening goes through the counterparty.** The `open` names the
//! counterparty's sponsor key as fee payer, `rent_payer` and `payee`, and
//! its receiving account as the one distribution entry at 10000 bps, with
//! `grace_period` at exactly its published minimum. This node signs it as
//! payer and posts it to the counterparty's `sponsorEndpoint` (ADR 0074
//! decision 9), which co-signs, submits and admits it. That is how the
//! receiver keeps the `payee` seat, without which it could not land its
//! latest voucher once this node asks to close (ADR 0074 decision 5). The
//! counterparty's `min_sponsored_deposit` bounds the opening deposit, and it
//! is the sponsor that says so: a deposit below it comes back as its named
//! refusal, [`OpenRefused`](BatchSettlementError::OpenRefused), rather than
//! being second-guessed here.
//!
//! **Winding down is `request_close`, then `distribute`.** Between them the
//! receiver has the grace period in which to land its latest voucher with
//! `settle_and_seal`; once it has, or once the grace period has run and the
//! permissionless `seal` has been cranked, `distribute` pays the receiver
//! what it landed and refunds this node the rest. `reclaim`, which follows,
//! returns rent to the channel's `rent_payer` -- the counterparty's sponsor
//! -- so it is the counterparty's sweep (`SolanaBatchWatcher`), not this
//! half's.
//!
//! **What it remembers.** Which channels it opened, the counterparty's
//! receiver on each (`distribute` must re-present the distribution `open`
//! committed to, and only its hash is on chain), the highest amount signed on
//! each, and what each still backs -- for the process lifetime. Journaling
//! that, and restoring the signed watermark from the receiver's
//! `POST /ilp/claim-state` after a restart, are the operator surface's and
//! the peering's (ADR 0075 decisions 6 and 8; issues #1376, #1378).

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use connector_chain_rpc::retry_read;
use connector_settlement::batch::{
    BatchChannelState, BatchSettlementError, BatchSettlementPayer, ChannelPresentation,
    OpenedChannel, OutboundChannelState, ReceiverTerms, Voucher, VoucherSigner,
};
use connector_settlement::ChannelId;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::message::{v0, VersionedMessage};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Signature, Signer};
use solana_sdk::transaction::VersionedTransaction;

use super::sweep::SponsoredChannel;
use super::{backend_error, sponsor, state_of, wire, SolanaBatchSettlement, CHAIN};

/// How long a post to a counterparty's sponsor endpoint may take. The
/// sponsor answers only once its co-signed `open` has confirmed, which can
/// take up to a blockhash's lifetime -- about a minute -- so this leaves that
/// and the round trip room, and no more.
pub const SPONSOR_POST_TIMEOUT: Duration = Duration::from_secs(120);

/// The client [`SolanaBatchSettlement`] posts opens with.
pub(super) fn sponsor_http_client() -> Result<reqwest::Client, BatchSettlementError> {
    reqwest::Client::builder()
        .timeout(SPONSOR_POST_TIMEOUT)
        .build()
        .map_err(backend_error)
}

/// What the paying half remembers about one channel it opened.
#[derive(Debug, Clone)]
pub(super) struct Outbound {
    /// The owner of the counterparty's receiving account: the one
    /// distribution recipient the `open` committed to, which `distribute`
    /// must name again.
    receiver: Pubkey,
    /// The highest cumulative amount signed: the next voucher must exceed it.
    signed: u128,
    /// The highest cumulative amount the channel backs, as of this node's
    /// own last open, top-up or close: `deposit` while Open, what was landed
    /// once Closing. Only this node moves it, which is what lets a voucher be
    /// signed without reading the chain (the port's `sign_voucher`).
    backed: u128,
    /// The channel as it stood once this node distributed it, kept because
    /// `distribute` may deallocate the account and leave nothing to read.
    finished: Option<BatchChannelState>,
}

/// Why a post to a sponsor endpoint produced no channel.
enum SponsorAnswer {
    /// The endpoint answered with a named refusal: its `error` and `detail`.
    Refused { name: String, detail: String },
    /// The endpoint could not be reached, or did not answer in its own terms.
    Unreachable(String),
}

impl SponsorAnswer {
    fn into_error(self) -> BatchSettlementError {
        match self {
            SponsorAnswer::Refused { name, detail } => {
                BatchSettlementError::OpenRefused(format!("{name}: {detail}"))
            }
            SponsorAnswer::Unreachable(reason) => BatchSettlementError::Backend(reason),
        }
    }
}

impl std::fmt::Display for SponsorAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SponsorAnswer::Refused { name, detail } => write!(f, "{name}: {detail}"),
            SponsorAnswer::Unreachable(reason) => write!(f, "{reason}"),
        }
    }
}

impl SolanaBatchSettlement {
    /// This node's settlement key: payer and voucher signer of every channel
    /// it opens, and sponsor of every channel it receives on.
    pub fn settlement_key(&self) -> Pubkey {
        self.sponsor.pubkey()
    }

    fn outbound(&self) -> MutexGuard<'_, HashMap<Pubkey, Outbound>> {
        self.outbound
            .lock()
            .expect("SolanaBatchSettlement outbound lock poisoned")
    }

    /// The channel `channel` names and what this node remembers of it, or
    /// [`NotOutbound`](BatchSettlementError::NotOutbound) if it did not
    /// open it.
    fn outbound_record(
        &self,
        channel: &ChannelId,
    ) -> Result<(Pubkey, Outbound), BatchSettlementError> {
        Pubkey::from_str(&channel.0)
            .ok()
            .and_then(|address| {
                self.outbound()
                    .get(&address)
                    .map(|record| (address, record.clone()))
            })
            .ok_or_else(|| BatchSettlementError::NotOutbound(channel.clone()))
    }

    /// Record what the channel at `address` backs now, after this node moved
    /// it.
    fn set_backed(&self, address: &Pubkey, backed: u128) {
        if let Some(record) = self.outbound().get_mut(address) {
            record.backed = backed;
        }
    }

    /// Where a cluster's rent exemption threshold is not 1 -- the pinned
    /// v2.1.21 test validator, and no public cluster -- `payment-channels`
    /// under-funds a new channel's rent and the counterparty's sponsor
    /// refuses the `open` unless the channel already holds it
    /// ([`sponsor::vet_channel_rent`], issue #1356). There this node, which
    /// holds SOL, puts the rent there itself first; the program then tops up
    /// nothing, and `reclaim` hands it to the `rent_payer`. On a cluster
    /// whose threshold is 1 this sends nothing.
    async fn prefund_channel_rent(&self, channel: &Pubkey) -> Result<(), BatchSettlementError> {
        let rent = self
            .cluster_rent()
            .await
            .map_err(BatchSettlementError::Backend)?;
        if sponsor::threshold_is_one(&rent) {
            return Ok(());
        }
        let minimum = rent.minimum_balance(wire::CHANNEL_ACCOUNT_LEN);
        self.submit(&[solana_sdk::system_instruction::transfer(
            &self.sponsor.pubkey(),
            channel,
            minimum,
        )])
        .await
    }

    /// `open`, as a transaction whose fee payer is the counterparty's
    /// `sponsor` and which this node has signed as payer, base64 of its wire
    /// bytes: exactly what a stock x402 client posts to a sponsor endpoint.
    async fn payer_signed_open(
        &self,
        open: &wire::OpenChannel,
        sponsor: &Pubkey,
    ) -> Result<String, BatchSettlementError> {
        let (blockhash, _) = retry_read(|| {
            self.rpc
                .get_latest_blockhash_with_commitment(CommitmentConfig::confirmed())
        })
        .await
        .map_err(backend_error)?;
        let message = VersionedMessage::V0(
            v0::Message::try_compile(
                sponsor,
                &[open.instruction(&self.program_id)],
                &[],
                blockhash,
            )
            .map_err(backend_error)?,
        );
        let payer = self.sponsor.pubkey();
        let signers = usize::from(message.header().num_required_signatures);
        let index = message.static_account_keys()[..signers]
            .iter()
            .position(|key| *key == payer)
            .ok_or_else(|| {
                BatchSettlementError::Backend(
                    "the open does not name this node as a signer".to_string(),
                )
            })?;
        let mut signatures = vec![Signature::default(); signers];
        signatures[index] = self.sponsor.sign_message(&message.serialize());
        let transaction = VersionedTransaction {
            signatures,
            message,
        };
        let bytes = bincode::serialize(&transaction).map_err(backend_error)?;
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    /// Post `transaction` to `endpoint` and read back the channel the
    /// sponsor made, or why it made none.
    async fn post_to_sponsor(
        &self,
        endpoint: &str,
        transaction: String,
    ) -> Result<Pubkey, SponsorAnswer> {
        let response = self
            .sponsor_http
            .post(endpoint)
            .json(&serde_json::json!({ "transaction": transaction }))
            .send()
            .await
            .map_err(|error| {
                SponsorAnswer::Unreachable(format!(
                    "the sponsor endpoint {endpoint} could not be reached: {error}"
                ))
            })?;
        let status = response.status();
        let body: serde_json::Value = response.json().await.map_err(|error| {
            SponsorAnswer::Unreachable(format!(
                "the sponsor endpoint {endpoint} answered {status} with no JSON: {error}"
            ))
        })?;
        let text = |field: &str| body.get(field).and_then(serde_json::Value::as_str);
        if !status.is_success() {
            return Err(match text("error") {
                Some(name) => SponsorAnswer::Refused {
                    name: name.to_string(),
                    detail: text("detail").unwrap_or_default().to_string(),
                },
                None => SponsorAnswer::Unreachable(format!(
                    "the sponsor endpoint {endpoint} answered {status} without naming why: {body}"
                )),
            });
        }
        text("channelId")
            .and_then(|channel| Pubkey::from_str(channel).ok())
            .ok_or_else(|| {
                SponsorAnswer::Unreachable(format!(
                    "the sponsor endpoint {endpoint} answered {status} with no channelId: {body}"
                ))
            })
    }
}

#[async_trait]
impl BatchSettlementPayer for SolanaBatchSettlement {
    /// Build the `open` ADR 0075 decision 3 describes, sign it as payer and
    /// post it to the counterparty's sponsor endpoint, which co-signs,
    /// submits and admits it. The deposit comes from this node's own
    /// associated token account; the rent and the fee are the sponsor's.
    ///
    /// The channel is remembered as this node's **before** the post, and
    /// forgotten only if the chain shows nothing was opened: a sponsor that
    /// submits and then fails to answer may still have made the channel,
    /// with this node's deposit in it.
    async fn open(
        &self,
        terms: ReceiverTerms,
        deposit: u128,
    ) -> Result<OpenedChannel, BatchSettlementError> {
        let presented = terms.chain();
        let ReceiverTerms::Solana(terms) = terms else {
            return Err(BatchSettlementError::WrongChain {
                presented,
                backend: CHAIN,
            });
        };
        if Pubkey::new_from_array(terms.mint) != self.mint {
            return Err(BatchSettlementError::TokenNotShared);
        }
        let deposit_units = u64::try_from(deposit).map_err(|_| {
            BatchSettlementError::Backend(format!(
                "a deposit of {deposit} does not fit payment-channels' u64"
            ))
        })?;
        let grace_period = u32::try_from(terms.min_grace_period_secs).map_err(|_| {
            BatchSettlementError::Backend(format!(
                "a grace period of {}s does not fit payment-channels' u32",
                terms.min_grace_period_secs
            ))
        })?;
        let sponsor = Pubkey::new_from_array(terms.sponsor);
        let receiver = Pubkey::new_from_array(terms.receiver);
        let open_slot = retry_read(|| {
            self.rpc
                .get_slot_with_commitment(CommitmentConfig::confirmed())
        })
        .await
        .map_err(backend_error)?;
        let payer = self.sponsor.pubkey();
        let open = wire::OpenChannel {
            payer,
            rent_payer: sponsor,
            payee: sponsor,
            mint: self.mint,
            token_program: spl_token::id(),
            authorized_signer: payer,
            salt: rand::random(),
            deposit: deposit_units,
            grace_period,
            open_slot,
            recipients: wire::sole_recipient(&receiver).to_vec(),
        };
        let channel = open.channel(&self.program_id);
        let id = ChannelId(channel.to_string());

        self.prefund_channel_rent(&channel).await?;
        let transaction = self.payer_signed_open(&open, &sponsor).await?;
        self.outbound().insert(
            channel,
            Outbound {
                receiver,
                signed: 0,
                backed: deposit,
                finished: None,
            },
        );
        match self
            .post_to_sponsor(&terms.sponsor_endpoint, transaction)
            .await
        {
            Ok(answered) if answered == channel => {}
            Ok(answered) => {
                return Err(BatchSettlementError::Backend(format!(
                    "the sponsor answered with channel {answered}, not the {channel} this open \
                     creates"
                )))
            }
            Err(answer) => {
                return match self.read(&channel).await {
                    Ok(Some(_)) => Err(BatchSettlementError::Backend(format!(
                        "the sponsor endpoint answered {answer}, but channel {channel} exists on \
                         chain; it is recorded as this node's, to withdraw from"
                    ))),
                    Ok(None) => {
                        self.outbound().remove(&channel);
                        Err(answer.into_error())
                    }
                    // Whether it landed cannot be told; the record stays,
                    // costing nothing if it did not.
                    Err(_) => Err(answer.into_error()),
                };
            }
        }

        let account = self.read_existing(&id, &channel).await?;
        if account.payer != payer || account.authorized_signer != payer {
            return Err(BatchSettlementError::Backend(format!(
                "channel {channel} names payer {} and authorized_signer {}, not this node's \
                 settlement key {payer}",
                account.payer, account.authorized_signer
            )));
        }
        Ok(OpenedChannel {
            presentation: ChannelPresentation::Solana { channel: id },
            voucher_signer: VoucherSigner::Solana(account.authorized_signer.to_bytes()),
        })
    }

    /// `top_up`, paid from this node's own token account. The program takes
    /// one only while the channel is Open: a sealed or distributed channel is
    /// [`ChannelSealed`](BatchSettlementError::ChannelSealed), and one this
    /// node has asked to close is refused before anything is sent.
    async fn top_up(
        &self,
        channel: &ChannelId,
        increment: u128,
    ) -> Result<OutboundChannelState, BatchSettlementError> {
        let (address, _) = self.outbound_record(channel)?;
        let account = self
            .read(&address)
            .await?
            .ok_or_else(|| BatchSettlementError::ChannelSealed(channel.clone()))?;
        match account.status {
            wire::ChannelStatus::Open => {}
            wire::ChannelStatus::Closing => {
                return Err(BatchSettlementError::Backend(format!(
                    "batch-settlement channel '{channel}' is closing; payment-channels takes a \
                     top_up only while a channel is open"
                )))
            }
            wire::ChannelStatus::Sealed | wire::ChannelStatus::Distributed => {
                return Err(BatchSettlementError::ChannelSealed(channel.clone()))
            }
        }
        let increment = u64::try_from(increment).map_err(|_| {
            BatchSettlementError::Backend(format!(
                "a top-up of {increment} does not fit payment-channels' u64"
            ))
        })?;
        self.submit(&[wire::top_up_instruction(
            &self.program_id,
            &self.sponsor.pubkey(),
            &address,
            &account.mint,
            &spl_token::id(),
            increment,
        )])
        .await?;
        let state = self.outbound_state(channel).await?;
        self.set_backed(&address, state.on_chain.voucher_ceiling());
        Ok(state)
    }

    /// The 50-byte message over `(channel, cumulative_amount, 0)`, signed by
    /// the settlement key. Answered from this node's own record, with no
    /// chain read: only this node's own top-ups and close move what the
    /// channel backs.
    async fn sign_voucher(
        &self,
        channel: &ChannelId,
        cumulative_amount: u128,
    ) -> Result<Voucher, BatchSettlementError> {
        let address = Pubkey::from_str(&channel.0)
            .map_err(|_| BatchSettlementError::NotOutbound(channel.clone()))?;
        let mut outbound = self.outbound();
        let record = outbound
            .get_mut(&address)
            .ok_or_else(|| BatchSettlementError::NotOutbound(channel.clone()))?;
        if cumulative_amount <= record.signed {
            return Err(BatchSettlementError::VoucherNotAdvancing {
                amount: cumulative_amount,
                signed: record.signed,
            });
        }
        let unbacked = BatchSettlementError::VoucherUnbacked {
            amount: cumulative_amount,
            backed: record.backed,
        };
        if cumulative_amount > record.backed {
            return Err(unbacked);
        }
        let units = u64::try_from(cumulative_amount).map_err(|_| unbacked)?;
        let message = connector_signer::solana_voucher_message(&address.to_bytes(), units, 0);
        let signature = self.sponsor.sign_message(&message);
        record.signed = cumulative_amount;
        Ok(Voucher {
            cumulative_amount,
            signature: signature.as_ref().to_vec(),
        })
    }

    /// `request_close`, starting the grace period in which the receiver may
    /// still land what it holds. Already Closing changes nothing.
    async fn start_withdrawal(
        &self,
        channel: &ChannelId,
    ) -> Result<OutboundChannelState, BatchSettlementError> {
        let (address, _) = self.outbound_record(channel)?;
        let account = self
            .read(&address)
            .await?
            .ok_or_else(|| BatchSettlementError::ChannelSealed(channel.clone()))?;
        match account.status {
            wire::ChannelStatus::Open => {
                self.submit(&[wire::request_close_instruction(
                    &self.program_id,
                    &self.sponsor.pubkey(),
                    &address,
                )])
                .await?;
            }
            wire::ChannelStatus::Closing => {}
            wire::ChannelStatus::Sealed | wire::ChannelStatus::Distributed => {
                return Err(BatchSettlementError::ChannelSealed(channel.clone()))
            }
        }
        let state = self.outbound_state(channel).await?;
        self.set_backed(&address, state.on_chain.voucher_ceiling());
        Ok(state)
    }

    /// `distribute`, once the channel is Sealed -- by the receiver's
    /// `settle_and_seal`, or by the permissionless `seal`, which this sends
    /// first once the grace period has run. An Open channel, and one already
    /// distributed, has no withdrawal to finish.
    async fn finish_withdrawal(
        &self,
        channel: &ChannelId,
    ) -> Result<OutboundChannelState, BatchSettlementError> {
        let (address, record) = self.outbound_record(channel)?;
        let no_withdrawal = || BatchSettlementError::NoWithdrawalPending(channel.clone());
        let account = self.read(&address).await?.ok_or_else(no_withdrawal)?;
        let account = match account.status {
            wire::ChannelStatus::Open | wire::ChannelStatus::Distributed => {
                return Err(no_withdrawal())
            }
            wire::ChannelStatus::Sealed => account,
            wire::ChannelStatus::Closing => {
                let now = self.clock().await?.unix_timestamp;
                let deadline = account
                    .closure_started_at
                    .saturating_add(i64::from(account.grace_period));
                if now < deadline {
                    return Err(BatchSettlementError::WithdrawalNotDue {
                        channel: channel.clone(),
                        remaining_secs: u64::try_from(deadline - now).unwrap_or(u64::MAX),
                    });
                }
                self.submit(&[wire::seal_instruction(&self.program_id, &address)])
                    .await?;
                self.read_existing(channel, &address).await?
            }
        };

        self.distribute(
            &SponsoredChannel {
                address,
                account: account.clone(),
            },
            &record.receiver,
            &Mutex::new(None),
        )
        .await?;

        // Inside its slot window a distributed channel stays allocated,
        // awaiting the receiver's `reclaim`; past it, `distribute`
        // deallocates it at once and there is nothing left to read.
        let on_chain = match self.read(&address).await? {
            Some(after) => state_of(channel, &after),
            None => state_of(
                channel,
                &wire::ChannelAccount {
                    status: wire::ChannelStatus::Distributed,
                    ..account
                },
            ),
        };
        if let Some(record) = self.outbound().get_mut(&address) {
            record.finished = Some(on_chain.clone());
        }
        Ok(OutboundChannelState {
            on_chain,
            signed: record.signed,
        })
    }

    async fn outbound_state(
        &self,
        channel: &ChannelId,
    ) -> Result<OutboundChannelState, BatchSettlementError> {
        let (address, record) = self.outbound_record(channel)?;
        let on_chain = match self.read(&address).await? {
            Some(account) => state_of(channel, &account),
            None => record
                .finished
                .ok_or_else(|| BatchSettlementError::ChannelNotFound(channel.clone()))?,
        };
        Ok(OutboundChannelState {
            on_chain,
            signed: record.signed,
        })
    }
}
