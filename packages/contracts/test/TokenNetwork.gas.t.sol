// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import "forge-std/Test.sol";
import "../src/TokenNetwork.sol";
import "../src/TokenNetworkRegistry.sol";
import "./mocks/MockERC20.sol";

/// @title TokenNetworkGasTest
/// @notice Gas benchmarking tests for TokenNetwork operations
/// @dev Measures gas costs against targets from Story 8.6 AC5
contract TokenNetworkGasTest is Test {
    TokenNetworkRegistry public registry;
    TokenNetwork public tokenNetwork;
    MockERC20 public token;

    address public alice;
    address public bob;

    // Gas targets from Story 8.6 AC5
    uint256 constant TARGET_OPEN_CHANNEL = 155_000;
    uint256 constant TARGET_DEPOSIT = 80_000;
    uint256 constant TARGET_CLOSE_CHANNEL = 100_000;
    uint256 constant TARGET_SETTLE_CHANNEL = 80_000;

    function setUp() public {
        // Deploy TokenNetworkRegistry
        registry = new TokenNetworkRegistry();

        // Deploy mock ERC20 token
        token = new MockERC20("Test Token", "TEST", 18);

        // Create TokenNetwork via registry
        address tokenNetworkAddress = registry.createTokenNetwork(address(token));
        tokenNetwork = TokenNetwork(tokenNetworkAddress);

        // Create test accounts
        alice = vm.addr(0xA11CE);
        bob = vm.addr(0xB0B);

        // Fund test accounts
        vm.deal(alice, 100 ether);
        vm.deal(bob, 100 ether);

        // Mint tokens to test accounts
        token.transfer(alice, 100000 * 10 ** 18);
        token.transfer(bob, 100000 * 10 ** 18);
    }

    /// @notice Gas benchmark: openChannel operation
    /// @dev Target: <150k gas
    function testGas_OpenChannel() public {
        vm.prank(alice);
        uint256 gasBefore = gasleft();
        bytes32 channelId = tokenNetwork.openChannel(bob, 1 hours);
        uint256 gasUsed = gasBefore - gasleft();

        console.log("Gas used for openChannel:", gasUsed);
        assertTrue(channelId != bytes32(0), "Channel should be created");
        assertLt(gasUsed, TARGET_OPEN_CHANNEL, "openChannel gas cost exceeds target");
    }

    /// @notice Gas benchmark: setTotalDeposit operation
    /// @dev Target: <80k gas
    function testGas_Deposit() public {
        // Setup: Open channel
        vm.prank(alice);
        bytes32 channelId = tokenNetwork.openChannel(bob, 1 hours);

        // Measure deposit gas cost
        vm.startPrank(alice);
        token.approve(address(tokenNetwork), 1000 * 10 ** 18);
        uint256 gasBefore = gasleft();
        tokenNetwork.setTotalDeposit(channelId, alice, 1000 * 10 ** 18);
        uint256 gasUsed = gasBefore - gasleft();
        vm.stopPrank();

        console.log("Gas used for setTotalDeposit:", gasUsed);
        assertLt(gasUsed, TARGET_DEPOSIT, "setTotalDeposit gas cost exceeds target");
    }

    /// @notice Gas benchmark: closeChannel operation
    /// @dev Target: <100k gas
    function testGas_CloseChannel() public {
        // Setup: Open channel and deposit
        vm.prank(alice);
        bytes32 channelId = tokenNetwork.openChannel(bob, 1 hours);

        vm.startPrank(alice);
        token.approve(address(tokenNetwork), 1000 * 10 ** 18);
        tokenNetwork.setTotalDeposit(channelId, alice, 1000 * 10 ** 18);
        vm.stopPrank();

        // Measure close gas cost (just channelId, no proof/sig)
        vm.prank(bob);
        uint256 gasBefore = gasleft();
        tokenNetwork.closeChannel(channelId);
        uint256 gasUsed = gasBefore - gasleft();

        console.log("Gas used for closeChannel:", gasUsed);
        assertLt(gasUsed, TARGET_CLOSE_CHANNEL, "closeChannel gas cost exceeds target");
    }

    /// @notice Gas benchmark: settleChannel operation
    /// @dev Target: <80k gas
    function testGas_SettleChannel() public {
        // Setup: Open, deposit, close channel
        vm.prank(alice);
        bytes32 channelId = tokenNetwork.openChannel(bob, 1 hours);

        vm.startPrank(alice);
        token.approve(address(tokenNetwork), 1000 * 10 ** 18);
        tokenNetwork.setTotalDeposit(channelId, alice, 1000 * 10 ** 18);
        vm.stopPrank();

        vm.startPrank(bob);
        token.approve(address(tokenNetwork), 1000 * 10 ** 18);
        tokenNetwork.setTotalDeposit(channelId, bob, 1000 * 10 ** 18);
        vm.stopPrank();

        // Close channel (just channelId)
        vm.prank(bob);
        tokenNetwork.closeChannel(channelId);

        // Fast forward past challenge period
        vm.warp(block.timestamp + 1 hours + 1);

        // Measure settle gas cost
        uint256 gasBefore = gasleft();
        tokenNetwork.settleChannel(channelId);
        uint256 gasUsed = gasBefore - gasleft();

        console.log("Gas used for settleChannel:", gasUsed);
        assertLt(gasUsed, TARGET_SETTLE_CHANNEL, "settleChannel gas cost exceeds target");
    }

}
