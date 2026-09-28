//! **Binding an inbound x402 channel to a peer, by its voucher signer**
//! (ADR 0075 decisions 4 and 5, issue #1377).
//!
//! Under ADR 0075 a peering is two one-way x402 `batch-settlement`
//! channels, and the inbound one is **admitted, not configured**: the peer
//! opens it, and this node admits it by exactly the rules it admits a
//! client's by. What makes it a *peer's* channel rather than a client's is
//! only this: its voucher signer -- EVM `payerAuthorizer`, Solana
//! `authorized_signer`, always as the chain records it -- is a key this node
//! has bound to that peer. An interaction then has role `peer` when it
//! carries a voucher on such a channel, or, for a packet that moves no
//! value, the voucher claim-state challenge signed by that signer
//! (`peer-carriage-spec.md` §1.2).
//!
//! # Keyed by signer, not by channel
//!
//! A peer may hold several live channels toward this node (ADR 0075
//! decision 4: "several live channels to one peer are legal"), and none of
//! their ids is known before the peer opens it. The one fact both sides know
//! in advance is the key the peer signs with, which its self-description
//! publishes (#1378) or a config row names (#1380). So the binding is from a
//! signer to a peer, and every channel that signer's vouchers verify on is
//! bound by it.
//!
//! # One signer, one relation
//!
//! A signer bound to two peers would make "which peering does this voucher
//! prove?" depend on iteration order, the failure
//! `ConfigError::PeerChannelDuplicate` exists to keep out of
//! `[[peer_channels]]`. [`VoucherSignerBindings::bind`] therefore refuses a
//! signer already bound to a different peer, by name, rather than
//! re-pointing it.
//!
//! # A runtime operation, and its two sources come later
//!
//! This module is the operation and nothing else. Its callers are the
//! sources ADR 0075 names: the key a peer's self-description publishes,
//! bound when `POST /peers` establishes the peering (#1378, #1379), and the
//! key a `[[peer_channels]]` row names (#1380). Removing a runtime peering
//! unbinds its signers (`Connector::remove_runtime_peer`), because
//! `DELETE /peers` is ADR 0060's kill switch and a switch that left the
//! peer role reachable would not be one.

use std::collections::HashMap;
use std::sync::RwLock;

pub use connector_settlement::batch::VoucherSigner;
use thiserror::Error;

/// Why a voucher signer could not be bound to a peer.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VoucherBindingError {
    /// The signer is already bound to another peering. One signer proves
    /// one relation; see this module's doc.
    #[error("voucher signer {signer} is already bound to peer '{bound_to}'")]
    SignerBoundElsewhere { signer: String, bound_to: String },
    /// No peering by this id exists, in the config file or the runtime
    /// table. A peer role naming a relation this node does not have would
    /// route to nothing.
    #[error("no peering '{0}' exists to bind a voucher signer to")]
    UnknownPeer(String),
}

/// Every voucher signer bound to a peering, and the peering it proves.
///
/// Read on every peer-role decision that presents a voucher or a challenge,
/// and written only by the binding operation and a peering's removal, so it
/// is a plain lock over a small map.
#[derive(Debug, Default)]
pub struct VoucherSignerBindings {
    by_signer: RwLock<HashMap<VoucherSigner, String>>,
}

impl VoucherSignerBindings {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind `signer` to `peer_id`. Binding it again to the same peer is a
    /// no-op; binding it to another is refused.
    ///
    /// # Errors
    ///
    /// [`VoucherBindingError::SignerBoundElsewhere`] when `signer` already
    /// proves a different peering.
    pub fn bind(&self, peer_id: &str, signer: VoucherSigner) -> Result<(), VoucherBindingError> {
        let mut by_signer = self
            .by_signer
            .write()
            .expect("voucher signer bindings lock poisoned");
        match by_signer.get(&signer) {
            Some(bound_to) if bound_to != peer_id => {
                Err(VoucherBindingError::SignerBoundElsewhere {
                    signer: describe(&signer),
                    bound_to: bound_to.clone(),
                })
            }
            Some(_) => Ok(()),
            None => {
                by_signer.insert(signer, peer_id.to_string());
                Ok(())
            }
        }
    }

