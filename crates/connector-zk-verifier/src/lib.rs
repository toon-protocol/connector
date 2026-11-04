//! Verifies o1js `ZkProgram` proofs (Pickles, side-loaded) under a verification key.
//!
//! The crate is the *verifier subset* of [`toon-protocol/mina-rust`] (Apache-2.0), vendored as
//! source at commit `6feeb3797fff6dba49fd848cb2d6decd63fee1b4` (branch `toon/v0.19.0`), with
//! everything a node needs and a verifier does not removed: see `NOTICE` for what was taken, what
//! was dropped and what was changed. The connector maintains this copy (ADR 0080 decision 15):
//! it is not a mirror and it does not track upstream, so a Mina era change is a change to this
//! crate, reviewed here.
//!
//! The API is three types and two functions. [`VerifierKey::from_o1js_base64`] reads o1js's
//! `verificationKey.data`; [`Verifier::new`] builds the verifier index once (1 to 2 seconds);
//! [`Verifier::verify`] checks one [`WrapProof`] (about 100 ms, CPU-bound and synchronous) against
//! its public input fields. [`fields`] parses the decimal strings o1js prints for those fields.
//!
//! Nothing here reads a file, opens a socket, reads the environment or prints: the structured
//! reference string is derived from its size alone, in process, once.
//!
//! [`toon-protocol/mina-rust`]: https://github.com/toon-protocol/mina-rust

mod ledger;
mod p2p_messages;
mod poseidon;
mod verify;

pub use mina_curves::pasta::Fp;
pub use verify::{fields, Verifier, VerifierError, VerifierKey, WrapProof};
