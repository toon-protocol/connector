//! The successor state a zk edge signs when an allowance closes (ADR 0080 decisions 2, 4 and 12).
//!
//! Nothing here is derived by the connector: the charge is zk_toon's `balance::charge`, the
//! anchor its `next_anchor`, the message its `state::StateMessage`, and the signature its
//! `state::sign_state` (chunked nonce, devnet domain). This module only composes them, so the
//! gate and the vectors build a successor one way.

use mina_signer::{Keypair, Signature};
use zk_note::balance::{charge, Commitment, Scalar};
use zk_note::state::{sign_state, StateMessage};

use crate::Field;

/// A signed successor state.
#[derive(Debug, Clone)]
pub struct Successor {
    /// `charge(C, Δ) = C − Δ·G`, from the proof's re-randomised commitment `C`.
    pub commitment: Commitment,
    /// `next_anchor(N)`, from the nullifier the allowance spent.
    pub anchor: Field,
    /// The five fields signed: `[state.x, state.isOdd, commitment.x, commitment.y, anchor]`.
    pub message: StateMessage,
    /// The state key's signature over [`Self::message`].
    pub signature: Signature,
}

/// Charge `commitment` by `delta`, advance the anchor past `nullifier`, and sign the result with
/// the zk edge's state key.
///
/// `None` only when the charged commitment is the point at infinity, which zk_toon's message
/// cannot carry: a balance exactly equal to the charge under a zero blinding, which no wallet
/// that blinds its commitments produces.
#[must_use]
pub fn successor(
    state: &Keypair,
    commitment: Commitment,
    nullifier: Field,
    delta: u64,
) -> Option<Successor> {
    use ark_ec::AffineRepr;
    let commitment = charge(commitment, Scalar::from(delta));
    if commitment.is_zero() {
        return None;
    }
    let anchor = zk_note::next_anchor(nullifier);
    let message = StateMessage::new(&state.public.into_compressed(), commitment, anchor);
    let signature = sign_state(state, &message);
    Some(Successor {
        commitment,
        anchor,
        message,
        signature,
    })
}
