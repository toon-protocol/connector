//! The successor state a zk edge signs when an allowance closes (ADR 0080 decisions 2, 4 and 12).
//!
//! Nothing here is derived by the connector: the charge is zk_toon's `balance::charge`, the
//! anchor its `next_anchor`, the message its `state::StateMessage`. This module only composes
//! them, so the gate and the vectors build a successor one way. It signs nothing: the state key
//! is `connector-signer`'s (`ZkStateSigner`), which signs [`SuccessorState::message`].

use mina_signer::CompressedPubKey;
use zk_note::balance::{charge, Commitment, Scalar};
use zk_note::state::StateMessage;

use crate::Field;

/// A successor state, before the state key signs it.
#[derive(Debug, Clone)]
pub struct SuccessorState {
    /// `charge(C, Δ) = C − Δ·G`, from the proof's re-randomised commitment `C`.
    pub commitment: Commitment,
    /// `next_anchor(N)`, from the nullifier the allowance spent.
    pub anchor: Field,
    /// The five fields the state key signs: `[state.x, state.isOdd, commitment.x, commitment.y,
    /// anchor]`.
    pub message: StateMessage,
}

/// Charge `commitment` by `delta` and advance the anchor past `nullifier`, under the zk edge's
/// state key `state_key`.
///
/// `None` only when the charged commitment is the point at infinity, which zk_toon's message
/// cannot carry: a balance exactly equal to the charge under a zero blinding, which no wallet
/// that blinds its commitments produces.
#[must_use]
pub fn successor_state(
    state_key: &CompressedPubKey,
    commitment: Commitment,
    nullifier: Field,
    delta: u64,
) -> Option<SuccessorState> {
    use ark_ec::AffineRepr;
    let commitment = charge(commitment, Scalar::from(delta));
    if commitment.is_zero() {
        return None;
    }
    let anchor = zk_note::next_anchor(nullifier);
    let message = StateMessage::new(state_key, commitment, anchor);
    Some(SuccessorState {
        commitment,
        anchor,
        message,
    })
}
