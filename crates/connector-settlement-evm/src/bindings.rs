//! Generated typed bindings for `contracts/MockERC20.json` (regenerate with
//! `forge build` against `contracts/MockERC20.sol` and copy the
//! `abi`/`bytecode`/`deployedBytecode` fields back out if it ever changes),
//! plus `TokenNetwork`/`TokenNetworkRegistry` bindings for
//! `packages/contracts/src` (issue #572; regenerate with
//! `contracts/regenerate-token-network-abi.sh`, not by hand -- see that
//! script and `tests/abi_provenance.rs`). `contracts/BYTECODE-PROVENANCE.md`
//! records the check that the deployed Base Sepolia bytecode at both
//! `TokenNetwork` and `TokenNetworkRegistry` addresses matches a local
//! build of this source.
//!
//! `contracts/SettlementChannel.sol` and its bindings are gone (issue
//! #576): this crate drives `TokenNetwork`, reached through
//! `TokenNetworkRegistry`, exclusively now.

use ethers::contract::abigen;

// A mock, mintable ERC-20 this crate's own tests (and any devnet tooling
// standing up a fresh chain with nothing real to point at) deploy in place
// of the real 6-decimal USDC `TokenNetwork` settles in production -- see
// `contracts/MockERC20.sol`.
abigen!(MockErc20, "./contracts/MockERC20.json");

// The minimal ERC-20 surface `EvmSettlementBackend` itself calls against
// whatever token a `TokenNetwork` instance was deployed for -- deliberately
// not tied to the mock's compiled artifact (it carries no `mint`), and
// correct against the real, already-deployed USDC contract production
// points at just as much as against `MockErc20` above.
//
// `decimals()` is here only so `EvmSettlementBackend::connect` can refuse a
// `[settlement] decimals` that the deployed token disagrees with (issue
// #564). It is not part of any value path: every claim in this network is
// denominated in the token's own base units, and every chain settles
// 6-decimal USDC, so nothing scales.
abigen!(
    Erc20,
    r#"[
        function approve(address spender, uint256 amount) external returns (bool)
        function transferFrom(address from, address to, uint256 amount) external returns (bool)
        function transfer(address to, uint256 amount) external returns (bool)
        function balanceOf(address account) external view returns (uint256)
        function decimals() external view returns (uint8)
    ]"#
);

// Bindings for the two contracts issue #566 retargets this crate onto --
// `packages/contracts/src/TokenNetwork.sol` and its
// `TokenNetworkRegistry.sol` factory (issue #572's ABI provenance, issue
// #576's rewrite that actually constructs and calls them).
//
// Each lives in its own `pub(crate)` module rather than at this module's
// top level: `abigen!` also generates an event-filter type per event name,
// flattened into whatever module it's invoked in, and
// `TokenNetwork.sol`/`TokenNetworkRegistry.sol` both inherit OpenZeppelin's
// `Ownable`/`Pausable`, so sharing a module would make
// `OwnershipTransferredFilter`/`PausedFilter`/`UnpausedFilter` ambiguous
// between the two.
//
// The artifacts are NOT hand-committed the way MockERC20.json above is --
// they are extracted verbatim from a real `forge build` of
// `packages/contracts` by `contracts/regenerate-token-network-abi.sh`, and
// `tests/abi_provenance.rs` asserts regenerating them against an unchanged
// `packages/contracts/src` is a no-op. Do not hand-edit
// `contracts/TokenNetwork.json` or `contracts/TokenNetworkRegistry.json`;
// regenerate them with that script instead.
pub(crate) mod token_network {
    use ethers::contract::abigen;

    abigen!(TokenNetwork, "./contracts/TokenNetwork.json");
}

pub(crate) mod token_network_registry {
    use ethers::contract::abigen;

    abigen!(
        TokenNetworkRegistry,
        "./contracts/TokenNetworkRegistry.json"
    );
}

// x402's `x402BatchSettlement` (ADR 0074, issue #1342): the contract a
// client's batch-settlement channel lives in, which this crate only reads
// and `claim`s on. It is not this repository's Solidity, so
// `regenerate-token-network-abi.sh` does not rebuild it: the ABI is the `abi`
// field of a `forge build` of x402 at the pinned commit `0cb1a1f0`.
// `contracts/x402/PROVENANCE.md` records that build and the deployed bytecode
// it matches, and `tests/x402_provenance.rs` holds the ABI to that bytecode.
pub(crate) mod x402_batch_settlement {
    use ethers::contract::abigen;

    abigen!(
        X402BatchSettlement,
        "./contracts/x402/x402BatchSettlement.abi.json"
    );
}
