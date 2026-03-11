// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import "forge-std/Test.sol";
import "../src/TokenNetwork.sol";
import "../src/TokenNetworkRegistry.sol";
import "./mocks/MockERC20.sol";

/// @title TokenNetworkIntegrationTest
/// @notice Integration tests for TokenNetwork payment channels
/// @dev Tests multi-channel scenarios, multi-token networks, and full lifecycle flows
contract TokenNetworkIntegrationTest is Test {
    TokenNetworkRegistry public registry;
    TokenNetwork public tokenNetwork;
    MockERC20 public token;

    address public alice;
    address public bob;
    address public charlie;

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
        charlie = vm.addr(0xC0C);

        // Fund test accounts
        vm.deal(alice, 100 ether);
        vm.deal(bob, 100 ether);
        vm.deal(charlie, 100 ether);

        // Mint tokens to test accounts
        token.transfer(alice, 100000 * 10 ** 18);
        token.transfer(bob, 100000 * 10 ** 18);
        token.transfer(charlie, 100000 * 10 ** 18);
    }

    /// @notice Integration Test: Multi-channel scenario with 3 participants
    /// @dev Tests concurrent channels: Alice-Bob, Bob-Charlie, Alice-Charlie
    function testIntegration_MultiChannelScenario() public {
        // Scenario: 3 participants (Alice, Bob, Charlie) open 3 channels
        // Alice-Bob, Bob-Charlie, Alice-Charlie
        // Each channel has deposits, transfers, and settles correctly

        // ===== Open 3 Channels =====
        vm.prank(alice);
        bytes32 channelAB = tokenNetwork.openChannel(bob, 1 hours);

        vm.prank(bob);
        bytes32 channelBC = tokenNetwork.openChannel(charlie, 1 hours);

        vm.prank(alice);
        bytes32 channelAC = tokenNetwork.openChannel(charlie, 1 hours);

        // Assert: All channels opened
        assertTrue(channelAB != bytes32(0), "Channel Alice-Bob should be created");
        assertTrue(channelBC != bytes32(0), "Channel Bob-Charlie should be created");
        assertTrue(channelAC != bytes32(0), "Channel Alice-Charlie should be created");

        // ===== Deposit to All Channels =====
        // Channel Alice-Bob: 1000 each
        vm.startPrank(alice);
        token.approve(address(tokenNetwork), 1000 * 10 ** 18);
        tokenNetwork.setTotalDeposit(channelAB, alice, 1000 * 10 ** 18);
        vm.stopPrank();

        vm.startPrank(bob);
        token.approve(address(tokenNetwork), 2000 * 10 ** 18); // Bob deposits to 2 channels
        tokenNetwork.setTotalDeposit(channelAB, bob, 1000 * 10 ** 18);
        vm.stopPrank();

        // Channel Bob-Charlie: 1000 each
        vm.startPrank(bob);
        tokenNetwork.setTotalDeposit(channelBC, bob, 1000 * 10 ** 18);
        vm.stopPrank();

        vm.startPrank(charlie);
        token.approve(address(tokenNetwork), 2000 * 10 ** 18); // Charlie deposits to 2 channels
        tokenNetwork.setTotalDeposit(channelBC, charlie, 1000 * 10 ** 18);
        vm.stopPrank();

        // Channel Alice-Charlie: 1000 each
        vm.startPrank(alice);
        token.approve(address(tokenNetwork), 1000 * 10 ** 18);
        tokenNetwork.setTotalDeposit(channelAC, alice, 1000 * 10 ** 18);
        vm.stopPrank();

        vm.startPrank(charlie);
        tokenNetwork.setTotalDeposit(channelAC, charlie, 1000 * 10 ** 18);
        vm.stopPrank();

        // ===== Close All Channels =====
        vm.prank(bob);
        tokenNetwork.closeChannel(channelAB);

        vm.prank(charlie);
        tokenNetwork.closeChannel(channelBC);

        vm.prank(charlie);
        tokenNetwork.closeChannel(channelAC);

        // ===== Wait for Challenge Period =====
        vm.warp(block.timestamp + 1 hours + 1);

        // Record balances before settlement
        uint256 aliceBalanceBefore = token.balanceOf(alice);
        uint256 bobBalanceBefore = token.balanceOf(bob);
        uint256 charlieBalanceBefore = token.balanceOf(charlie);

        // ===== Settle All Channels =====
        tokenNetwork.settleChannel(channelAB);
        tokenNetwork.settleChannel(channelBC);
        tokenNetwork.settleChannel(channelAC);

        // ===== Validate Final Balances =====
        // Without balance proofs at close, settlement returns deposits to each participant
        // Channel Alice-Bob: Alice deposited 1000, Bob deposited 1000 → each gets own deposit back
        // Channel Bob-Charlie: Bob deposited 1000, Charlie deposited 1000 → each gets own deposit back
        // Channel Alice-Charlie: Alice deposited 1000, Charlie deposited 1000 → each gets own deposit back

        uint256 aliceBalanceAfter = token.balanceOf(alice);
        uint256 bobBalanceAfter = token.balanceOf(bob);
        uint256 charlieBalanceAfter = token.balanceOf(charlie);

        // Alice: +1000 (from Alice-Bob) + 1000 (from Alice-Charlie) = +2000
        assertEq(aliceBalanceAfter, aliceBalanceBefore + 2000 * 10 ** 18, "Alice balance should increase by 2000");

        // Bob: +1000 (from Alice-Bob) + 1000 (from Bob-Charlie) = +2000
        assertEq(bobBalanceAfter, bobBalanceBefore + 2000 * 10 ** 18, "Bob balance should increase by 2000");

        // Charlie: +1000 (from Bob-Charlie) + 1000 (from Alice-Charlie) = +2000
        assertEq(
            charlieBalanceAfter, charlieBalanceBefore + 2000 * 10 ** 18, "Charlie balance should increase by 2000"
        );

        // ===== Validate All Channels Settled =====
        (, TokenNetwork.ChannelState stateAB,,,,) = tokenNetwork.channels(channelAB);
        assertEq(uint256(stateAB), uint256(TokenNetwork.ChannelState.Settled), "Channel Alice-Bob should be Settled");

        (, TokenNetwork.ChannelState stateBC,,,,) = tokenNetwork.channels(channelBC);
        assertEq(
            uint256(stateBC), uint256(TokenNetwork.ChannelState.Settled), "Channel Bob-Charlie should be Settled"
        );

        (, TokenNetwork.ChannelState stateAC,,,,) = tokenNetwork.channels(channelAC);
        assertEq(
            uint256(stateAC), uint256(TokenNetwork.ChannelState.Settled), "Channel Alice-Charlie should be Settled"
        );
    }

    /// @notice Integration Test: Multi-token channels (USDC, DAI, USDT)
    /// @dev Tests TokenNetworkRegistry managing multiple TokenNetworks
    function testIntegration_MultiTokenChannels() public {
        // Deploy 3 tokens: USDC (6 decimals), DAI (18 decimals), USDT (6 decimals)
        MockERC20 usdc = new MockERC20("USD Coin", "USDC", 6);
        MockERC20 dai = new MockERC20("Dai Stablecoin", "DAI", 18);
        MockERC20 usdt = new MockERC20("Tether", "USDT", 6);

        // Create TokenNetworks via registry
        address tnUSDC = registry.createTokenNetwork(address(usdc));
        address tnDAI = registry.createTokenNetwork(address(dai));
        address tnUSDT = registry.createTokenNetwork(address(usdt));

        // Assert: TokenNetworks created
        assertTrue(tnUSDC != address(0), "USDC TokenNetwork should be created");
        assertTrue(tnDAI != address(0), "DAI TokenNetwork should be created");
        assertTrue(tnUSDT != address(0), "USDT TokenNetwork should be created");

        // Assert: All TokenNetworks different addresses
        assertTrue(tnUSDC != tnDAI && tnDAI != tnUSDT && tnUSDC != tnUSDT, "TokenNetworks should have unique addresses");

        // Assert: Registry mappings correct
        assertEq(registry.getTokenNetwork(address(usdc)), tnUSDC, "USDC TokenNetwork mapping should be correct");
        assertEq(registry.getTokenNetwork(address(dai)), tnDAI, "DAI TokenNetwork mapping should be correct");
        assertEq(registry.getTokenNetwork(address(usdt)), tnUSDT, "USDT TokenNetwork mapping should be correct");

        // Mint tokens to alice and bob
        usdc.transfer(alice, 10000 * 10 ** 6); // 10,000 USDC (6 decimals)
        usdc.transfer(bob, 10000 * 10 ** 6);
        dai.transfer(alice, 10000 * 10 ** 18); // 10,000 DAI (18 decimals)
        dai.transfer(bob, 10000 * 10 ** 18);
        usdt.transfer(alice, 10000 * 10 ** 6); // 10,000 USDT (6 decimals)
        usdt.transfer(bob, 10000 * 10 ** 6);

        // Open channels for all 3 tokens
        vm.prank(alice);
        bytes32 channelUSDC = TokenNetwork(tnUSDC).openChannel(bob, 1 hours);

        vm.prank(alice);
        bytes32 channelDAI = TokenNetwork(tnDAI).openChannel(bob, 1 hours);

        vm.prank(alice);
        bytes32 channelUSDT = TokenNetwork(tnUSDT).openChannel(bob, 1 hours);

        // Deposit to all channels
        vm.startPrank(alice);
        usdc.approve(tnUSDC, 1000 * 10 ** 6);
        TokenNetwork(tnUSDC).setTotalDeposit(channelUSDC, alice, 1000 * 10 ** 6);

        dai.approve(tnDAI, 1000 * 10 ** 18);
        TokenNetwork(tnDAI).setTotalDeposit(channelDAI, alice, 1000 * 10 ** 18);

        usdt.approve(tnUSDT, 1000 * 10 ** 6);
        TokenNetwork(tnUSDT).setTotalDeposit(channelUSDT, alice, 1000 * 10 ** 6);
        vm.stopPrank();

        vm.startPrank(bob);
        usdc.approve(tnUSDC, 1000 * 10 ** 6);
        TokenNetwork(tnUSDC).setTotalDeposit(channelUSDC, bob, 1000 * 10 ** 6);

        dai.approve(tnDAI, 1000 * 10 ** 18);
        TokenNetwork(tnDAI).setTotalDeposit(channelDAI, bob, 1000 * 10 ** 18);

        usdt.approve(tnUSDT, 1000 * 10 ** 6);
        TokenNetwork(tnUSDT).setTotalDeposit(channelUSDT, bob, 1000 * 10 ** 6);
        vm.stopPrank();

        // Validate token isolation: USDC channel doesn't affect DAI/USDT channels
        assertEq(usdc.balanceOf(tnUSDC), 2000 * 10 ** 6, "USDC TokenNetwork should hold 2000 USDC");
        assertEq(dai.balanceOf(tnDAI), 2000 * 10 ** 18, "DAI TokenNetwork should hold 2000 DAI");
        assertEq(usdt.balanceOf(tnUSDT), 2000 * 10 ** 6, "USDT TokenNetwork should hold 2000 USDT");

        // Validate all channels opened correctly
        (, TokenNetwork.ChannelState stateUSDC,,,,) = TokenNetwork(tnUSDC).channels(channelUSDC);
        assertEq(uint256(stateUSDC), uint256(TokenNetwork.ChannelState.Opened), "USDC channel should be Opened");

        (, TokenNetwork.ChannelState stateDAI,,,,) = TokenNetwork(tnDAI).channels(channelDAI);
        assertEq(uint256(stateDAI), uint256(TokenNetwork.ChannelState.Opened), "DAI channel should be Opened");

        (, TokenNetwork.ChannelState stateUSDT,,,,) = TokenNetwork(tnUSDT).channels(channelUSDT);
        assertEq(uint256(stateUSDT), uint256(TokenNetwork.ChannelState.Opened), "USDT channel should be Opened");
    }

    /// @notice Integration Test: Full channel lifecycle end-to-end
    /// @dev Tests: open → deposit → close → wait → settle
    function testIntegration_ChannelLifecycleEnd2End() public {
        // ===== Open Channel =====
        vm.prank(alice);
        bytes32 channelId = tokenNetwork.openChannel(bob, 1 hours);

        (, TokenNetwork.ChannelState state,,,,) = tokenNetwork.channels(channelId);
        assertEq(uint256(state), uint256(TokenNetwork.ChannelState.Opened), "Channel should be Opened");

        // ===== Deposit =====
        vm.startPrank(alice);
        token.approve(address(tokenNetwork), 1000 * 10 ** 18);
        tokenNetwork.setTotalDeposit(channelId, alice, 1000 * 10 ** 18);
        vm.stopPrank();

        vm.startPrank(bob);
        token.approve(address(tokenNetwork), 1000 * 10 ** 18);
        tokenNetwork.setTotalDeposit(channelId, bob, 1000 * 10 ** 18);
        vm.stopPrank();

        // Verify deposits (3 values: deposit, nonce, transferredAmount)
        (uint256 aliceDeposit,, ) = tokenNetwork.participants(channelId, alice);
        assertEq(aliceDeposit, 1000 * 10 ** 18, "Alice deposit should be 1000");

        (uint256 bobDeposit,, ) = tokenNetwork.participants(channelId, bob);
        assertEq(bobDeposit, 1000 * 10 ** 18, "Bob deposit should be 1000");

        // ===== Close Channel =====
        uint256 aliceBalanceBeforeSettle = token.balanceOf(alice);
        uint256 bobBalanceBeforeSettle = token.balanceOf(bob);

        vm.prank(alice);
        tokenNetwork.closeChannel(channelId);

        (, TokenNetwork.ChannelState closedState,,,,) = tokenNetwork.channels(channelId);
        assertEq(uint256(closedState), uint256(TokenNetwork.ChannelState.Closed), "Channel should be Closed");

        // ===== Wait for Challenge Period =====
        vm.warp(block.timestamp + 1 hours + 1);

        // ===== Settle Channel =====
        tokenNetwork.settleChannel(channelId);

        // Validate channel settled
        (, TokenNetwork.ChannelState finalState,,,,) = tokenNetwork.channels(channelId);
        assertEq(uint256(finalState), uint256(TokenNetwork.ChannelState.Settled), "Channel should be Settled");

        // Validate final balances: each participant gets their deposit back
        uint256 aliceBalanceAfterSettle = token.balanceOf(alice);
        uint256 bobBalanceAfterSettle = token.balanceOf(bob);

        assertEq(
            aliceBalanceAfterSettle,
            aliceBalanceBeforeSettle + 1000 * 10 ** 18,
            "Alice should receive her 1000 deposit back"
        );
        assertEq(
            bobBalanceAfterSettle,
            bobBalanceBeforeSettle + 1000 * 10 ** 18,
            "Bob should receive his 1000 deposit back"
        );
    }

}
