// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import "@openzeppelin/contracts/access/Ownable.sol";
import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import "@openzeppelin/contracts/utils/Pausable.sol";
import "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import "@openzeppelin/contracts/utils/cryptography/EIP712.sol";
import "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";

/// @title TokenNetwork
/// @notice Manages payment channels for a specific ERC20 token
/// @dev Deployed by TokenNetworkRegistry for each token. Supports channel opening, deposits, closure, and settlement.
contract TokenNetwork is ReentrancyGuard, EIP712, Pausable, Ownable {
    using SafeERC20 for IERC20;

    /// @notice The ERC20 token address this TokenNetwork manages
    address public immutable token;

    /// @notice Maximum deposit per participant per channel
    uint256 public immutable maxChannelDeposit;

    /// @notice Maximum channel lifetime before force-close allowed
    uint256 public immutable maxChannelLifetime;

    /// @notice Monotonically increasing counter for unique channel IDs
    uint256 public channelCounter;

    /// @notice Minimum settlement timeout (1 hour)
    uint256 public constant MIN_SETTLEMENT_TIMEOUT = 1 hours;

    /// @notice EIP-712 type hash for balance proof verification
    bytes32 private constant BALANCE_PROOF_TYPEHASH = keccak256(
        "BalanceProof(bytes32 channelId,uint256 nonce,uint256 transferredAmount,uint256 lockedAmount,bytes32 locksRoot)"
    );

    /// @notice Channel lifecycle states
    enum ChannelState {
        NonExistent, /// Channel doesn't exist
        Opened, /// Channel active, deposits allowed
        Closed, /// Channel closed, challenge period active
        Settled /// Channel settled, funds distributed
    }

    /// @notice Off-chain state representation for channel closure
    struct BalanceProof {
        bytes32 channelId; /// Channel identifier
        uint256 nonce; /// Monotonically increasing state counter
        uint256 transferredAmount; /// Cumulative amount sent to counterparty
        uint256 lockedAmount; /// Amount in pending HTLCs (unused in Story 8.4)
        bytes32 locksRoot; /// Merkle root of hash-locked transfers (unused in Story 8.4)
    }

    /// @notice Per-participant channel state
    struct ParticipantState {
        uint256 deposit; /// Total deposited by participant
        uint256 nonce; /// Monotonically increasing state counter
        uint256 transferredAmount; /// Cumulative amount sent to counterparty
    }

    /// @notice Channel metadata
    struct Channel {
        uint256 settlementTimeout; /// Challenge period duration in seconds
        ChannelState state; /// Current channel state
        uint256 closedAt; /// Block timestamp when channel closed
        uint256 openedAt; /// Block timestamp when channel opened
        address participant1; /// First channel participant
        address participant2; /// Second channel participant
    }

    /// @notice Mapping of channel IDs to channel data
    mapping(bytes32 => Channel) public channels;

    /// @notice Mapping of channel IDs and participant addresses to participant state
    mapping(bytes32 => mapping(address => ParticipantState)) public participants;

    /// @notice Mapping of channel IDs and participant addresses to cumulative claimed amounts
    /// @dev Tracks tokens already transferred out via claimFromChannel to prevent double-pay at settlement
    mapping(bytes32 => mapping(address => uint256)) public claimedAmounts;

    /// @notice Thrown when participant address is invalid (zero address or same as caller)
    error InvalidParticipant();

    /// @notice Thrown when settlement timeout is below minimum
    error InvalidSettlementTimeout();

    /// @notice Thrown when channel already exists between participants
    error ChannelAlreadyExists();

    /// @notice Thrown when channel doesn't exist
    error ChannelDoesNotExist();

    /// @notice Thrown when operation not allowed in current channel state
    error InvalidChannelState();

    /// @notice Thrown when deposit amount validation fails
    error InsufficientDeposit();

    /// @notice Thrown when balance proof validation fails
    error InvalidBalanceProof();

    /// @notice Thrown when signature recovery fails or wrong signer
    error InvalidSignature();

    /// @notice Thrown when nonce not greater than stored nonce
    error InvalidNonce();

    /// @notice Thrown when settlement called too early
    error SettlementTimeoutNotExpired();

    /// @notice Thrown when deposit exceeds maximum channel deposit limit
    error DepositLimitExceeded();

    /// @notice Thrown when attempting to force-close channel before expiry
    error ChannelNotExpired();


    /// @notice Thrown when emergency withdraw attempted but contract is not paused
    error ContractNotPaused();

    /// @notice Thrown when claim amount exceeds counterparty's available deposit
    error InsufficientChannelBalance();

    /// @notice Thrown when there is nothing new to claim (no increase in transferred amount)
    error NothingToClaim();

    /// @notice Emitted when a new channel is opened
    /// @param channelId The unique channel identifier
    /// @param participant1 The first channel participant
    /// @param participant2 The second channel participant
    /// @param settlementTimeout The challenge period duration
    event ChannelOpened(
        bytes32 indexed channelId, address indexed participant1, address indexed participant2, uint256 settlementTimeout
    );

    /// @notice Emitted when a participant deposits tokens to a channel
    /// @param channelId The unique channel identifier
    /// @param participant The participant who deposited
    /// @param totalDeposit The new cumulative deposit amount
    event ChannelNewDeposit(bytes32 indexed channelId, address indexed participant, uint256 totalDeposit);

    /// @notice Emitted when a channel is closed (grace period starts)
    /// @param channelId The unique channel identifier
    /// @param closingParticipant The participant who initiated the close
    event ChannelClosed(bytes32 indexed channelId, address indexed closingParticipant);

    /// @notice Emitted when a channel is settled and funds distributed
    /// @param channelId The unique channel identifier
    /// @param participant1Amount The final amount transferred to participant1
    /// @param participant2Amount The final amount transferred to participant2
    event ChannelSettled(bytes32 indexed channelId, uint256 participant1Amount, uint256 participant2Amount);

    /// @notice Emitted when channel is force-closed after expiry
    /// @param channelId The unique channel identifier
    /// @param timestamp Block timestamp when force-closed
    event ChannelClosedByExpiry(bytes32 indexed channelId, uint256 timestamp);

    /// @notice Emitted when a participant claims transferred funds from a channel
    /// @param channelId The unique channel identifier
    /// @param claimant The participant who claimed the funds
    /// @param claimedAmount The amount claimed in this transaction
    /// @param totalClaimed The cumulative amount claimed by this participant
    event ChannelClaimed(bytes32 indexed channelId, address indexed claimant, uint256 claimedAmount, uint256 totalClaimed);

    /// @notice Emitted when owner performs emergency token recovery
    /// @param channelId The channel identifier (if applicable)
    /// @param recipient The address receiving the recovered tokens
    /// @param amount The amount of tokens recovered
    event EmergencyWithdrawal(bytes32 indexed channelId, address indexed recipient, uint256 amount);

    /// @notice Deploy a new TokenNetwork for a specific token
    /// @param _token The ERC20 token address
    /// @param _maxChannelDeposit Maximum deposit per participant per channel (default: 1M tokens scaled by decimals)
    /// @param _maxChannelLifetime Maximum channel lifetime before force-close allowed (default: 365 days)
    constructor(address _token, uint256 _maxChannelDeposit, uint256 _maxChannelLifetime) EIP712("TokenNetwork", "1") Ownable(msg.sender) {
        token = _token;
        maxChannelDeposit = _maxChannelDeposit;
        maxChannelLifetime = _maxChannelLifetime;
    }

    /// @notice Open a new payment channel with another participant
    /// @param participant2 The address of the other channel participant
    /// @param settlementTimeout The challenge period duration in seconds (minimum 1 hour)
    /// @return channelId The unique identifier for the created channel
    /// @dev Computes channelId as keccak256(p1, p2, channelCounter). Emits ChannelOpened event.
    function openChannel(address participant2, uint256 settlementTimeout) external nonReentrant whenNotPaused returns (bytes32) {
        // Validate participants
        if (participant2 == address(0)) revert InvalidParticipant();
        if (msg.sender == participant2) revert InvalidParticipant();

        // Validate settlement timeout
        if (settlementTimeout < MIN_SETTLEMENT_TIMEOUT) revert InvalidSettlementTimeout();

        // Normalize participant order (p1 < p2 lexicographically)
        (address p1, address p2) = msg.sender < participant2 ? (msg.sender, participant2) : (participant2, msg.sender);

        // Compute unique channel ID
        bytes32 channelId = keccak256(abi.encodePacked(p1, p2, channelCounter));
        channelCounter++;

        // Check channel doesn't already exist
        if (channels[channelId].state != ChannelState.NonExistent) revert ChannelAlreadyExists();

        // Initialize channel state
        channels[channelId] = Channel({
            settlementTimeout: settlementTimeout,
            state: ChannelState.Opened,
            closedAt: 0,
            openedAt: block.timestamp,
            participant1: p1,
            participant2: p2
        });

        // Emit event
        emit ChannelOpened(channelId, p1, p2, settlementTimeout);

        return channelId;
    }

    /// @notice Deposit tokens to a channel
    /// @param channelId The unique channel identifier
    /// @param participant The participant whose deposit is being increased
    /// @param totalDeposit The new cumulative deposit amount (not incremental)
    /// @dev Uses SafeERC20 for token transfers. Handles fee-on-transfer tokens by measuring actual balance changes.
    function setTotalDeposit(bytes32 channelId, address participant, uint256 totalDeposit) external nonReentrant whenNotPaused {
        // Validate channel exists and is open
        Channel storage channel = channels[channelId];
        if (channel.state != ChannelState.Opened) revert InvalidChannelState();

        // Validate participant
        if (participant != channel.participant1 && participant != channel.participant2) {
            revert InvalidParticipant();
        }

        // Calculate incremental deposit amount
        uint256 currentDeposit = participants[channelId][participant].deposit;
        if (totalDeposit < currentDeposit) revert InsufficientDeposit();
        uint256 depositAmount = totalDeposit - currentDeposit;

        // Transfer tokens using SafeERC20 and measure actual balance change
        uint256 balanceBefore = IERC20(token).balanceOf(address(this));
        IERC20(token).safeTransferFrom(msg.sender, address(this), depositAmount);
        uint256 balanceAfter = IERC20(token).balanceOf(address(this));
        uint256 actualReceived = balanceAfter - balanceBefore;

        // Validate deposit limit
        uint256 newDeposit = currentDeposit + actualReceived;
        if (newDeposit > maxChannelDeposit) revert DepositLimitExceeded();

        // Update participant deposit state
        participants[channelId][participant].deposit = newDeposit;

        // Emit event
        emit ChannelNewDeposit(channelId, participant, participants[channelId][participant].deposit);
    }

    /// @notice Claim transferred funds from a channel using counterparty's signed balance proof
    /// @param channelId The unique channel identifier
    /// @param balanceProof The balance proof signed by the counterparty (sender)
    /// @param signature The EIP-712 signature of the balance proof
    /// @dev Works on both Opened and Closed channels. During the challenge period after close,
    ///      the receiver can submit claims before settlement returns remaining funds to the depositor.
    ///      Channel state is NOT changed by this function — only tokens are transferred.
    ///      Only the delta since last claim is transferred (prevents double-pay).
    function claimFromChannel(bytes32 channelId, BalanceProof memory balanceProof, bytes memory signature)
        external
        nonReentrant
        whenNotPaused
    {
        // Validate channel exists and is open or closed (claims allowed during challenge period)
        Channel storage channel = channels[channelId];
        if (channel.state != ChannelState.Opened && channel.state != ChannelState.Closed) revert InvalidChannelState();

        // Validate caller is a participant
        if (msg.sender != channel.participant1 && msg.sender != channel.participant2) {
            revert InvalidParticipant();
        }

        // Determine counterparty (the one who signed the balance proof / the sender)
        address counterparty = msg.sender == channel.participant1 ? channel.participant2 : channel.participant1;

        // Validate balance proof channelId matches
        if (balanceProof.channelId != channelId) revert InvalidBalanceProof();

        // Compute EIP-712 struct hash (same format as closeChannel)
        bytes32 structHash = keccak256(
            abi.encode(
                BALANCE_PROOF_TYPEHASH,
                balanceProof.channelId,
                balanceProof.nonce,
                balanceProof.transferredAmount,
                balanceProof.lockedAmount,
                balanceProof.locksRoot
            )
        );

        // Compute EIP-712 digest and recover signer
        bytes32 digest = _hashTypedDataV4(structHash);
        address recovered = ECDSA.recover(digest, signature);

        // Validate signer is the counterparty (sender of funds)
        if (recovered != counterparty) revert InvalidSignature();

        // Validate nonce is greater than stored nonce
        ParticipantState storage counterpartyState = participants[channelId][counterparty];
        if (balanceProof.nonce <= counterpartyState.nonce) revert InvalidNonce();

        // Calculate claimable amount (delta since last claim)
        uint256 previouslyClaimed = claimedAmounts[channelId][msg.sender];
        uint256 newTransferred = balanceProof.transferredAmount;
        if (newTransferred <= previouslyClaimed) revert NothingToClaim();
        uint256 claimAmount = newTransferred - previouslyClaimed;

        // Verify counterparty has enough deposit to cover all claims
        // Available balance = deposit - withdrawn - total claimed by counterparty's peers
        if (counterpartyState.deposit < newTransferred) {
            revert InsufficientChannelBalance();
        }

        // Update counterparty's on-chain state (nonce and transferredAmount)
        counterpartyState.nonce = balanceProof.nonce;
        counterpartyState.transferredAmount = newTransferred;

        // Update claimed amounts tracking
        claimedAmounts[channelId][msg.sender] = newTransferred;

        // Transfer the claimed tokens to the caller
        IERC20(token).safeTransfer(msg.sender, claimAmount);

        // Emit event
        emit ChannelClaimed(channelId, msg.sender, claimAmount, newTransferred);
    }

    /// @notice Close a payment channel, starting the grace period
    /// @param channelId The unique identifier for the channel
    /// @dev Any participant can call. Starts the challenge period during which the receiver
    ///      can submit claims via claimFromChannel. After the grace period, settleChannel
    ///      returns remaining funds to each depositor.
    function closeChannel(bytes32 channelId)
        external
        nonReentrant
        whenNotPaused
    {
        // Validate channel exists and is open
        Channel storage channel = channels[channelId];
        if (channel.state != ChannelState.Opened) revert InvalidChannelState();

        // Validate caller is a participant
        if (msg.sender != channel.participant1 && msg.sender != channel.participant2) {
            revert InvalidParticipant();
        }

        // Update channel state to Closed and record timestamp
        channel.state = ChannelState.Closed;
        channel.closedAt = block.timestamp;

        // Emit event
        emit ChannelClosed(channelId, msg.sender);
    }

    /// @notice Settle a channel after the grace period expires
    /// @param channelId The unique identifier for the channel
    /// @dev Anyone can call after the grace period. Returns remaining funds (deposit minus
    ///      amounts already claimed via claimFromChannel) to each depositor.
    function settleChannel(bytes32 channelId) external nonReentrant whenNotPaused {
        // Validate channel is in Closed state
        Channel storage channel = channels[channelId];
        if (channel.state != ChannelState.Closed) revert InvalidChannelState();

        // Validate grace period has expired
        if (block.timestamp < channel.closedAt + channel.settlementTimeout) {
            revert SettlementTimeoutNotExpired();
        }

        // Each depositor gets back: deposit - what the counterparty already claimed from them
        // claimedAmounts[p2] = tokens p2 received from p1's signed proofs via claimFromChannel
        uint256 participant1Deposit = participants[channelId][channel.participant1].deposit;
        uint256 participant2Deposit = participants[channelId][channel.participant2].deposit;

        // p2 claimed from p1's deposit, p1 claimed from p2's deposit
        uint256 claimedFromP1 = claimedAmounts[channelId][channel.participant2]; // what p2 took from p1
        uint256 claimedFromP2 = claimedAmounts[channelId][channel.participant1]; // what p1 took from p2

        uint256 participant1FinalBalance = participant1Deposit - claimedFromP1;
        uint256 participant2FinalBalance = participant2Deposit - claimedFromP2;

        // Update channel state to Settled
        channel.state = ChannelState.Settled;

        // Return remaining funds to each depositor
        if (participant1FinalBalance > 0) {
            IERC20(token).safeTransfer(channel.participant1, participant1FinalBalance);
        }

        if (participant2FinalBalance > 0) {
            IERC20(token).safeTransfer(channel.participant2, participant2FinalBalance);
        }

        // Emit event
        emit ChannelSettled(channelId, participant1FinalBalance, participant2FinalBalance);
    }

    /// @notice Force-close channel after maximum lifetime expires
    /// @param channelId The unique channel identifier
    /// @dev Anyone can call after channel expires. Uses deposit amounts as final balances.
    function forceCloseExpiredChannel(bytes32 channelId) external nonReentrant whenNotPaused {
        Channel storage channel = channels[channelId];

        // Validate channel is open
        if (channel.state != ChannelState.Opened) revert InvalidChannelState();

        // Validate channel has expired
        if (block.timestamp < channel.openedAt + maxChannelLifetime) {
            revert ChannelNotExpired();
        }

        // Close channel without balance proof (use deposits as final state)
        channel.state = ChannelState.Closed;
        channel.closedAt = block.timestamp;

        // Emit event
        emit ChannelClosedByExpiry(channelId, block.timestamp);
    }

    /// @notice Emergency token recovery for stuck funds (owner only, contract must be paused)
    /// @param channelId The channel identifier (if applicable, use bytes32(0) for general recovery)
    /// @param recipient The address to receive the recovered tokens
    /// @dev Only allowed when contract is paused. Last resort for invalid state recovery.
    function emergencyWithdraw(bytes32 channelId, address recipient) external onlyOwner {
        // Validate contract is paused (emergency situation only)
        if (!paused()) revert ContractNotPaused();

        // Calculate locked tokens in contract
        uint256 lockedAmount = IERC20(token).balanceOf(address(this));

        // Transfer all locked tokens to recipient
        IERC20(token).safeTransfer(recipient, lockedAmount);

        // Emit event for transparency
        emit EmergencyWithdrawal(channelId, recipient, lockedAmount);
    }

    /// @notice Pause all channel operations in emergency
    /// @dev Only owner can pause, emits Paused event
    function pause() external onlyOwner {
        _pause();
    }

    /// @notice Resume all channel operations after emergency
    /// @dev Only owner can unpause, emits Unpaused event
    function unpause() external onlyOwner {
        _unpause();
    }
}
