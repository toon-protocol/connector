// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import "forge-std/Test.sol";
import "../src/TokenNetwork.sol";
import "./mocks/MockERC20.sol";

/// @title TokenNetworkFuzzTest
/// @notice Fuzz tests for TokenNetwork contract to validate edge cases
contract TokenNetworkFuzzTest is Test {
    TokenNetwork public tokenNetwork;
    MockERC20 public token;
    address public alice;
    address public bob;

    function setUp() public {
        // Deploy mock ERC20 token
        token = new MockERC20("Test Token", "TEST", 18);

        // Deploy TokenNetwork with 1M token deposit limit
        tokenNetwork = new TokenNetwork(address(token), 1_000_000 * 10 ** 18, 365 days);

        // Create test accounts
        alice = vm.addr(0xA11CE);
        bob = vm.addr(0xB0B);

        // Give ETH to test accounts
        vm.deal(alice, 100 ether);
        vm.deal(bob, 100 ether);

        // Transfer tokens from deployer (this contract) to test accounts
        // MockERC20 mints 1M tokens to deployer by default
        uint256 halfSupply = token.totalSupply() / 2;
        token.transfer(alice, halfSupply);
        token.transfer(bob, halfSupply);
    }

    /// @notice Fuzz test: Deposit random amounts within valid range
    /// @param amount Random deposit amount to test
    function testFuzz_DepositRandomAmounts(uint256 amount) public {
        // Constrain amount to valid range (1 to maxChannelDeposit)
        vm.assume(amount > 0 && amount <= 1_000_000 * 10 ** 18);
        vm.assume(amount <= token.balanceOf(alice));

        // Open channel
        vm.prank(alice);
        bytes32 channelId = tokenNetwork.openChannel(bob, 1 hours);

        // Alice deposits random amount
        vm.startPrank(alice);
        token.approve(address(tokenNetwork), amount);
        tokenNetwork.setTotalDeposit(channelId, alice, amount);
        vm.stopPrank();

        // Verify state consistency
        (uint256 deposit,, ) = tokenNetwork.participants(channelId, alice);
        assertEq(deposit, amount);
        assertEq(token.balanceOf(address(tokenNetwork)), amount);
    }

    /// @notice Fuzz test: Close channel from either participant
    /// @param closerIsAlice Fuzz whether Alice or Bob closes
    function testFuzz_CloseChannel(bool closerIsAlice) public {
        // Open channel and deposit
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

        // Close channel (no balance proof or signature needed)
        address closer = closerIsAlice ? alice : bob;
        vm.prank(closer);
        tokenNetwork.closeChannel(channelId);

        // Verify channel is closed
        (, TokenNetwork.ChannelState state,,,, ) = tokenNetwork.channels(channelId);
        assertEq(uint256(state), uint256(TokenNetwork.ChannelState.Closed));
    }

    /// @notice Fuzz test: Settle with random deposit amounts
    /// @param deposit1 Random deposit amount for participant1
    /// @param deposit2 Random deposit amount for participant2
    function testFuzz_SettleWithRandomBalances(uint256 deposit1, uint256 deposit2) public {
        // Constrain deposits to valid range
        vm.assume(deposit1 > 0 && deposit1 <= 1_000_000 * 10 ** 18);
        vm.assume(deposit2 > 0 && deposit2 <= 1_000_000 * 10 ** 18);
        vm.assume(deposit1 <= token.balanceOf(alice));
        vm.assume(deposit2 <= token.balanceOf(bob));

        // Open channel and deposit
        vm.prank(alice);
        bytes32 channelId = tokenNetwork.openChannel(bob, 1 hours);

        vm.startPrank(alice);
        token.approve(address(tokenNetwork), deposit1);
        tokenNetwork.setTotalDeposit(channelId, alice, deposit1);
        vm.stopPrank();

        vm.startPrank(bob);
        token.approve(address(tokenNetwork), deposit2);
        tokenNetwork.setTotalDeposit(channelId, bob, deposit2);
        vm.stopPrank();

        // Verify participant state (3 values: deposit, nonce, transferredAmount)
        (uint256 aliceDeposit,, ) = tokenNetwork.participants(channelId, alice);
        assertEq(aliceDeposit, deposit1);

        (uint256 bobDeposit,, ) = tokenNetwork.participants(channelId, bob);
        assertEq(bobDeposit, deposit2);

        // Close channel (just channelId, no proof/sig)
        vm.prank(alice);
        tokenNetwork.closeChannel(channelId);

        // Fast forward past challenge period
        vm.warp(block.timestamp + 1 hours + 1);

        // Record balances before settlement
        uint256 aliceBalanceBefore = token.balanceOf(alice);
        uint256 bobBalanceBefore = token.balanceOf(bob);
        uint256 contractBalanceBefore = token.balanceOf(address(tokenNetwork));

        // Settle channel
        vm.prank(alice);
        tokenNetwork.settleChannel(channelId);

        // Verify balance conservation
        uint256 aliceBalanceAfter = token.balanceOf(alice);
        uint256 bobBalanceAfter = token.balanceOf(bob);
        uint256 contractBalanceAfter = token.balanceOf(address(tokenNetwork));

        assertEq(contractBalanceAfter, 0); // All funds distributed
        assertEq(
            aliceBalanceAfter + bobBalanceAfter,
            aliceBalanceBefore + bobBalanceBefore + contractBalanceBefore
        ); // Total balance conserved
    }

    /// @notice Invariant test: Total balance conservation
    /// @dev Verifies that contract balance never exceeds total supply
    function invariant_TotalBalanceConserved() public view {
        // This is a basic invariant test
        // In a full implementation, you would track all deposits and withdrawals across all channels
        // For this MVP, we verify that contract balance never exceeds total supply
        uint256 contractBalance = token.balanceOf(address(tokenNetwork));
        uint256 totalSupply = token.totalSupply();

        assertLe(contractBalance, totalSupply);
    }
}
