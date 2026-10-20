//! The state message and its Mina Schnorr signature.
//!
//! The zk edge signs `(vault, commitment x, commitment y, anchor)` once Δ is journaled. Two
//! requirements come from o1js, both pinned by `(r, s)` in `fixtures/parity.json` because a
//! signature made the other way is still *valid* and a test that only verified would not
//! notice (spike #1512, convention 2):
//!
//! - the **chunked/packed nonce derivation** (`NonceMode::Chunked`), which reproduces o1js;
//! - the **devnet domain** (`NetworkId::TESTNET`, prefix `CodaSignature*******`), **even on
//!   mainnet**, because o1js's provable `Signature.verify` accepts nothing else.

use crate::{Field, Point};
use mina_hasher::{Hashable, ROInput};
use mina_signer::{Keypair, NetworkId, NonceMode, PubKey, Signature, Signer};

pub use mina_signer::{
    Keypair as StateKeypair, PubKey as StatePubKey, Signature as StateSignature,
};

/// The signing domain: always the devnet one. See the module docs.
pub const SIGNING_NETWORK: NetworkId = NetworkId::TESTNET;

/// The successor state of a note: what the zk edge signs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateMessage {
    /// The vault, as a field element.
    pub vault: Field,
    /// The successor balance commitment.
    pub commitment: Point,
    /// The successor anchor, `Poseidon(TAG_ANCHOR, N)`.
    pub anchor: Field,
}

impl StateMessage {
    /// The signed fields, in order.
    #[must_use]
    pub fn fields(&self) -> [Field; 4] {
        [
            self.vault,
            self.commitment.x,
            self.commitment.y,
            self.anchor,
        ]
    }
}

impl Hashable for StateMessage {
    type D = NetworkId;

    fn to_roinput(&self) -> ROInput {
        self.fields()
            .into_iter()
            .fold(ROInput::new(), ROInput::append_field)
    }

    fn domain_string(network: NetworkId) -> Option<String> {
        Some(network.into_domain_string())
    }
}

/// Sign `fields` the way o1js's `Signature.create` does.
#[must_use]
pub fn sign_fields(keypair: &Keypair, fields: &[Field]) -> Signature {
    sign_with(keypair, &Fields(fields.to_vec()), NonceMode::Chunked)
}

/// Verify a signature made by [`sign_fields`].
#[must_use]
pub fn verify_fields(signature: &Signature, key: &PubKey, fields: &[Field]) -> bool {
    mina_signer::create_kimchi::<Fields>(SIGNING_NETWORK).verify(
        signature,
        key,
        &Fields(fields.to_vec()),
    )
}

/// Sign a state message.
#[must_use]
pub fn sign_state(keypair: &Keypair, state: &StateMessage) -> Signature {
    sign_with(keypair, state, NonceMode::Chunked)
}

/// Verify a signed state against the zk edge's state public key.
#[must_use]
pub fn verify_state(signature: &Signature, key: &PubKey, state: &StateMessage) -> bool {
    mina_signer::create_kimchi::<StateMessage>(SIGNING_NETWORK).verify(signature, key, state)
}

/// Sign with an explicit nonce mode. Public so a test can show the legacy derivation
/// differs from the pinned `(r, s)`; nothing signs a state with `Legacy`.
#[must_use]
pub fn sign_with<H: Hashable<D = NetworkId> + 'static>(
    keypair: &Keypair,
    message: &H,
    nonce: NonceMode,
) -> Signature {
    mina_signer::create_kimchi::<H>(SIGNING_NETWORK).sign(keypair, message, nonce)
}

#[derive(Clone)]
struct Fields(Vec<Field>);

impl Hashable for Fields {
    type D = NetworkId;

    fn to_roinput(&self) -> ROInput {
        self.0
            .iter()
            .copied()
            .fold(ROInput::new(), ROInput::append_field)
    }

    fn domain_string(network: NetworkId) -> Option<String> {
        Some(network.into_domain_string())
    }
}
