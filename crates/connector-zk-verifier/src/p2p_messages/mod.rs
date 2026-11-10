//! The Mina wire types of a Pickles proof and a verification key, represented in Rust.
//!
//! The rest of upstream's `mina-p2p-messages` (gossip, RPC, blocks, transactions) is not here.

pub mod array;
pub mod bigint;
pub mod char;
pub mod list;
pub mod number;
pub mod pseq;
pub mod string;
pub mod v2;

pub use binprot;
