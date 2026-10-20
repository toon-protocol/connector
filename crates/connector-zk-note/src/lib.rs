//! Zk-note primitives (ADR 0080): everything the payer, the zk edge and the challenger must
//! agree on byte for byte.
//!
//! Pure: no I/O, no clock, and no dependency on any connector crate. Every value in here is
//! pinned against vectors o1js produced (`fixtures/parity.json`, spike #1512). The
//! proof-systems crates are pinned by git `rev` to the commit the live Mina daemon
//! `4.0.0-mainnet-mesa` pins.
//!
//! - [`poseidon`]: the kimchi Poseidon over the Pallas base field, and `hashWithPrefix`.
//! - [`tags`]: the domain tags and generator seeds, listed once. Wire law.
//! - [`group_map`]: `hashToGroup` through Snarky's conic group map.
//! - [`commitment`]: the generators and the balance commitment's arithmetic.
//! - [`derive`]: the note commitment, leaf, nullifier, binding and successor anchor.
//! - [`tiers`]: the published set of budgets.
//! - [`state`]: the state message and its Mina Schnorr signature.

pub mod commitment;
pub mod derive;
pub mod group_map;
pub mod poseidon;
pub mod state;
pub mod tags;
pub mod tiers;

/// The Pallas base field, the field every value in a zk note lives in.
pub use mina_curves::pasta::Fp as Field;
/// The Pallas scalar field, in which a balance and a blinding factor live.
pub use mina_curves::pasta::Fq as Scalar;
/// A point on Pallas, in affine form.
pub use mina_curves::pasta::Pallas as Point;
