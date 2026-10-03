//! `connector probe` -- learn what a path costs (ADR 0011).
//!
//! A probe is not a packet type and has no endpoint of its own: it is an
//! ordinary packet, sent in the expectation of a reject, through the same
//! `POST /packets` that `connector send` uses. So this module forms nothing
//! and signs nothing -- it calls [`crate::send::send`] and reads the answer
//! for what it says about *cost*.
//!
//! # The three answers, and the fourth that is not one
//!
//! * A reject other than `R01` carries the cost of the whole path the packet
//!   travelled: that is what a packet of this size must carry to be
//!   delivered.
//! * An `R01` is a **partial** sum (ADR 0011, #1467): the probe stopped at a
//!   hop whose fee exceeded the amount, and the figure is the least amount
//!   that gets a packet past it. Probe again with that amount to read on.
//! * A fulfil means the amount covered the path and the packet was delivered
//!   and paid for.
//!
//! A reject that says nothing about cost -- no route (`F02`), the packet out
//! of time (`R00`), this node's own fault (`T00`), a peer or app that did not
//! answer (`T01`), a rate limit (`T05`) -- is a failure: the figure on it is
//! the fees of a path that did not reach where it was going.

use crate::send::{self, Outcome, SendError, SendOptions};

/// The reject codes whose accumulated cost is not an answer, because the
/// path did not exist or did not answer (ADR 0051).
const NO_ANSWER_CODES: [&str; 5] = ["F02", "R00", "T00", "T01", "T05"];

/// What a probe learned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// A reject other than `R01`: `cost` is the whole path's.
    Complete {
        code: String,
        message: String,
        cost: u64,
    },
    /// An `R01`: `cost` is the amount to carry to get past the refusing hop.
    Partial {
        code: String,
        message: String,
        cost: u64,
    },
    /// The amount covered the path and the packet was delivered and paid for.
    Delivered { status: u16, body: Vec<u8> },
}

/// A finished probe, with what it was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReport {
    pub destination: String,
    pub amount: u64,
    pub finding: Finding,
}

#[derive(Debug)]
pub enum ProbeError {
    /// The packet could not be formed, sent or decoded.
    Send(SendError),
    /// A reject that says nothing about cost.
    NoAnswer {
        code: String,
        message: String,
        cost: u64,
    },
    /// A fulfil whose fulfilment is not the one this probe's wrap derives.
    WrongFulfillment,
    /// `--dry-run` reached a probe. Not offered on the command line; kept so
    /// the match over [`Outcome`] is exhaustive without a panic.
    NotSent,
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeError::Send(source) => write!(f, "{source}"),
            ProbeError::NoAnswer {
                code,
                message,
                cost,
            } => write!(
                f,
                "the probe learned no cost: REJECT {code} -- {message}. The path does not exist \
                 or did not answer, so the {cost} base units on the reject are not what it \
                 costs."
            ),
            ProbeError::WrongFulfillment => write!(
                f,
                "the probe was fulfilled, but not by the node --seal-to names: the fulfilment \
                 does not match the one this probe's gift wrap derives (ADR 0019)"
            ),
            ProbeError::NotSent => write!(f, "nothing was sent"),
        }
    }
}

impl std::error::Error for ProbeError {}

impl From<SendError> for ProbeError {
    fn from(source: SendError) -> Self {
        ProbeError::Send(source)
    }
}

/// Read what a send's outcome says about cost.
pub fn classify(outcome: Outcome) -> Result<Finding, ProbeError> {
    match outcome {
        Outcome::Fulfilled { status, body } => Ok(Finding::Delivered { status, body }),
        Outcome::FulfilledWithWrongFulfillment => Err(ProbeError::WrongFulfillment),
        Outcome::NotSent => Err(ProbeError::NotSent),
        Outcome::Rejected {
            code,
            message,
            accumulated_cost: cost,
        } => {
            if NO_ANSWER_CODES.contains(&code.as_str()) {
                Err(ProbeError::NoAnswer {
                    code,
                    message,
                    cost,
                })
            } else if code == "R01" {
                Ok(Finding::Partial {
                    code,
                    message,
                    cost,
                })
            } else {
                Ok(Finding::Complete {
                    code,
                    message,
                    cost,
                })
            }
        }
    }
}

