//! The reject-code rule, through the crate's public path as a dependent crate
//! would call it.

use connector_cli::{reject_cost_reading, CostReading};

#[test]
fn r01_is_partial() {
    assert_eq!(reject_cost_reading("R01"), CostReading::Partial);
}

#[test]
fn codes_that_state_no_cost_are_no_answer() {
    for code in ["F00", "F01", "F02", "R00", "T00", "T01", "T05"] {
        assert_eq!(reject_cost_reading(code), CostReading::NoAnswer, "{code}");
    }
}

#[test]
fn any_other_code_is_complete() {
    for code in ["T04", "F99", "F06", "", "r01"] {
        assert_eq!(reject_cost_reading(code), CostReading::Complete, "{code}");
    }
}
