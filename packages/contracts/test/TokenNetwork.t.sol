// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import "forge-std/Test.sol";
import "../src/TokenNetwork.sol";
import "./mocks/MockERC20.sol";
import "./mocks/MockERC20WithFee.sol";

/// @title TokenNetworkTest
/// @notice Unit tests for TokenNetwork contract
contract TokenNetworkTest is Test {
    TokenNetwork public tokenNetwork;
    MockERC20 public token;
    address public alice;
    address public bob;
    address public charlie;

    uint256 public alicePrivateKey;
    uint256 public bobPrivateKey;
    uint256 public charliePrivateKey;

    function setUp() public {
        // Deploy mock ERC20 token
        token = new MockERC20("Test Token", "TEST", 18);

        // Deploy TokenNetwork with 1M token deposit limit
        tokenNetwork = new TokenNetwork(address(token), 1_000_000 * 10 ** 18, 365 days);

        // Create test accounts with private keys for EIP-712 signing
        alicePrivateKey = 0xA11CE;
        bobPrivateKey = 0xB0B;
        charliePrivateKey = 0xC0C;

        alice = vm.addr(alicePrivateKey);
        bob = vm.addr(bobPrivateKey);
        charlie = vm.addr(charliePrivateKey);

        // Mint tokens to test accounts
        vm.deal(alice, 100 ether);
        vm.deal(bob, 100 ether);
        vm.deal(charlie, 100 ether);

        // Transfer tokens to alice and bob for deposit tests
        token.transfer(alice, 10000 * 10 ** 18);
        token.transfer(bob, 10000 * 10 ** 18);
        token.transfer(charlie, 10000 * 10 ** 18);
    }

    // Test: openChannel - Happy path channel opening
    function testOpenChannel() public {
        vm.startPrank(alice);
        bytes32 channelId = tokenNetwork.openChannel(bob, 1 hours);

        // Assert channelId is not zero
        assertTrue(channelId != bytes32(0), "Channel ID should not be zero");

        // Assert channel state is Opened
        (uint256 settlementTimeout, TokenNetwork.ChannelState state,,, address p1, address p2) =
            tokenNetwork.channels(channelId);
        assertEq(uint256(state), uint256(TokenNetwork.ChannelState.Opened), "Channel should be Opened");
        assertEq(settlementTimeout, 1 hours, "Settlement timeout should match");

        // Assert participants are alice and bob (normalized order)
        assertTrue((p1 == alice && p2 == bob) || (p1 == bob && p2 == alice), "Participants should be alice and bob");

        vm.stopPrank();
    }

    // Test: openChannel emits ChannelOpened event
    function testOpenChannelEmitsEvent() public {
        vm.startPrank(alice);

        // Participants are normalized (p1 < p2 lexicographically)
        (address p1, address p2) = alice < bob ? (alice, bob) : (bob, alice);
        bytes32 expectedChannelId = keccak256(abi.encodePacked(p1, p2, uint256(0)));

        // Expect ChannelOpened event with normalized participants
        vm.expectEmit(true, true, true, true);
        emit ChannelOpened(expectedChannelId, p1, p2, 1 hours);

        tokenNetwork.openChannel(bob, 1 hours);

        vm.stopPrank();
    }

    // Event declarations for testing
    event ChannelOpened(
        bytes32 indexed channelId, address indexed participant1, address indexed participant2, uint256 settlementTimeout
    );

    event ChannelNewDeposit(bytes32 indexed channelId, address indexed participant, uint256 totalDeposit);

    // Test: openChannel reverts on zero address
    function testOpenChannelRevertsOnZeroAddress() public {
        vm.startPrank(alice);
        vm.expectRevert(TokenNetwork.InvalidParticipant.selector);
        tokenNetwork.openChannel(address(0), 1 hours);
        vm.stopPrank();
    }

    // Test: openChannel reverts on self-channel
    function testOpenChannelRevertsOnSelfChannel() public {
        vm.startPrank(alice);
        vm.expectRevert(TokenNetwork.InvalidParticipant.selector);
        tokenNetwork.openChannel(alice, 1 hours);
        vm.stopPrank();
    }

    // Test: openChannel reverts on invalid timeout
    function testOpenChannelRevertsOnInvalidTimeout() public {
        vm.startPrank(alice);
        vm.expectRevert(TokenNetwork.InvalidSettlementTimeout.selector);
        tokenNetwork.openChannel(bob, 30 minutes); // Below 1 hour minimum
        vm.stopPrank();
    }

    // Test: setTotalDeposit - Happy path deposit
    function testSetTotalDeposit() public {
        // Open channel
        vm.startPrank(alice);
        bytes32 channelId = tokenNetwork.openChannel(bob, 1 hours);
        vm.stopPrank();

        // Alice approves TokenNetwork to spend tokens
        vm.startPrank(alice);
        token.approve(address(tokenNetwork), 1000 * 10 ** 18);

        // Alice deposits 1000 tokens
        uint256 depositAmount = 1000 * 10 ** 18;
        uint256 balanceBefore = token.balanceOf(address(tokenNetwork));

        tokenNetwork.setTotalDeposit(channelId, alice, depositAmount);

        // Assert participant deposit updated
        (uint256 deposit,,) = tokenNetwork.participants(channelId, alice);
        assertEq(deposit, depositAmount, "Alice deposit should be 1000 tokens");

        // Assert TokenNetwork contract balance increased
        uint256 balanceAfter = token.balanceOf(address(tokenNetwork));
        assertEq(balanceAfter - balanceBefore, depositAmount, "Contract balance should increase by deposit amount");

        vm.stopPrank();
    }

    // Test: setTotalDeposit emits ChannelNewDeposit event
    function testSetTotalDepositEmitsEvent() public {
        // Open channel
        vm.startPrank(alice);
        bytes32 channelId = tokenNetwork.openChannel(bob, 1 hours);
        vm.stopPrank();

        // Alice approves and deposits
        vm.startPrank(alice);
        uint256 depositAmount = 1000 * 10 ** 18;
        token.approve(address(tokenNetwork), depositAmount);

        vm.expectEmit(true, true, false, true);
        emit ChannelNewDeposit(channelId, alice, depositAmount);

        tokenNetwork.setTotalDeposit(channelId, alice, depositAmount);
        vm.stopPrank();
    }

    // Test: setTotalDeposit - Cumulative deposit behavior
    function testSetTotalDepositIncremental() public {
        // Open channel
        vm.startPrank(alice);
        bytes32 channelId = tokenNetwork.openChannel(bob, 1 hours);
        vm.stopPrank();

        // Alice first deposit: 1000 tokens
        vm.startPrank(alice);
        token.approve(address(tokenNetwork), 2000 * 10 ** 18);
        tokenNetwork.setTotalDeposit(channelId, alice, 1000 * 10 ** 18);

        // Alice second deposit: additional 1000 (cumulative 2000)
        uint256 balanceBefore = token.balanceOf(address(tokenNetwork));
        tokenNetwork.setTotalDeposit(channelId, alice, 2000 * 10 ** 18);
        uint256 balanceAfter = token.balanceOf(address(tokenNetwork));

        // Assert participant deposit updated to 2000
        (uint256 deposit,,) = tokenNetwork.participants(channelId, alice);
        assertEq(deposit, 2000 * 10 ** 18, "Alice deposit should be 2000 tokens");

        // Assert only 1000 additional tokens transferred
        assertEq(balanceAfter - balanceBefore, 1000 * 10 ** 18, "Only 1000 additional tokens should be transferred");

        vm.stopPrank();
    }

    // Test: setTotalDeposit reverts on non-existent channel
    function testSetTotalDepositRevertsOnNonExistentChannel() public {
        // Create a fake channel ID that doesn't exist
        bytes32 fakeChannelId = keccak256("nonexistent");

        // Alice tries to deposit to non-existent channel
        vm.startPrank(alice);
        token.approve(address(tokenNetwork), 1000 * 10 ** 18);
        vm.expectRevert(TokenNetwork.InvalidChannelState.selector);
        tokenNetwork.setTotalDeposit(fakeChannelId, alice, 1000 * 10 ** 18);
        vm.stopPrank();
    }

    // Test: setTotalDeposit reverts on invalid participant
    function testSetTotalDepositRevertsOnInvalidParticipant() public {
        // Open channel between alice and bob
        vm.startPrank(alice);
        bytes32 channelId = tokenNetwork.openChannel(bob, 1 hours);
        vm.stopPrank();

        // Charlie tries to deposit (not a participant)
        vm.startPrank(charlie);
        token.approve(address(tokenNetwork), 1000 * 10 ** 18);
        vm.expectRevert(TokenNetwork.InvalidParticipant.selector);
        tokenNetwork.setTotalDeposit(channelId, charlie, 1000 * 10 ** 18);
        vm.stopPrank();
    }

    // Test: setTotalDeposit reverts on decreasing deposit
    function testSetTotalDepositRevertsOnDecreasingDeposit() public {
        // Open channel
        vm.startPrank(alice);
        bytes32 channelId = tokenNetwork.openChannel(bob, 1 hours);
        vm.stopPrank();

        // Alice deposits 1000 tokens
        vm.startPrank(alice);
        token.approve(address(tokenNetwork), 1000 * 10 ** 18);
        tokenNetwork.setTotalDeposit(channelId, alice, 1000 * 10 ** 18);

        // Alice tries to decrease deposit to 500
        vm.expectRevert(TokenNetwork.InsufficientDeposit.selector);
        tokenNetwork.setTotalDeposit(channelId, alice, 500 * 10 ** 18);
        vm.stopPrank();
    }

    // Test: Channel ID uniqueness
    function testChannelIdUniqueness() public {
        vm.startPrank(alice);

        // Open channel: alice → bob
        bytes32 channelId1 = tokenNetwork.openChannel(bob, 1 hours);

        // Open channel: alice → charlie
        bytes32 channelId2 = tokenNetwork.openChannel(charlie, 1 hours);

        // Assert different channel IDs
        assertTrue(channelId1 != channelId2, "Channel IDs should be unique");

        vm.stopPrank();
    }

    // Test: Multiple channels per TokenNetwork
    function testMultipleChannels() public {
        // Open channel: alice → bob
        vm.prank(alice);
        bytes32 channelId1 = tokenNetwork.openChannel(bob, 1 hours);

        // Open channel: alice → charlie
        vm.prank(alice);
        bytes32 channelId2 = tokenNetwork.openChannel(charlie, 1 hours);

        // Open channel: bob → charlie
        vm.prank(bob);
        bytes32 channelId3 = tokenNetwork.openChannel(charlie, 1 hours);

        // Assert all channels have state Opened
        (, TokenNetwork.ChannelState state1,,,,) = tokenNetwork.channels(channelId1);
        (, TokenNetwork.ChannelState state2,,,,) = tokenNetwork.channels(channelId2);
        (, TokenNetwork.ChannelState state3,,,,) = tokenNetwork.channels(channelId3);

        assertEq(uint256(state1), uint256(TokenNetwork.ChannelState.Opened), "Channel 1 should be Opened");
        assertEq(uint256(state2), uint256(TokenNetwork.ChannelState.Opened), "Channel 2 should be Opened");
        assertEq(uint256(state3), uint256(TokenNetwork.ChannelState.Opened), "Channel 3 should be Opened");

        // Assert all channels have unique IDs
        assertTrue(channelId1 != channelId2, "Channel 1 and 2 should have different IDs");
        assertTrue(channelId1 != channelId3, "Channel 1 and 3 should have different IDs");
        assertTrue(channelId2 != channelId3, "Channel 2 and 3 should have different IDs");
    }

    // Test: Both participants can deposit to the same channel
    function testBothParticipantsCanDeposit() public {
        // Open channel
        vm.startPrank(alice);
        bytes32 channelId = tokenNetwork.openChannel(bob, 1 hours);
        vm.stopPrank();

        // Alice deposits 1000 tokens
        vm.startPrank(alice);
        token.approve(address(tokenNetwork), 1000 * 10 ** 18);
        tokenNetwork.setTotalDeposit(channelId, alice, 1000 * 10 ** 18);
        vm.stopPrank();

        // Bob deposits 2000 tokens
        vm.startPrank(bob);
        token.approve(address(tokenNetwork), 2000 * 10 ** 18);
        tokenNetwork.setTotalDeposit(channelId, bob, 2000 * 10 ** 18);
        vm.stopPrank();

        // Assert both deposits recorded
        (uint256 aliceDeposit,,) = tokenNetwork.participants(channelId, alice);
        (uint256 bobDeposit,,) = tokenNetwork.participants(channelId, bob);

        assertEq(aliceDeposit, 1000 * 10 ** 18, "Alice deposit should be 1000 tokens");
        assertEq(bobDeposit, 2000 * 10 ** 18, "Bob deposit should be 2000 tokens");
    }

    // Test: Third party can deposit on behalf of participant
    function testThirdPartyCanDeposit() public {
        // Open channel between alice and bob
        vm.startPrank(alice);
        bytes32 channelId = tokenNetwork.openChannel(bob, 1 hours);
        vm.stopPrank();

        // Charlie deposits on behalf of alice
        vm.startPrank(charlie);
        token.approve(address(tokenNetwork), 1000 * 10 ** 18);
        tokenNetwork.setTotalDeposit(channelId, alice, 1000 * 10 ** 18);
        vm.stopPrank();

        // Assert alice's deposit is updated (even though charlie paid)
        (uint256 aliceDeposit,,) = tokenNetwork.participants(channelId, alice);
        assertEq(aliceDeposit, 1000 * 10 ** 18, "Alice deposit should be 1000 tokens");
    }

    // ===== Helper Functions for Channel Closure Tests =====

    /// @notice Helper to create and fund a channel
    function createAndFundChannel(address participant1, address participant2, uint256 deposit1, uint256 deposit2)
        internal
        returns (bytes32)
    {
        // Open channel
        vm.prank(participant1);
        bytes32 channelId = tokenNetwork.openChannel(participant2, 1 hours);

        // Fund participant1
        if (deposit1 > 0) {
            vm.startPrank(participant1);
            token.approve(address(tokenNetwork), deposit1);
            tokenNetwork.setTotalDeposit(channelId, participant1, deposit1);
            vm.stopPrank();
        }

        // Fund participant2
        if (deposit2 > 0) {
            vm.startPrank(participant2);
            token.approve(address(tokenNetwork), deposit2);
            tokenNetwork.setTotalDeposit(channelId, participant2, deposit2);
            vm.stopPrank();
        }

        return channelId;
    }

    /// @notice Helper to sign a balance proof using EIP-712
    function signBalanceProof(
        uint256 privateKey,
        bytes32 channelId,
        uint256 nonce,
        uint256 transferredAmount,
        uint256 lockedAmount,
        bytes32 locksRoot
    ) internal view returns (bytes memory) {
        // Compute EIP-712 domain separator manually
        bytes32 TYPE_HASH =
            keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)");
        bytes32 nameHash = keccak256("TokenNetwork");
        bytes32 versionHash = keccak256("1");
        bytes32 domainSeparator =
            keccak256(abi.encode(TYPE_HASH, nameHash, versionHash, block.chainid, address(tokenNetwork)));

        // Compute struct hash
        bytes32 structHash = keccak256(
            abi.encode(
                keccak256(
                    "BalanceProof(bytes32 channelId,uint256 nonce,uint256 transferredAmount,uint256 lockedAmount,bytes32 locksRoot)"
                ),
                channelId,
                nonce,
                transferredAmount,
                lockedAmount,
                locksRoot
            )
        );

        // Compute digest
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", domainSeparator, structHash));

        // Sign digest
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(privateKey, digest);
        return abi.encodePacked(r, s, v);
    }

    // ===== Channel Closure Tests =====

    // Test: closeChannel - Happy path channel closure
    function testCloseChannel() public {
        bytes32 channelId = createAndFundChannel(alice, bob, 1000 * 10 ** 18, 1000 * 10 ** 18);

        // Bob calls closeChannel with just channelId
        vm.prank(bob);
        tokenNetwork.closeChannel(channelId);

        // Assert: Channel state is Closed
        (, TokenNetwork.ChannelState state, uint256 closedAt,,,) = tokenNetwork.channels(channelId);
        assertEq(uint256(state), uint256(TokenNetwork.ChannelState.Closed), "Channel should be Closed");
        assertEq(closedAt, block.timestamp, "Channel closedAt should be current timestamp");
    }

    event ChannelClosed(bytes32 indexed channelId, address indexed closingParticipant);

    // Test: closeChannel reverts on invalid state
    function testCloseChannelRevertsOnInvalidState() public {
        bytes32 channelId = createAndFundChannel(alice, bob, 1000 * 10 ** 18, 1000 * 10 ** 18);

        // Bob closes channel
        vm.prank(bob);
        tokenNetwork.closeChannel(channelId);

        // Try to close again
        vm.expectRevert(TokenNetwork.InvalidChannelState.selector);
        vm.prank(bob);
        tokenNetwork.closeChannel(channelId);
    }

    // Test: settleChannel - Happy path settlement
    function testSettleChannel() public {
        bytes32 channelId = createAndFundChannel(alice, bob, 1000 * 10 ** 18, 1000 * 10 ** 18);

        // Bob closes channel
        vm.prank(bob);
        tokenNetwork.closeChannel(channelId);

        // Fast forward past challenge period
        vm.warp(block.timestamp + 1 hours + 1);

        // Record balances before settlement
        uint256 aliceBalanceBefore = token.balanceOf(alice);
        uint256 bobBalanceBefore = token.balanceOf(bob);

        // Settle channel
        tokenNetwork.settleChannel(channelId);

        // Assert: Channel state is Settled
        (, TokenNetwork.ChannelState state,,,,) = tokenNetwork.channels(channelId);
        assertEq(uint256(state), uint256(TokenNetwork.ChannelState.Settled), "Channel should be Settled");

        // Assert: Each depositor gets deposit - claimedAmounts back
        // No claims were made, so Alice gets 1000, Bob gets 1000
        uint256 aliceBalanceAfter = token.balanceOf(alice);
        assertEq(aliceBalanceAfter - aliceBalanceBefore, 1000 * 10 ** 18, "Alice should receive 1000 tokens");

        uint256 bobBalanceAfter = token.balanceOf(bob);
        assertEq(bobBalanceAfter - bobBalanceBefore, 1000 * 10 ** 18, "Bob should receive 1000 tokens");

        // Assert: TokenNetwork contract balance is reduced
        uint256 contractBalance = token.balanceOf(address(tokenNetwork));
        assertEq(contractBalance, 0, "Contract balance should be 0 after settlement");
    }

    event ChannelSettled(bytes32 indexed channelId, uint256 participant1Amount, uint256 participant2Amount);

    // Test: settleChannel reverts before timeout
    function testSettleChannelRevertsBeforeTimeout() public {
        bytes32 channelId = createAndFundChannel(alice, bob, 1000 * 10 ** 18, 1000 * 10 ** 18);

        // Close channel
        vm.prank(bob);
        tokenNetwork.closeChannel(channelId);

        // Try to settle immediately
        vm.expectRevert(TokenNetwork.SettlementTimeoutNotExpired.selector);
        tokenNetwork.settleChannel(channelId);
    }

    // Test: settleChannel reverts on wrong state
    function testSettleChannelRevertsOnWrongState() public {
        bytes32 channelId = createAndFundChannel(alice, bob, 1000 * 10 ** 18, 1000 * 10 ** 18);

        // Try to settle opened channel
        vm.expectRevert(TokenNetwork.InvalidChannelState.selector);
        tokenNetwork.settleChannel(channelId);
    }

    // Test: bilateral transfers - Both participants claim then close and settle
    function testBilateralTransfers() public {
        bytes32 channelId = createAndFundChannel(alice, bob, 1000 * 10 ** 18, 1000 * 10 ** 18);

        // Bob claims 300 from Alice's signed balance proof
        TokenNetwork.BalanceProof memory aliceProof = TokenNetwork.BalanceProof({
            channelId: channelId,
            nonce: 1,
            transferredAmount: 300 * 10 ** 18,
            lockedAmount: 0,
            locksRoot: bytes32(0)
        });
        bytes memory aliceSignature = signBalanceProof(alicePrivateKey, channelId, 1, 300 * 10 ** 18, 0, bytes32(0));

        vm.prank(bob);
        tokenNetwork.claimFromChannel(channelId, aliceProof, aliceSignature);

        // Alice claims 200 from Bob's signed balance proof
        TokenNetwork.BalanceProof memory bobProof = TokenNetwork.BalanceProof({
            channelId: channelId,
            nonce: 1,
            transferredAmount: 200 * 10 ** 18,
            lockedAmount: 0,
            locksRoot: bytes32(0)
        });
        bytes memory bobSignature = signBalanceProof(bobPrivateKey, channelId, 1, 200 * 10 ** 18, 0, bytes32(0));

        vm.prank(alice);
        tokenNetwork.claimFromChannel(channelId, bobProof, bobSignature);

        // Close and settle
        vm.prank(bob);
        tokenNetwork.closeChannel(channelId);

        vm.warp(block.timestamp + 1 hours + 1);

        uint256 aliceBalanceBefore = token.balanceOf(alice);
        uint256 bobBalanceBefore = token.balanceOf(bob);

        tokenNetwork.settleChannel(channelId);

        // Settlement returns deposit - claimedAmounts to each depositor
        // Alice: 1000 - 300 (claimed by Bob) = 700
        // Bob: 1000 - 200 (claimed by Alice) = 800
        uint256 aliceBalanceAfter = token.balanceOf(alice);
        assertEq(aliceBalanceAfter - aliceBalanceBefore, 700 * 10 ** 18, "Alice should receive 700 tokens from settlement");

        uint256 bobBalanceAfter = token.balanceOf(bob);
        assertEq(bobBalanceAfter - bobBalanceBefore, 800 * 10 ** 18, "Bob should receive 800 tokens from settlement");
    }

    // Test: Pause prevents all state-changing operations
    function testPausePreventOperations() public {
        // Pause contract
        tokenNetwork.pause();

        // Test: openChannel reverts when paused
        vm.startPrank(alice);
        vm.expectRevert();
        tokenNetwork.openChannel(bob, 1 hours);
        vm.stopPrank();

        // Unpause to setup channel for other tests
        tokenNetwork.unpause();

        // Open channel and deposit
        vm.startPrank(alice);
        bytes32 channelId = tokenNetwork.openChannel(bob, 1 hours);
        token.approve(address(tokenNetwork), 1000 * 10 ** 18);
        tokenNetwork.setTotalDeposit(channelId, alice, 1000 * 10 ** 18);
        vm.stopPrank();

        vm.startPrank(bob);
        token.approve(address(tokenNetwork), 1000 * 10 ** 18);
        tokenNetwork.setTotalDeposit(channelId, bob, 1000 * 10 ** 18);
        vm.stopPrank();

        // Pause again
        tokenNetwork.pause();

        // Test: setTotalDeposit reverts when paused
        vm.startPrank(alice);
        vm.expectRevert();
        tokenNetwork.setTotalDeposit(channelId, alice, 2000 * 10 ** 18);
        vm.stopPrank();

        // Test: closeChannel reverts when paused
        vm.startPrank(bob);
        vm.expectRevert();
        tokenNetwork.closeChannel(channelId);
        vm.stopPrank();

        // Unpause to close channel
        tokenNetwork.unpause();

        vm.prank(bob);
        tokenNetwork.closeChannel(channelId);

        // Pause again
        tokenNetwork.pause();

        // Fast forward past challenge period
        vm.warp(block.timestamp + 1 hours + 1);

        // Test: settleChannel reverts when paused
        vm.expectRevert();
        tokenNetwork.settleChannel(channelId);

        // Unpause and settle should work
        tokenNetwork.unpause();
        tokenNetwork.settleChannel(channelId);
    }

    // Test: Unpause restores operations
    function testUnpauseRestoresOperations() public {
        // Pause
        tokenNetwork.pause();

        // Verify paused
        vm.startPrank(alice);
        vm.expectRevert();
        tokenNetwork.openChannel(bob, 1 hours);
        vm.stopPrank();

        // Unpause
        tokenNetwork.unpause();

        // Should work now
        vm.startPrank(alice);
        bytes32 channelId = tokenNetwork.openChannel(bob, 1 hours);
        assertTrue(channelId != bytes32(0), "Channel should be created after unpause");
        vm.stopPrank();
    }

    // Test: Deposit limit prevents excessive deposit
    function testDepositLimitPreventsExcessiveDeposit() public {
        // Deploy TokenNetwork with 1000 token deposit limit for testing
        TokenNetwork testNetwork = new TokenNetwork(address(token), 1000 * 10 ** 18, 365 days);

        // Open channel
        vm.startPrank(alice);
        bytes32 channelId = testNetwork.openChannel(bob, 1 hours);

        // Approve amount over limit
        token.approve(address(testNetwork), 1500 * 10 ** 18);

        // Try to deposit more than maxChannelDeposit (1000 tokens) - should revert
        vm.expectRevert(TokenNetwork.DepositLimitExceeded.selector);
        testNetwork.setTotalDeposit(channelId, alice, 1100 * 10 ** 18);

        vm.stopPrank();
    }

    // Test: Deposit limit allows multiple deposits under limit
    function testDepositLimitAllowsMultipleDepositsUnderLimit() public {
        // Deploy TokenNetwork with 1000 token deposit limit for testing
        TokenNetwork testNetwork = new TokenNetwork(address(token), 1000 * 10 ** 18, 365 days);

        // Open channel
        vm.startPrank(alice);
        bytes32 channelId = testNetwork.openChannel(bob, 1 hours);

        // First deposit: 500 tokens
        token.approve(address(testNetwork), 500 * 10 ** 18);
        testNetwork.setTotalDeposit(channelId, alice, 500 * 10 ** 18);

        // Second deposit: additional 400 tokens (total 900, under 1000 limit)
        token.approve(address(testNetwork), 400 * 10 ** 18);
        testNetwork.setTotalDeposit(channelId, alice, 900 * 10 ** 18);

        // Verify deposit
        (uint256 aliceDeposit,,) = testNetwork.participants(channelId, alice);
        assertEq(aliceDeposit, 900 * 10 ** 18, "Alice should have 900 tokens deposited");

        vm.stopPrank();
    }

    // Test: Deposit with fee-on-transfer token
    function testDepositWithFeeOnTransferToken() public {
        // Deploy mock ERC20 with 10% transfer fee
        MockERC20WithFee feeToken = new MockERC20WithFee("Fee Token", "FEE", 18, 10);

        // Deploy TokenNetwork for fee token with 1M token deposit limit
        TokenNetwork feeTokenNetwork = new TokenNetwork(address(feeToken), 1_000_000 * 10 ** 18, 365 days);

        // Mint tokens to alice
        feeToken.transfer(alice, 10000 * 10 ** 18);

        // Open channel
        vm.startPrank(alice);
        bytes32 channelId = feeTokenNetwork.openChannel(bob, 1 hours);

        // Approve and deposit 1000 tokens
        feeToken.approve(address(feeTokenNetwork), 1000 * 10 ** 18);
        feeTokenNetwork.setTotalDeposit(channelId, alice, 1000 * 10 ** 18);

        // Verify: Participant deposit equals actualReceived (900 tokens after 10% fee)
        (uint256 aliceDeposit,,) = feeTokenNetwork.participants(channelId, alice);
        assertEq(aliceDeposit, 900 * 10 ** 18, "Deposit should be 900 tokens (90% of 1000 after 10% fee)");

        vm.stopPrank();
    }

    function testForceCloseExpiredChannel() public {
        // Open channel
        vm.prank(alice);
        bytes32 channelId = tokenNetwork.openChannel(bob, 1 hours);

        // Fast forward 366 days (past 365 day expiry)
        vm.warp(block.timestamp + 366 days);

        // Anyone can force-close expired channel
        tokenNetwork.forceCloseExpiredChannel(channelId);

        // Verify: Channel is Closed
        (, TokenNetwork.ChannelState state, uint256 closedAt,,,) = tokenNetwork.channels(channelId);
        assertEq(uint256(state), uint256(TokenNetwork.ChannelState.Closed), "Channel should be Closed");
        assertEq(closedAt, block.timestamp, "closedAt should be current timestamp");
    }

    function testForceCloseRevertsOnActiveChannel() public {
        // Open channel
        vm.prank(alice);
        bytes32 channelId = tokenNetwork.openChannel(bob, 1 hours);

        // Try force-close before expiry (should revert)
        vm.expectRevert(TokenNetwork.ChannelNotExpired.selector);
        tokenNetwork.forceCloseExpiredChannel(channelId);

        // Fast forward 364 days (still before 365 day expiry)
        vm.warp(block.timestamp + 364 days);

        // Try again (should still revert)
        vm.expectRevert(TokenNetwork.ChannelNotExpired.selector);
        tokenNetwork.forceCloseExpiredChannel(channelId);
    }

    function testEmergencyWithdrawOnlyWhenPaused() public {
        // Open channel and deposit tokens
        vm.prank(alice);
        bytes32 channelId = tokenNetwork.openChannel(bob, 1 hours);

        vm.startPrank(alice);
        token.approve(address(tokenNetwork), 1000 * 10 ** 18);
        tokenNetwork.setTotalDeposit(channelId, alice, 1000 * 10 ** 18);
        vm.stopPrank();

        // Try emergency withdraw when NOT paused (should revert)
        vm.expectRevert(TokenNetwork.ContractNotPaused.selector);
        tokenNetwork.emergencyWithdraw(channelId, address(this));
    }

    function testEmergencyWithdrawOnlyOwner() public {
        // Open channel and deposit tokens
        vm.prank(alice);
        bytes32 channelId = tokenNetwork.openChannel(bob, 1 hours);

        vm.startPrank(alice);
        token.approve(address(tokenNetwork), 1000 * 10 ** 18);
        tokenNetwork.setTotalDeposit(channelId, alice, 1000 * 10 ** 18);
        vm.stopPrank();

        // Pause contract
        tokenNetwork.pause();

        // Try emergency withdraw as non-owner (should revert)
        vm.prank(alice);
        vm.expectRevert();
        tokenNetwork.emergencyWithdraw(channelId, alice);
    }

    function testEmergencyWithdrawRecoveryStuckFunds() public {
        // Open channel and deposit tokens
        vm.prank(alice);
        bytes32 channelId = tokenNetwork.openChannel(bob, 1 hours);

        vm.startPrank(alice);
        token.approve(address(tokenNetwork), 1000 * 10 ** 18);
        tokenNetwork.setTotalDeposit(channelId, alice, 1000 * 10 ** 18);
        vm.stopPrank();

        // Simulate emergency: pause contract
        tokenNetwork.pause();

        // Record owner balance before recovery
        address owner = address(this);
        uint256 ownerBalanceBefore = token.balanceOf(owner);
        uint256 contractBalance = token.balanceOf(address(tokenNetwork));

        // Owner performs emergency withdrawal
        vm.expectEmit(true, true, false, true);
        emit TokenNetwork.EmergencyWithdrawal(channelId, owner, contractBalance);
        tokenNetwork.emergencyWithdraw(channelId, owner);

        // Verify all tokens recovered
        assertEq(token.balanceOf(owner), ownerBalanceBefore + contractBalance);
        assertEq(token.balanceOf(address(tokenNetwork)), 0);
    }


    // ===== claimFromChannel Tests =====

    event ChannelClaimed(bytes32 indexed channelId, address indexed claimant, uint256 claimedAmount, uint256 totalClaimed);

    // Test: claimFromChannel - Happy path: B claims from A's signed balance proof, channel stays open
    function testClaimFromChannel() public {
        bytes32 channelId = createAndFundChannel(alice, bob, 1000 * 10 ** 18, 0);

        uint256 bobBalanceBefore = token.balanceOf(bob);

        // Alice signs a balance proof transferring 300 to Bob
        TokenNetwork.BalanceProof memory balanceProof = TokenNetwork.BalanceProof({
            channelId: channelId,
            nonce: 1,
            transferredAmount: 300 * 10 ** 18,
            lockedAmount: 0,
            locksRoot: bytes32(0)
        });
        bytes memory aliceSignature = signBalanceProof(alicePrivateKey, channelId, 1, 300 * 10 ** 18, 0, bytes32(0));

        // Bob claims from channel
        vm.startPrank(bob);
        vm.expectEmit(true, true, false, true);
        emit ChannelClaimed(channelId, bob, 300 * 10 ** 18, 300 * 10 ** 18);
        tokenNetwork.claimFromChannel(channelId, balanceProof, aliceSignature);
        vm.stopPrank();

        // Verify Bob received the tokens
        assertEq(token.balanceOf(bob), bobBalanceBefore + 300 * 10 ** 18, "Bob should receive claimed tokens");

        // Verify channel is STILL OPEN
        (, TokenNetwork.ChannelState state,,,,) = tokenNetwork.channels(channelId);
        assertEq(uint256(state), uint256(TokenNetwork.ChannelState.Opened), "Channel should remain open after claim");

        // Verify claimed amounts tracking
        assertEq(tokenNetwork.claimedAmounts(channelId, bob), 300 * 10 ** 18, "Claimed amount should be tracked");
    }

    // Test: claimFromChannel - Multiple sequential claims
    function testClaimFromChannelMultipleClaims() public {
        bytes32 channelId = createAndFundChannel(alice, bob, 1000 * 10 ** 18, 0);

        uint256 bobBalanceBefore = token.balanceOf(bob);

        // First claim: 200 tokens
        TokenNetwork.BalanceProof memory proof1 = TokenNetwork.BalanceProof({
            channelId: channelId,
            nonce: 1,
            transferredAmount: 200 * 10 ** 18,
            lockedAmount: 0,
            locksRoot: bytes32(0)
        });
        bytes memory sig1 = signBalanceProof(alicePrivateKey, channelId, 1, 200 * 10 ** 18, 0, bytes32(0));

        vm.prank(bob);
        tokenNetwork.claimFromChannel(channelId, proof1, sig1);

        assertEq(token.balanceOf(bob), bobBalanceBefore + 200 * 10 ** 18, "After first claim");

        // Second claim: cumulative 500 tokens (delta = 300)
        TokenNetwork.BalanceProof memory proof2 = TokenNetwork.BalanceProof({
            channelId: channelId,
            nonce: 2,
            transferredAmount: 500 * 10 ** 18,
            lockedAmount: 0,
            locksRoot: bytes32(0)
        });
        bytes memory sig2 = signBalanceProof(alicePrivateKey, channelId, 2, 500 * 10 ** 18, 0, bytes32(0));

        vm.prank(bob);
        tokenNetwork.claimFromChannel(channelId, proof2, sig2);

        assertEq(token.balanceOf(bob), bobBalanceBefore + 500 * 10 ** 18, "After second claim");

        // Verify channel is still open
        (, TokenNetwork.ChannelState state,,,,) = tokenNetwork.channels(channelId);
        assertEq(uint256(state), uint256(TokenNetwork.ChannelState.Opened), "Channel should still be open");

        // Verify total claimed
        assertEq(tokenNetwork.claimedAmounts(channelId, bob), 500 * 10 ** 18, "Total claimed should be cumulative");
    }

    // Test: claimFromChannel - Reverts on stale nonce
    function testClaimFromChannelRevertsOnStaleNonce() public {
        bytes32 channelId = createAndFundChannel(alice, bob, 1000 * 10 ** 18, 0);

        // First claim with nonce 1
        TokenNetwork.BalanceProof memory proof1 = TokenNetwork.BalanceProof({
            channelId: channelId,
            nonce: 1,
            transferredAmount: 200 * 10 ** 18,
            lockedAmount: 0,
            locksRoot: bytes32(0)
        });
        bytes memory sig1 = signBalanceProof(alicePrivateKey, channelId, 1, 200 * 10 ** 18, 0, bytes32(0));

        vm.prank(bob);
        tokenNetwork.claimFromChannel(channelId, proof1, sig1);

        // Try to claim again with same nonce (should revert)
        vm.prank(bob);
        vm.expectRevert(TokenNetwork.InvalidNonce.selector);
        tokenNetwork.claimFromChannel(channelId, proof1, sig1);
    }

    // Test: claimFromChannel - Reverts when claim exceeds deposit
    function testClaimFromChannelRevertsOnInsufficientBalance() public {
        bytes32 channelId = createAndFundChannel(alice, bob, 500 * 10 ** 18, 0);

        // Try to claim more than Alice deposited
        TokenNetwork.BalanceProof memory balanceProof = TokenNetwork.BalanceProof({
            channelId: channelId,
            nonce: 1,
            transferredAmount: 600 * 10 ** 18,
            lockedAmount: 0,
            locksRoot: bytes32(0)
        });
        bytes memory aliceSignature = signBalanceProof(alicePrivateKey, channelId, 1, 600 * 10 ** 18, 0, bytes32(0));

        vm.prank(bob);
        vm.expectRevert(TokenNetwork.InsufficientChannelBalance.selector);
        tokenNetwork.claimFromChannel(channelId, balanceProof, aliceSignature);
    }

    // Test: claimFromChannel - Reverts on wrong signer
    function testClaimFromChannelRevertsOnWrongSigner() public {
        bytes32 channelId = createAndFundChannel(alice, bob, 1000 * 10 ** 18, 0);

        // Charlie signs (not a participant)
        TokenNetwork.BalanceProof memory balanceProof = TokenNetwork.BalanceProof({
            channelId: channelId,
            nonce: 1,
            transferredAmount: 300 * 10 ** 18,
            lockedAmount: 0,
            locksRoot: bytes32(0)
        });
        bytes memory charlieSignature = signBalanceProof(charliePrivateKey, channelId, 1, 300 * 10 ** 18, 0, bytes32(0));

        vm.prank(bob);
        vm.expectRevert(TokenNetwork.InvalidSignature.selector);
        tokenNetwork.claimFromChannel(channelId, balanceProof, charlieSignature);
    }

    // Test: claimFromChannel then settleChannel - No double-pay
    function testClaimThenSettleNoDoublePay() public {
        bytes32 channelId = createAndFundChannel(alice, bob, 1000 * 10 ** 18, 500 * 10 ** 18);

        uint256 aliceBalanceBefore = token.balanceOf(alice);
        uint256 bobBalanceBefore = token.balanceOf(bob);

        // Bob claims 300 from Alice's balance proof
        TokenNetwork.BalanceProof memory claimProof = TokenNetwork.BalanceProof({
            channelId: channelId,
            nonce: 1,
            transferredAmount: 300 * 10 ** 18,
            lockedAmount: 0,
            locksRoot: bytes32(0)
        });
        bytes memory aliceSig = signBalanceProof(alicePrivateKey, channelId, 1, 300 * 10 ** 18, 0, bytes32(0));

        vm.prank(bob);
        tokenNetwork.claimFromChannel(channelId, claimProof, aliceSig);

        // Bob already received 300 from claim
        assertEq(token.balanceOf(bob), bobBalanceBefore + 300 * 10 ** 18, "Bob should have claimed 300");

        // Bob closes channel
        vm.prank(bob);
        tokenNetwork.closeChannel(channelId);

        // Fast forward past challenge period
        vm.warp(block.timestamp + 1 hours + 1);

        // Settle channel
        tokenNetwork.settleChannel(channelId);

        // Settlement returns deposit - claimedAmounts to each depositor:
        // Alice: deposit=1000, claimedAmounts(alice)=300 (claimed by Bob) → settle returns 1000 - 300 = 700
        // Bob: deposit=500, claimedAmounts(bob)=0 → settle returns 500 - 0 = 500
        // Bob's total: 300 (from claim) + 500 (from settle) = 800
        assertEq(token.balanceOf(alice), aliceBalanceBefore + 700 * 10 ** 18, "Alice final balance after settle");
        assertEq(token.balanceOf(bob), bobBalanceBefore + 300 * 10 ** 18 + 500 * 10 ** 18, "Bob final balance (claim + settle)");
    }

    // Test: claimFromChannel - Reverts on settled channel
    function testClaimFromChannelRevertsOnSettledChannel() public {
        bytes32 channelId = createAndFundChannel(alice, bob, 1000 * 10 ** 18, 0);

        // Close the channel
        vm.prank(bob);
        tokenNetwork.closeChannel(channelId);

        // Fast forward past challenge period and settle
        vm.warp(block.timestamp + 1 hours + 1);
        tokenNetwork.settleChannel(channelId);

        // Try to claim from settled channel
        TokenNetwork.BalanceProof memory claimProof = TokenNetwork.BalanceProof({
            channelId: channelId,
            nonce: 1,
            transferredAmount: 200 * 10 ** 18,
            lockedAmount: 0,
            locksRoot: bytes32(0)
        });
        bytes memory aliceSig = signBalanceProof(alicePrivateKey, channelId, 1, 200 * 10 ** 18, 0, bytes32(0));

        vm.prank(bob);
        vm.expectRevert(TokenNetwork.InvalidChannelState.selector);
        tokenNetwork.claimFromChannel(channelId, claimProof, aliceSig);
    }

    // Test: claimFromChannel - Reverts when nothing to claim
    function testClaimFromChannelRevertsOnNothingToClaim() public {
        bytes32 channelId = createAndFundChannel(alice, bob, 1000 * 10 ** 18, 0);

        // First claim: 200 tokens with nonce 1
        TokenNetwork.BalanceProof memory proof1 = TokenNetwork.BalanceProof({
            channelId: channelId,
            nonce: 1,
            transferredAmount: 200 * 10 ** 18,
            lockedAmount: 0,
            locksRoot: bytes32(0)
        });
        bytes memory sig1 = signBalanceProof(alicePrivateKey, channelId, 1, 200 * 10 ** 18, 0, bytes32(0));

        vm.prank(bob);
        tokenNetwork.claimFromChannel(channelId, proof1, sig1);

        // Try to claim again with higher nonce but same transferredAmount (nothing new to claim)
        TokenNetwork.BalanceProof memory proof2 = TokenNetwork.BalanceProof({
            channelId: channelId,
            nonce: 2,
            transferredAmount: 200 * 10 ** 18,
            lockedAmount: 0,
            locksRoot: bytes32(0)
        });
        bytes memory sig2 = signBalanceProof(alicePrivateKey, channelId, 2, 200 * 10 ** 18, 0, bytes32(0));

        vm.prank(bob);
        vm.expectRevert(TokenNetwork.NothingToClaim.selector);
        tokenNetwork.claimFromChannel(channelId, proof2, sig2);
    }
}
