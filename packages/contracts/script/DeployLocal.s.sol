// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../test/mocks/MockERC20.sol";
import "../src/TokenNetwork.sol";
import "../src/TokenNetworkRegistry.sol";

/**
 * @title DeployLocalScript
 * @notice Deploys MockERC20 token, TokenNetworkRegistry, and TokenNetwork for local testing with Anvil
 * @dev Run with: forge script script/DeployLocal.s.sol --rpc-url http://localhost:8545 --broadcast
 */
contract DeployLocalScript is Script {
    // Anvil's default accounts (deterministic for testing)
    // Account 0: 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
    // Private key: 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80

    function run() external {
        // Use Anvil's first account private key
        uint256 deployerPrivateKey = 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80;
        address deployer = vm.addr(deployerPrivateKey);

        console.log("Deployer address:", deployer);

        vm.startBroadcast(deployerPrivateKey);

        // Deploy MockERC20 token (USDC token - 18 decimals for local testing)
        MockERC20 usdcToken = new MockERC20("USD Coin", "USDC", 18);
        console.log("USDC Token deployed to:", address(usdcToken));

        // Deploy TokenNetworkRegistry
        TokenNetworkRegistry registry = new TokenNetworkRegistry();
        console.log("TokenNetworkRegistry deployed to:", address(registry));

        // Create TokenNetwork for USDC token through the registry
        address tokenNetworkAddress = registry.createTokenNetwork(address(usdcToken));
        console.log("TokenNetwork created at:", tokenNetworkAddress);

        // Transfer tokens to test peer wallets
        // Anvil test accounts: Account 2 (peer1), Account 3 (peer2)
        address[] memory peerAddresses = new address[](2);
        peerAddresses[0] = 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC; // peer1 (Anvil account 2)
        peerAddresses[1] = 0x90F79bf6EB2c4f870365E785982E1f101E93b906; // peer2 (Anvil account 3)

        uint256 tokensPerPeer = 10000 * 10**18; // 10k tokens each

        for (uint i = 0; i < peerAddresses.length; i++) {
            usdcToken.transfer(peerAddresses[i], tokensPerPeer);
            console.log("Transferred 10k USDC to:", peerAddresses[i]);
        }

        vm.stopBroadcast();

        // Output addresses in format easy to parse
        console.log("");
        console.log("=== DEPLOYMENT COMPLETE ===");
        console.log("USDC_TOKEN_ADDRESS=%s", address(usdcToken));
        console.log("TOKEN_NETWORK_REGISTRY_ADDRESS=%s", address(registry));
        console.log("TOKEN_NETWORK_ADDRESS=%s", tokenNetworkAddress);
    }
}
