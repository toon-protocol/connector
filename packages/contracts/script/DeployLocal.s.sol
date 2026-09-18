// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "@openzeppelin/contracts/metatx/ERC2771Forwarder.sol";
import "../test/mocks/MockERC20.sol";
import "../src/TokenNetwork.sol";
import "../src/TokenNetworkRegistry.sol";
import "../src/RollingSwapChannel.sol";
import "./Erc2771ForwarderDeployment.sol";

/**
 * @title DeployLocalScript
 * @notice Deploys MockERC20 token, an ERC2771Forwarder, TokenNetworkRegistry and a TokenNetwork
 *         for local testing with Anvil
 * @dev Run with: forge script script/DeployLocal.s.sol --rpc-url http://localhost:8545 --broadcast
 *
 *      The deploy ORDER here is load-bearing twice over:
 *
 *      1. The forwarder is deployed and set on the registry before `createTokenNetwork` (issue
 *         #1261). `TokenNetwork` is `ERC2771Context`, whose forwarder is immutable, so a network
 *         created while the registry's `trustedForwarder` is still `address(0)` can never host a
 *         meta-transaction and can never be repaired -- see Erc2771ForwarderDeployment.
 *      2. The forwarder is inserted AFTER the registry, not before it. Every address this script
 *         lands is `CREATE(deployer, nonce)`, and the `TokenNetwork` is `CREATE(registry, 1)`, so
 *         the registry's own address has to stay put: every `token_network` declaration across
 *         the committed `local/` topology configs hardcodes the network this deploy produces, and
 *         `crates/connector-bin/tests/evm_channel_domain_is_the_deployment.rs` holds them to the
 *         address recorded in regen-anvil-state.sh. Deploying the forwarder first would move the
 *         registry, the network and the mock USDC all at once.
 */
contract DeployLocalScript is Script, Erc2771ForwarderDeployment {
    // Anvil's default accounts (deterministic for testing)
    // Account 0: 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
    // Private key: 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80

    /// @notice Script entrypoint: broadcasts the whole deploy from anvil's account 0.
    function run()
        external
        returns (
            MockERC20 usdcToken,
            ERC2771Forwarder forwarder,
            TokenNetworkRegistry registry,
            TokenNetwork tokenNetwork,
            RollingSwapChannel rollingSwapChannel
        )
    {
        // Use Anvil's first account private key
        uint256 deployerPrivateKey = 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80;
        console.log("Deployer address:", vm.addr(deployerPrivateKey));

        vm.startBroadcast(deployerPrivateKey);
        (usdcToken, forwarder, registry, tokenNetwork, rollingSwapChannel) = deploy();
        vm.stopBroadcast();

        logSummary(usdcToken, forwarder, registry, tokenNetwork, rollingSwapChannel);
    }

    /// @notice The deploy sequence itself, with no broadcast bookkeeping -- callable from run()
    ///         (wrapped in a broadcast) or directly from a test, which is how
    ///         test/DeployLocal.t.sol observes exactly what a broadcast produces. Mirrors
    ///         DeployTestnetCutoverScript's own run()/deploy() split for the same reason.
    function deploy()
        public
        returns (
            MockERC20 usdcToken,
            ERC2771Forwarder forwarder,
            TokenNetworkRegistry registry,
            TokenNetwork tokenNetwork,
            RollingSwapChannel rollingSwapChannel
        )
    {
        // Deploy MockERC20 token (USDC - 6 decimals, matching real USDC and the
        // Solana SPL mint so claim amounts are one scale across chains)
        usdcToken = new MockERC20("USD Coin", "USDC", 6);
        console.log("USDC Token deployed to:", address(usdcToken));

        // Deploy TokenNetworkRegistry. FIRST, ahead of the forwarder: see the address note in
        // this contract's docs -- everything downstream of the registry's nonce is hardcoded in
        // committed configs.
        registry = new TokenNetworkRegistry();
        console.log("TokenNetworkRegistry deployed to:", address(registry));

        // Deploy the ERC-2771 forwarder, trust it on the registry, and only then create the
        // TokenNetwork for USDC -- the immutable-forwarder ordering (issue #1261).
        (forwarder, tokenNetwork) = _createForwarderAwareTokenNetwork(registry, address(usdcToken));
        console.log("ERC2771Forwarder deployed to:", address(forwarder));
        console.log("TokenNetwork created at:", address(tokenNetwork));

        // Deploy the rolling-swap chain-B settlement contract (connector#315),
        // bound to the same USDC token with a 1-day challenge window. This is
        // the production surface the sdk/client `updateBalance` settlement tx
        // redeems against on a swap's destination chain. Deploying it here keeps
        // it in the anvil-state snapshot regenerated per connector#317, so
        // rolling-swap integration tests can settle against a real deployment.
        rollingSwapChannel = new RollingSwapChannel(address(usdcToken), 1 days);
        console.log("RollingSwapChannel deployed to:", address(rollingSwapChannel));

        // Transfer tokens to test peer wallets
        // Anvil test accounts: Account 2 (peer1), Account 3 (peer2)
        address[] memory peerAddresses = new address[](2);
        peerAddresses[0] = 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC; // peer1 (Anvil account 2)
        peerAddresses[1] = 0x90F79bf6EB2c4f870365E785982E1f101E93b906; // peer2 (Anvil account 3)

        uint256 tokensPerPeer = 10000 * 10 ** 6; // 10k USDC each (6 decimals)

        for (uint256 i = 0; i < peerAddresses.length; i++) {
            usdcToken.transfer(peerAddresses[i], tokensPerPeer);
            console.log("Transferred 10k USDC to:", peerAddresses[i]);
        }
    }

    /// @notice Output addresses in a format easy to parse. The forwarder is named here because a
    ///         relayer driving a meta-transaction needs its address and can read it nowhere else.
    function logSummary(
        MockERC20 usdcToken,
        ERC2771Forwarder forwarder,
        TokenNetworkRegistry registry,
        TokenNetwork tokenNetwork,
        RollingSwapChannel rollingSwapChannel
    ) internal pure {
        console.log("");
        console.log("=== DEPLOYMENT COMPLETE ===");
        console.log("USDC_TOKEN_ADDRESS=%s", address(usdcToken));
        console.log("ERC2771_FORWARDER_ADDRESS=%s", address(forwarder));
        console.log("TOKEN_NETWORK_REGISTRY_ADDRESS=%s", address(registry));
        console.log("TOKEN_NETWORK_ADDRESS=%s", address(tokenNetwork));
        console.log("ROLLING_SWAP_CHANNEL_ADDRESS=%s", address(rollingSwapChannel));
    }
}
