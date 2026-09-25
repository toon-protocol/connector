use std::sync::RwLock;

use super::port::{ChannelPresentation, Voucher};

/// The latest voucher this node holds on one batch-settlement channel, with
/// what it takes to find that channel on chain again: what a watcher or
/// sweep lands (ADR 0074 decision 5, issue #1344).
///
/// The presentation is the one the channel was admitted under -- on EVM it
/// carries the `ChannelConfig`, which the chain never gives back and `claim`
/// needs -- so a watcher can land on a channel its backend has not admitted
/// in this process, such as one whose payer began to exit while the node
/// was down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldVoucher {
    pub presentation: ChannelPresentation,
    pub voucher: Voucher,
}

/// Where the watchers and sweeps read the vouchers this node holds (ADR 0074
/// decision 5). In a running node that is the client edge's claim gate, the
/// one place a voucher is accepted and journaled; the watchers only read it.
///
/// Synchronous on purpose: the answer is this process's own record, never a
/// chain read, so a sweep that asks costs no I/O.
pub trait HeldVouchers: Send + Sync {
    /// One entry per channel, carrying that channel's highest accepted
    /// voucher. A channel with no accepted voucher is absent.
    fn held_vouchers(&self) -> Vec<HeldVoucher>;
}

/// A plain list is a [`HeldVouchers`]: what a test hands a watcher in place
/// of the claim gate, and changes as its client pays.
impl HeldVouchers for RwLock<Vec<HeldVoucher>> {
    fn held_vouchers(&self) -> Vec<HeldVoucher> {
        self.read().expect("held vouchers lock poisoned").clone()
    }
}