/// Send the probe packet and read its answer.
pub async fn probe(options: &SendOptions) -> Result<ProbeReport, ProbeError> {
    let sent = send::send(options).await?;
    Ok(ProbeReport {
        destination: sent.destination,
        amount: sent.amount,
        finding: classify(sent.outcome)?,
    })
}

/// The report in plain words.
pub fn describe(report: &ProbeReport) -> String {
    let to = format!("{} base units to {}", report.amount, report.destination);
    match &report.finding {
        Finding::Complete {
            code,
            message,
            cost,
        } => format!(
            "COST {cost} -- a packet of this size to {} must carry {cost} base units to be \
             delivered.\nThe probe ({to}) was rejected {code}: {message}",
            report.destination
        ),
        Finding::Partial {
            code,
            message,
            cost,
        } => format!(
            "PARTIAL COST {cost} -- the probe ({to}) stopped at a hop it could not pay ({code}: \
             {message}). The sum is incomplete: carry at least {cost} base units to get past \
             that hop, then probe again with --amount {cost} to read the rest."
        ),
        Finding::Delivered { status, body } => format!(
            "DELIVERED -- the probe ({to}) covered the path's cost, so it was delivered and \
             {} base units were paid.\nthe terminating app answered {status}:\n{}",
            report.amount,
            String::from_utf8_lossy(body)
        ),
    }
}

/// The report as one JSON object: `outcome`, and the reject's `code`,
/// `message` and `accumulatedCost` when there was one.
pub fn describe_json(report: &ProbeReport) -> String {
    let value = match &report.finding {
        Finding::Complete {
            code,
            message,
            cost,
        }
        | Finding::Partial {
            code,
            message,
            cost,
        } => {
            let outcome = if code == "R01" { "partial" } else { "complete" };
            serde_json::json!({
                "outcome": outcome,
                "destination": report.destination,
                "amount": report.amount,
                "code": code,
                "message": message,
                "accumulatedCost": cost,
            })
        }
        Finding::Delivered { status, body } => serde_json::json!({
            "outcome": "delivered",
            "destination": report.destination,
            "amount": report.amount,
            "paid": report.amount,
            "status": status,
            "body": String::from_utf8_lossy(body),
        }),
    };
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejected(code: &str, cost: u64) -> Outcome {
        Outcome::Rejected {
            code: code.to_string(),
            message: "m".to_string(),
            accumulated_cost: cost,
        }
    }

    #[test]
    fn a_reject_other_than_r01_is_a_complete_sum() {
        for code in ["F03", "F06", "T04"] {
            assert!(
                matches!(
                    classify(rejected(code, 9)),
                    Ok(Finding::Complete { cost: 9, .. })
                ),
                "{code}"
            );
        }
    }

    #[test]
    fn an_r01_is_a_partial_sum() {
        assert!(matches!(
            classify(rejected("R01", 4)),
            Ok(Finding::Partial { cost: 4, .. })
        ));
    }

    #[test]
    fn a_reject_that_says_nothing_about_cost_is_a_failure() {
        for code in NO_ANSWER_CODES {
            assert!(
                matches!(
                    classify(rejected(code, 3)),
                    Err(ProbeError::NoAnswer { .. })
                ),
                "{code}"
            );
        }
    }

    #[test]
    fn a_fulfil_is_delivered_and_a_wrong_fulfilment_is_not() {
        assert!(matches!(
            classify(Outcome::Fulfilled {
                status: 200,
                body: vec![]
            }),
            Ok(Finding::Delivered { status: 200, .. })
        ));
        assert!(matches!(
            classify(Outcome::FulfilledWithWrongFulfillment),
            Err(ProbeError::WrongFulfillment)
        ));
    }

    #[test]
    fn the_text_states_the_cost_and_the_json_carries_it() {
        let report = ProbeReport {
            destination: "g.a".to_string(),
            amount: 0,
            finding: classify(rejected("F03", 37)).unwrap(),
        };
        assert!(describe(&report).contains("must carry 37 base units"));
        let json: serde_json::Value = serde_json::from_str(&describe_json(&report)).unwrap();
        assert_eq!(json["code"], "F03");
        assert_eq!(json["accumulatedCost"], 37);
        assert_eq!(json["outcome"], "complete");

        let partial = ProbeReport {
            finding: classify(rejected("R01", 5)).unwrap(),
            ..report
        };
        assert!(describe(&partial).contains("PARTIAL"));
        assert!(describe(&partial).contains("--amount 5"));
    }
}