    /// Unbind every signer bound to `peer_id`.
    pub fn unbind_peer(&self, peer_id: &str) {
        self.by_signer
            .write()
            .expect("voucher signer bindings lock poisoned")
            .retain(|_, bound_to| bound_to != peer_id);
    }

    /// The peering `signer` is bound to, if any.
    #[must_use]
    pub fn peer_for(&self, signer: &VoucherSigner) -> Option<String> {
        self.by_signer
            .read()
            .expect("voucher signer bindings lock poisoned")
            .get(signer)
            .cloned()
    }

    /// Whether no signer is bound at all -- every node before its first
    /// x402 peering, on which no voucher can prove anything but a client.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_signer
            .read()
            .expect("voucher signer bindings lock poisoned")
            .is_empty()
    }
}

/// A signer as an operator reads one: `0x`-hex on EVM, base58 on Solana.
fn describe(signer: &VoucherSigner) -> String {
    match signer {
        VoucherSigner::Evm(address) => format!(
            "0x{}",
            address
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ),
        VoucherSigner::Solana(key) => bs58::encode(key).into_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVM: VoucherSigner = VoucherSigner::Evm([0x11; 20]);
    const SOLANA: VoucherSigner = VoucherSigner::Solana([0x22; 32]);

    #[test]
    fn a_bound_signer_resolves_to_its_peer_and_an_unbound_one_to_none() {
        let bindings = VoucherSignerBindings::new();
        assert!(bindings.is_empty());
        bindings.bind("store", EVM).unwrap();

        assert_eq!(bindings.peer_for(&EVM).as_deref(), Some("store"));
        assert_eq!(bindings.peer_for(&SOLANA), None);
        assert!(!bindings.is_empty());
    }

    /// The same bytes on the other chain are another key: an EVM address and
    /// a Solana public key never stand for each other.
    #[test]
    fn a_signer_is_bound_per_chain() {
        let bindings = VoucherSignerBindings::new();
        bindings
            .bind("store", VoucherSigner::Evm([0x33; 20]))
            .unwrap();

        let mut solana = [0u8; 32];
        solana[..20].copy_from_slice(&[0x33; 20]);
        assert_eq!(bindings.peer_for(&VoucherSigner::Solana(solana)), None);
    }

    #[test]
    fn binding_a_signer_twice_to_its_peer_is_a_no_op() {
        let bindings = VoucherSignerBindings::new();
        bindings.bind("store", EVM).unwrap();
        bindings.bind("store", EVM).unwrap();

        assert_eq!(bindings.peer_for(&EVM).as_deref(), Some("store"));
    }

    #[test]
    fn a_signer_bound_to_one_peer_is_refused_for_another_and_keeps_its_first() {
        let bindings = VoucherSignerBindings::new();
        bindings.bind("store", EVM).unwrap();

        assert_eq!(
            bindings.bind("relay", EVM),
            Err(VoucherBindingError::SignerBoundElsewhere {
                signer: format!("0x{}", "11".repeat(20)),
                bound_to: "store".to_string(),
            })
        );
        assert_eq!(bindings.peer_for(&EVM).as_deref(), Some("store"));
    }

    #[test]
    fn unbinding_a_peer_releases_every_signer_it_held_and_no_other() {
        let bindings = VoucherSignerBindings::new();
        bindings.bind("store", EVM).unwrap();
        bindings.bind("store", SOLANA).unwrap();
        bindings
            .bind("relay", VoucherSigner::Evm([0x44; 20]))
            .unwrap();

        bindings.unbind_peer("store");

        assert_eq!(bindings.peer_for(&EVM), None);
        assert_eq!(bindings.peer_for(&SOLANA), None);
        assert_eq!(
            bindings
                .peer_for(&VoucherSigner::Evm([0x44; 20]))
                .as_deref(),
            Some("relay")
        );
        bindings.bind("relay", EVM).unwrap();
    }
}
