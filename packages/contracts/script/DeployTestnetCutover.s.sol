// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "@openzeppelin/contracts/metatx/ERC2771Forwarder.sol";
import "../src/TokenNetwork.sol";
import "../src/TokenNetworkRegistry.sol";
import "./Erc2771ForwarderDeployment.sol";

/**
 * @title DeployTestnetCutoverScript
 * @notice Cuts devnet over to a meta-tx-aware `TokenNetwork` (issue #695, the deploy half of
 *         #694's ERC-2771 support): deploys an `ERC2771Forwarder` and a fresh
 *         `TokenNetworkRegistry` bound to it, then creates a `TokenNetwork` through that
 *         registry for the SAME mock USDC devnet already uses -- so every existing balance and
 *         faucet distribution keeps working, and only the channel contract itself is new.
 * @dev Run with:
 *      forge script script/DeployTestnetCutover.s.sol --rpc-url https://sepolia.base.org --broadcast
 *
 *      Keyless simulation / fork-test (no broadcast, no PRIVATE_KEY required):
 *        forge script script/DeployTestnetCutover.s.sol --fork-url https://sepolia.base.org
 *
 *      run() deliberately does NOT hard-require PRIVATE_KEY at parse time -- mirrors
 *      DeployMainnet.s.sol so this script can be exercised by a Base-Sepolia fork test in CI
 *      with no secrets (see test/DeployTestnetCutover.fork.t.sol).
 *
 * Why a fresh registry rather than reusing the live one: `TokenNetworkRegistry.createTokenNetwork`
 * reverts with `TokenNetworkAlreadyExists` for a token it has already registered
 * (TokenNetworkRegistry.sol), and the live devnet registry already has a `TokenNetwork` for this
 * USDC. `TokenNetwork` itself is not upgradeable (no proxy in this project), so there is no way to
 * swap the live registry's existing mapping in place -- a new registry is the only way to mint a
 * second, forwarder-aware `TokenNetwork` for the same token. Channels on the OLD registry/
 * TokenNetwork are unaffected and keep settling there; only newly opened channels move to the new
 * one. See docs/evm-deployment.md for the full cutover + repoint + rollback runbook.
 *
 * Environment variables (all optional; conservative defaults below apply if unset):
 *   PRIVATE_KEY           - Deployer's private key. If unset, deploy() still runs (simulation
 *                            only, nothing is broadcast even with --broadcast).
 *   EXISTING_USDC_ADDRESS - The devnet mock USDC TokenNetwork channels should keep settling in.
 *                            Defaults to the live Base Sepolia devnet mock USDC recorded in
 *                            packages/contracts/deployments/base-sepolia.md.
 */
contract DeployTestnetCutoverScript is Script, Erc2771ForwarderDeployment {
    /// @notice The live devnet mock USDC (packages/contracts/deployments/base-sepolia.md).
    ///         Reused, never redeployed -- existing balances and faucet distributions must
    ///         survive the cutover unchanged.
    address public constant DEFAULT_EXISTING_USDC = 0x49beE1Bca5d15Fb0963117923403F9498119a9Ce;

    /// @notice Script entrypoint. Broadcasts only if PRIVATE_KEY is set; otherwise runs as a
    ///         keyless simulation so `forge script ... --fork-url` (and fork tests) never need a
    ///         key.
    function run()
        external
        returns (ERC2771Forwarder forwarder, TokenNetworkRegistry registry, TokenNetwork tokenNetwork)
    {
        uint256 deployerPrivateKey = vm.envOr("PRIVATE_KEY", uint256(0));
        bool shouldBroadcast = deployerPrivateKey != 0;

        if (shouldBroadcast) {
            vm.startBroadcast(deployerPrivateKey);
        } else {
            console.log("PRIVATE_KEY not set -- running keyless simulation (no broadcast)");
        }

        (forwarder, registry, tokenNetwork) = deploy();

        if (shouldBroadcast) {
            vm.stopBroadcast();
        }

        logSummary(forwarder, registry, tokenNetwork);
    }

    /// @notice Core deploy logic, env-defaulted via vm.envOr. Reads no PRIVATE_KEY, performs no
    ///         broadcast bookkeeping -- safe to call from run() (wrapped in broadcast) or directly
    ///         from a fork test / a keyless `forge script --fork-url` simulation.
    function deploy()
        public
        returns (ERC2771Forwarder forwarder, TokenNetworkRegistry registry, TokenNetwork tokenNetwork)
    {
        address existingUsdc = vm.envOr("EXISTING_USDC_ADDRESS", DEFAULT_EXISTING_USDC);
        return deploy(existingUsdc);
    }

    /// @notice Parameterized deploy logic underlying deploy(). Exposed directly so tests can
    ///         exercise the constructor-wiring behavior for an overridden token without mutating
    ///         process-wide env vars via vm.setEnv (which is not safe under parallel test runs).
    function deploy(address existingUsdc)
        public
        returns (ERC2771Forwarder forwarder, TokenNetworkRegistry registry, TokenNetwork tokenNetwork)
    {
        registry = new TokenNetworkRegistry();
        (forwarder, tokenNetwork) = _createForwarderAwareTokenNetwork(registry, existingUsdc);
    }

    function logSummary(ERC2771Forwarder forwarder, TokenNetworkRegistry registry, TokenNetwork tokenNetwork)
        internal
        view
    {
        console.log("");
        console.log("=== ERC-2771 CUTOVER DEPLOYMENT COMPLETE (Base Sepolia / chainId 84532) ===");
        console.log("BASE_FORWARDER_ADDRESS=%s", address(forwarder));
        console.log("BASE_REGISTRY_ADDRESS=%s", address(registry));
        console.log("BASE_TOKEN_NETWORK_ADDRESS=%s", address(tokenNetwork));
        console.log("BASE_USDC_TOKEN_ADDRESS=%s (unchanged)", tokenNetwork.token());
        console.log("");
        console.log("NEXT STEPS -- docs/evm-deployment.md carries the checklist; this is the summary:");
        console.log("  1. Repoint [settlement.evm] contract_address to the registry address above in");
        console.log("     BOTH toon-protocol/relay's and toon-protocol/store's own committed configs");
        console.log("     (ADR 0068 -- that is what each box actually runs) AND in this repo's");
        console.log("     infra/linode-store/connector-rust.toml / infra/linode-relay/connector-rust.toml");
        console.log("     fixtures, which devnet_configs_load.rs still asserts against.");
        console.log("  2. Work the rest of the runbook's repoint checklist: test literals, the two");
        console.log("     live-chain workflows, infra/linode-relay/swap.config.json and");
        console.log("     infra/linode/endpoints.json all name the registry or the TokenNetwork too.");
        console.log("     Nothing advertises the address any more -- ADR 0046 removed the announce.");
        console.log("  3. Record the addresses above in packages/contracts/deployments.json and");
        console.log("     packages/contracts/deployments/base-sepolia.md.");
        console.log("  4. Land and apply the config in each node repo FIRST, and only THEN bump that");
        console.log("     repo's own pinned connector tag (ADR 0068). Contract, both box configs and");
        console.log("     the image tag are one matched set once channel ids are derived (ADR 0059).");
        console.log("  5. Rollback: revert contract_address to the OLD registry everywhere above, AND");
        console.log("     roll each node repo's pin back to the last pre-ADR-0059 build. The old");
        console.log("     TokenNetwork is untouched and still settles pre-cutover channels.");
    }
}
