//! XRPL settlement (map #1563, spec #1596).
//!
//! Today this crate holds the test chain only: `test_support::Xrpld`, a
//! disposable stand-alone `xrpld` in a container, the repository's first
//! container-backed tier-3 chain (ADR 0084). The backend itself lands in
//! later tickets of the spec.

#[cfg(feature = "test-util")]
pub mod test_support;
