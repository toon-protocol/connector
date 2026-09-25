//! The committed x402 ABI describes the committed x402 bytecode (issue #1342,
//! `contracts/x402/PROVENANCE.md`): every function the ABI declares is one the
//! deployed runtime's dispatcher answers. Offline, with no chain and no
//! compiler. The record says why x402 is not rebuilt here.

use std::path::{Path, PathBuf};

use ethers::abi::Abi;

fn x402_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("contracts/x402")
}

fn runtime(name: &str) -> Vec<u8> {
    let path = x402_dir().join(format!("{name}.runtime.hex"));
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let digits = text.trim().trim_start_matches("0x");
    (0..digits.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&digits[i..i + 2], 16).expect("hex"))
        .collect()
}

/// A dispatcher compares the call's selector against each function's with a
/// `PUSH4 <selector>`.
fn dispatches(code: &[u8], selector: [u8; 4]) -> bool {
    code.windows(5)
        .any(|window| window[0] == 0x63 && window[1..] == selector)
}

#[test]
fn every_function_in_the_committed_abi_is_dispatched_by_the_deployed_bytecode() {
    let path = x402_dir().join("x402BatchSettlement.abi.json");
    let abi: Abi = serde_json::from_str(
        &std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display())),
    )
    .expect("the committed ABI parses");
    let code = runtime("x402BatchSettlement");

    let functions: Vec<_> = abi.functions().collect();
    assert!(
        functions.len() >= 20,
        "the ABI is x402BatchSettlement's, not a fragment"
    );
    for function in functions {
        assert!(
            dispatches(&code, function.short_signature()),
            "{} (0x{}) is in the ABI but not in the deployed bytecode",
            function.signature(),
            function
                .short_signature()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
    }
    // And the check can fail: a selector nothing declares is not found.
    assert!(!dispatches(&code, [0xde, 0xad, 0xbe, 0xef]));
}

/// The claim this backend sends is the one the deployed contract takes, by
/// the selector `forge` reported for it at the pin.
#[test]
fn claim_is_the_selector_forge_reported_at_the_pin() {
    let path = x402_dir().join("x402BatchSettlement.abi.json");
    let abi: Abi = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let claim = abi.function("claim").expect("claim");
    assert_eq!(claim.short_signature(), [0x29, 0x23, 0x7b, 0x0c]);
    assert!(dispatches(
        &runtime("x402BatchSettlement"),
        [0x29, 0x23, 0x7b, 0x0c]
    ));
}
