//! Typed bindings for the contracts this crate calls: x402's
//! `x402BatchSettlement`, and the few functions it asks of a token.
//!
//! TOON's `TokenNetwork`/`TokenNetworkRegistry` bindings, their committed
//! ABI and its `abi_provenance` check are deleted with TOON's channels (ADR
//! 0075 decision 12, issue #1385).

use ethers::contract::abigen;

// `decimals()` is here only so `EvmBatchSettlementBackend::connect` can
// refuse a `[settlement.evm] decimals` that the deployed token disagrees
// with (issue #564). It is not part of any value path: every amount is in
// the token's own base units, so nothing scales.
abigen!(
    Erc20,
    r#"[
        function decimals() external view returns (uint8)
    ]"#
);

// x402's `x402BatchSettlement` (ADR 0074, issue #1342): the contract every
// channel lives in. It is not this repository's Solidity: the ABI is the
// `abi` field of a `forge build` of x402 at the pinned commit `0cb1a1f0`.
// `contracts/x402/PROVENANCE.md` records that build and the deployed bytecode
// it matches, and `tests/x402_provenance.rs` holds the ABI to that bytecode.
pub(crate) mod x402_batch_settlement {
    use ethers::contract::abigen;

    abigen!(
        X402BatchSettlement,
        "./contracts/x402/x402BatchSettlement.abi.json"
    );
}

// What the paying half of the batch-settlement port (ADR 0075 decision 3,
// issue #1374) asks of the token it deposits, and of Permit2, and nothing
// more. `authorizationState` is EIP-3009's own view function: a token that
// answers it takes deposits through `ERC3009DepositCollector`, and one whose
// dispatcher reverts on it takes them through `Permit2DepositCollector`.
// `DOMAIN_SEPARATOR` is read from the token and from Permit2 alike, so
// neither EIP-712 domain is rebuilt here from anything a config says.
pub(crate) mod deposit {
    use ethers::contract::abigen;

    abigen!(
        DepositToken,
        r#"[
            function authorizationState(address authorizer, bytes32 nonce) external view returns (bool)
            function DOMAIN_SEPARATOR() external view returns (bytes32)
            function allowance(address owner, address spender) external view returns (uint256)
            function approve(address spender, uint256 amount) external returns (bool)
        ]"#
    );
}
