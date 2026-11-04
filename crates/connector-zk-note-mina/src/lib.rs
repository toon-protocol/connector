//! The zk edge's Mina vault adapter (ADR 0080 decision 15, issue #1514).
//!
//! [`MinaZkNoteVault`] is the [`ZkNoteVault`](connector_client_edge::zk_note::ZkNoteVault)
//! that reads a real vault. It proves nothing and sends no Mina transaction;
//! every operation is a read:
//!
//! - **`verify_request_proof`**: Pickles verification under the request key
//!   this build carries ([`key`]), on the blocking pool, bounded.
//! - **`root_known`**, **`creation_slot`**, **`slot_anchor`**: the daemon's
//!   account read (`graphql_url`).
//! - **`nullifier_used`**: the vault's actions from the archive node
//!   (`archive_url`), checked against the account's latest action state
//!   ([`actions`]).
//!
//! Both endpoints are one [`GraphqlRpc`](connector_chain_rpc::graphql::GraphqlRpc)
//! over a table-built [`RpcTransport`](connector_chain_rpc::RpcTransport) on the
//! `toon-zk-note-mina` circuit. A failed read is
//! [`ZkNoteVaultUnavailable`](connector_client_edge::zk_note::ZkNoteVaultUnavailable),
//! which refuses that one allowance by name.

pub mod actions;
pub mod key;
mod vault;

pub use key::{carried_key_hash, request_verifier, KeyRefusal};
pub use vault::MinaZkNoteVault;
