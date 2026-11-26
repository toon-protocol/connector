use thiserror::Error;

/// An ILP address a table names that does not parse. Raised by both `node.rs` and
/// `route.rs`, so it belongs to neither (issue #1727).
#[derive(Debug, Error)]
pub enum AddressRefusal {
    #[error("invalid {field} '{value}': not a valid ILP address")]
    InvalidAddress { field: &'static str, value: String },
}
