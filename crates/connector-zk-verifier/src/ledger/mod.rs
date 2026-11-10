//! The proof-verification subset of `mina-tree` (see the crate `NOTICE`).

#![allow(
    clippy::type_complexity,
    reason = "upstream spells the Pickles circuit closures as boxed trait objects"
)]

#[macro_use]
mod cache;

pub mod account;
mod hash;
mod lazy;
pub mod proofs;
mod util;
pub mod verifier;

pub use account::*;
pub use hash::*;
pub use util::*;
