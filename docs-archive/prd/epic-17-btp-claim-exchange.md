# Epic 17: BTP Off-Chain Claim Exchange Protocol

**Epic Number:** 17

**Goal:** Implement standardized off-chain payment channel claim exchange via BTP protocolData for all three settlement chains (XRP, EVM/Base L2, and Aptos). Enable connectors to send cryptographically signed settlement claims to peers over the existing BTP WebSocket connection without requiring separate communication channels. Build unified claim encoding/decoding infrastructure, implement claim verification workflows, add claim persistence for dispute resolution, and provide telemetry for monitoring claim exchange health. This epic completes the settlement loop by enabling peers to actually exchange claims off-chain after settlement thresholds are reached, resolving the TODO at `unified-settlement-executor.ts:273`.

**Foundation:** This epic builds on the existing BTP (Bilateral Transfer Protocol - RFC-0023) infrastructure from Epic 2, which provides WebSocket connections between peers with sub-protocol multiplexing via `protocolData`. The BTP message format supports both ILP packets AND arbitrary protocol data, enabling us to send settlement claims alongside (or instead of) payment packets. This epic also integrates with the claim signing infrastructure from Epic 9 (XRP), Epic 8 (EVM), and Epic 13 (Aptos).

**Important:** This epic focuses on the **transport layer for claim exchange**, not the blockchain settlement mechanics (those were completed in Epics 8, 9, and 13). The BTP protocol already handles authentication, connection management, and message delivery—we're adding a new `payment-channel-claim` sub-protocol to carry settlement claims between peers.

**Reference:**

- [RFC-0023: Bilateral Transfer Protocol](https://interledger.org/rfcs/0023-bilateral-transfer-protocol/)
- BTP implementation: `packages/connector/src/btp/`
- Claim signers: `xrp-claim-signer.ts`, `aptos-claim-signer.ts`
- Settlement executor: `unified-settlement-executor.ts`

---

## Story 17.1: BTP Claim Message Protocol Definition

As a protocol designer,
I want a standardized BTP sub-protocol message format for payment channel claims,
so that all three blockchain types (XRP, EVM, Aptos) can exchange claims using consistent encoding.

**Prerequisites:** None (extends existing BTP types)

### Acceptance Criteria

1. `BTPClaimMessage` interface defined in `packages/connector/src/btp/btp-claim-types.ts`
2. Message format supports all three blockchain types: `xrp`, `evm`, `aptos`
3. Message includes: blockchain type, channel ID, amount, signature, public key, nonce, timestamp
4. Message uses JSON encoding for human readability and debugging
5. Protocol name standardized as `payment-channel-claim`
6. Content type code defined as `1` (application/json)
7. Message validation function ensures all required fields present
8. Message validation enforces blockchain-specific constraints (e.g., XRP amount in drops)
9. TypeScript type guards implemented for each blockchain type
10. Unit tests verify message creation, serialization, and validation

### BTP Claim Message Specification

```typescript
// packages/connector/src/btp/btp-claim-types.ts

/**
 * Blockchain type discriminator for payment channel claims
 */
export type BlockchainType = 'xrp' | 'evm' | 'aptos';

/**
 * Base payment channel claim structure (common fields)
 */
export interface BaseClaimMessage {
  /** Protocol version for future compatibility */
  version: '1.0';

  /** Blockchain type discriminator */
  blockchain: BlockchainType;

  /** Unique message ID for idempotency tracking */
  messageId: string;

  /** ISO 8601 timestamp when claim was created */
  timestamp: string;

  /** Sender's peer ID (for correlation) */
  senderId: string;
}

/**
 * XRP Ledger payment channel claim
 */
export interface XRPClaimMessage extends BaseClaimMessage {
  blockchain: 'xrp';

  /** Channel ID (64-character hex string) */
  channelId: string;

  /** Cumulative amount in XRP drops (string for bigint precision) */
  amount: string;

  /** ed25519 signature (128 hex characters) */
  signature: string;

  /** ed25519 public key with ED prefix (66 hex characters) */
  publicKey: string;
}

/**
 * EVM (Base L2) payment channel balance proof
 */
export interface EVMClaimMessage extends BaseClaimMessage {
  blockchain: 'evm';

  /** Channel ID (bytes32 hex string) */
  channelId: string;

  /** Balance proof nonce (monotonically increasing) */
  nonce: number;

  /** Cumulative transferred amount (string for bigint precision) */
  transferredAmount: string;

  /** Locked amount (0 for simple transfers) */
  lockedAmount: string;

  /** Merkle root of locks (32-byte hex, zeros for no locks) */
  locksRoot: string;

  /** EIP-712 signature (hex string) */
  signature: string;

  /** Ethereum address of signer */
  signerAddress: string;
}

/**
 * Aptos payment channel claim
 */
export interface AptosClaimMessage extends BaseClaimMessage {
  blockchain: 'aptos';

  /** Channel owner address (Aptos account address) */
  channelOwner: string;

  /** Cumulative amount in octas (string for bigint precision) */
  amount: string;

  /** Balance proof nonce (monotonically increasing) */
  nonce: number;

  /** ed25519 signature (hex string) */
  signature: string;

  /** ed25519 public key (hex string) */
  publicKey: string;
}

/**
 * Union type for all claim message types
 */
export type BTPClaimMessage = XRPClaimMessage | EVMClaimMessage | AptosClaimMessage;

/**
 * Type guards for blockchain-specific messages
 */
export function isXRPClaim(msg: BTPClaimMessage): msg is XRPClaimMessage {
  return msg.blockchain === 'xrp';
}

export function isEVMClaim(msg: BTPClaimMessage): msg is EVMClaimMessage {
  return msg.blockchain === 'evm';
}

export function isAptosClaim(msg: BTPClaimMessage): msg is AptosClaimMessage {
  return msg.blockchain === 'aptos';
}

/**
 * BTP Protocol Constants
 */
export const BTP_CLAIM_PROTOCOL = {
  /** Protocol name for BTP protocolData */
  NAME: 'payment-channel-claim',

  /** Content type: 1 = application/json */
  CONTENT_TYPE: 1,

  /** Current protocol version */
  VERSION: '1.0',
} as const;
```

### Message Validation

```typescript
/**
 * Validate claim message structure and constraints
 *
 * @throws Error if message is invalid
 */
export function validateClaimMessage(msg: unknown): asserts msg is BTPClaimMessage {
  if (!msg || typeof msg !== 'object') {
    throw new Error('Claim message must be an object');
  }

  const claim = msg as Partial<BTPClaimMessage>;

  // Validate common fields
  if (claim.version !== '1.0') {
    throw new Error(`Unsupported claim version: ${claim.version}`);
  }

  if (!claim.blockchain || !['xrp', 'evm', 'aptos'].includes(claim.blockchain)) {
    throw new Error(`Invalid blockchain type: ${claim.blockchain}`);
  }

  if (!claim.messageId || typeof claim.messageId !== 'string') {
    throw new Error('Missing or invalid messageId');
  }

  if (!claim.timestamp || !isValidISO8601(claim.timestamp)) {
    throw new Error('Missing or invalid timestamp');
  }

  if (!claim.senderId || typeof claim.senderId !== 'string') {
    throw new Error('Missing or invalid senderId');
  }

  // Blockchain-specific validation
  switch (claim.blockchain) {
    case 'xrp':
      validateXRPClaim(claim as XRPClaimMessage);
      break;
    case 'evm':
      validateEVMClaim(claim as EVMClaimMessage);
      break;
    case 'aptos':
      validateAptosClaim(claim as AptosClaimMessage);
      break;
  }
}

function validateXRPClaim(claim: Partial<XRPClaimMessage>): void {
  if (!claim.channelId || !/^[0-9A-Fa-f]{64}$/.test(claim.channelId)) {
    throw new Error('Invalid XRP channelId: must be 64-character hex string');
  }

  if (!claim.amount || BigInt(claim.amount) <= 0n) {
    throw new Error('Invalid XRP amount: must be positive drops');
  }

  if (!claim.signature || !/^[0-9A-Fa-f]{128}$/.test(claim.signature)) {
    throw new Error('Invalid XRP signature: must be 128-character hex string');
  }

  if (!claim.publicKey || !/^ED[0-9A-Fa-f]{64}$/i.test(claim.publicKey)) {
    throw new Error('Invalid XRP publicKey: must be ED prefix + 64 hex characters');
  }
}

function validateEVMClaim(claim: Partial<EVMClaimMessage>): void {
  if (!claim.channelId || !/^0x[0-9A-Fa-f]{64}$/.test(claim.channelId)) {
    throw new Error('Invalid EVM channelId: must be bytes32 hex string');
  }

  if (typeof claim.nonce !== 'number' || claim.nonce < 0) {
    throw new Error('Invalid EVM nonce: must be non-negative number');
  }

  if (!claim.transferredAmount || BigInt(claim.transferredAmount) < 0n) {
    throw new Error('Invalid EVM transferredAmount');
  }

  if (!claim.signature || !/^0x[0-9A-Fa-f]+$/.test(claim.signature)) {
    throw new Error('Invalid EVM signature: must be hex string');
  }

  if (!claim.signerAddress || !/^0x[0-9A-Fa-f]{40}$/.test(claim.signerAddress)) {
    throw new Error('Invalid EVM signerAddress: must be Ethereum address');
  }
}

function validateAptosClaim(claim: Partial<AptosClaimMessage>): void {
  if (!claim.channelOwner || !/^0x[0-9A-Fa-f]+$/.test(claim.channelOwner)) {
    throw new Error('Invalid Aptos channelOwner: must be Aptos address');
  }

  if (!claim.amount || BigInt(claim.amount) <= 0n) {
    throw new Error('Invalid Aptos amount: must be positive octas');
  }

  if (typeof claim.nonce !== 'number' || claim.nonce < 0) {
    throw new Error('Invalid Aptos nonce: must be non-negative number');
  }

  if (!claim.signature || !/^[0-9A-Fa-f]+$/.test(claim.signature)) {
    throw new Error('Invalid Aptos signature: must be hex string');
  }

  if (!claim.publicKey || !/^[0-9A-Fa-f]+$/.test(claim.publicKey)) {
    throw new Error('Invalid Aptos publicKey: must be hex string');
  }
}
```

### Technical Notes

**Why JSON Encoding:**

- Human-readable for debugging and logging
- Easy to inspect in packet captures
- Standard serialization across all three chains
- Minimal overhead for settlement (low frequency compared to ILP packets)

**Why Separate Protocol from ILP Packets:**

- Settlement claims are not payment packets
- Claims can be sent independently of packet flow
- Clearer semantics and easier debugging
- Allows future protocol versioning

**BTP Wire Format:**

```
BTP MESSAGE
├─ Type: MESSAGE (6)
├─ Request ID: <correlation-id>
└─ Data:
   ├─ Protocol Data Array:
   │  └─ Entry 0:
   │     ├─ Protocol Name: "payment-channel-claim"
   │     ├─ Content Type: 1 (JSON)
   │     └─ Data: <JSON-encoded claim>
   └─ ILP Packet: (empty for claim-only messages)
```

---

## Story 17.2: Claim Sender Implementation

As a settlement executor,
I want to send signed claims to peers via BTP when settlement thresholds are reached,
so that peers can redeem claims on-chain to receive settlement payments.

**Prerequisites:** Story 17.1 (BTP claim message types)

### Acceptance Criteria

1. `ClaimSender` class implemented in `packages/connector/src/settlement/claim-sender.ts`
2. ClaimSender integrates with `BTPClient` to send claims over existing connections
3. ClaimSender creates blockchain-specific claim messages from signer output
4. ClaimSender generates unique message IDs for idempotency tracking
5. ClaimSender wraps claims in BTP protocolData format
6. ClaimSender implements retry logic for failed claim sends (3 attempts, exponential backoff)
7. ClaimSender logs all claim sends with structured logging
8. ClaimSender emits telemetry events for claim send success/failure
9. ClaimSender stores sent claims in SQLite for dispute resolution
10. Unit tests verify claim sending for all three blockchain types

### ClaimSender Interface

```typescript
// packages/connector/src/settlement/claim-sender.ts

import { Logger } from '../utils/logger';
import type { BTPClient } from '../btp/btp-client';
import type { TelemetryEmitter } from '../telemetry/telemetry-emitter';
import type { BTPClaimMessage } from '../btp/btp-claim-types';
import { Database } from 'better-sqlite3';

export interface ClaimSendResult {
  success: boolean;
  messageId: string;
  timestamp: string;
  error?: string;
}

export class ClaimSender {
  constructor(
    private readonly db: Database,
    private readonly logger: Logger,
    private readonly telemetryEmitter?: TelemetryEmitter,
    private readonly nodeId?: string
  ) {}

  /**
   * Send XRP payment channel claim to peer
   *
   * @param peerId - Destination peer ID
   * @param btpClient - BTP client for this peer
   * @param channelId - XRP channel ID (64-char hex)
   * @param amount - Amount in drops (string)
   * @param signature - ed25519 signature (128-char hex)
   * @param publicKey - ed25519 public key (66-char hex with ED prefix)
   * @returns Send result with message ID
   */
  async sendXRPClaim(
    peerId: string,
    btpClient: BTPClient,
    channelId: string,
    amount: string,
    signature: string,
    publicKey: string
  ): Promise<ClaimSendResult>;

  /**
   * Send EVM payment channel balance proof to peer
   *
   * @param peerId - Destination peer ID
   * @param btpClient - BTP client for this peer
   * @param channelId - EVM channel ID (bytes32 hex)
   * @param nonce - Balance proof nonce
   * @param transferredAmount - Cumulative amount (string)
   * @param lockedAmount - Locked amount (string, usually "0")
   * @param locksRoot - Locks merkle root (32-byte hex)
   * @param signature - EIP-712 signature (hex)
   * @param signerAddress - Ethereum address
   * @returns Send result with message ID
   */
  async sendEVMClaim(
    peerId: string,
    btpClient: BTPClient,
    channelId: string,
    nonce: number,
    transferredAmount: string,
    lockedAmount: string,
    locksRoot: string,
    signature: string,
    signerAddress: string
  ): Promise<ClaimSendResult>;

  /**
   * Send Aptos payment channel claim to peer
   *
   * @param peerId - Destination peer ID
   * @param btpClient - BTP client for this peer
   * @param channelOwner - Aptos channel owner address
   * @param amount - Amount in octas (string)
   * @param nonce - Claim nonce
   * @param signature - ed25519 signature (hex)
   * @param publicKey - ed25519 public key (hex)
   * @returns Send result with message ID
   */
  async sendAptosClaim(
    peerId: string,
    btpClient: BTPClient,
    channelOwner: string,
    amount: string,
    nonce: number,
    signature: string,
    publicKey: string
  ): Promise<ClaimSendResult>;

  /**
   * Send generic claim message (internal)
   * Handles BTP wrapping, retry logic, persistence, telemetry
   */
  private async sendClaim(
    peerId: string,
    btpClient: BTPClient,
    claimMessage: BTPClaimMessage
  ): Promise<ClaimSendResult>;
}
```

### Implementation Details

**Message ID Generation:**

```typescript
function generateMessageId(
  blockchain: BlockchainType,
  channelId: string,
  nonce: number | undefined
): string {
  // Format: <blockchain>-<channelId-prefix>-<nonce>-<timestamp>
  const prefix = channelId.substring(0, 8);
  const nonceStr = nonce !== undefined ? nonce.toString() : 'n/a';
  const timestamp = Date.now();
  return `${blockchain}-${prefix}-${nonceStr}-${timestamp}`;
}
```

**BTP Message Construction:**

```typescript
import { BTPMessageType } from '../btp/btp-types';
import { serializeBTPMessage } from '../btp/btp-message-parser';
import { BTP_CLAIM_PROTOCOL } from '../btp/btp-claim-types';

function createBTPClaimMessage(claimMessage: BTPClaimMessage, requestId: number): Buffer {
  const btpMessage = {
    type: BTPMessageType.MESSAGE,
    requestId,
    data: {
      protocolData: [
        {
          protocolName: BTP_CLAIM_PROTOCOL.NAME,
          contentType: BTP_CLAIM_PROTOCOL.CONTENT_TYPE,
          data: Buffer.from(JSON.stringify(claimMessage), 'utf8'),
        },
      ],
      // No ILP packet needed for claims
    },
  };

  return serializeBTPMessage(btpMessage);
}
```

**Retry Logic:**

```typescript
async function sendWithRetry(
  btpClient: BTPClient,
  message: Buffer,
  maxAttempts: number = 3
): Promise<void> {
  for (let attempt = 1; attempt <= maxAttempts; attempt++) {
    try {
      await btpClient.sendRawMessage(message);
      return; // Success
    } catch (error) {
      if (attempt === maxAttempts) {
        throw error; // Final attempt failed
      }

      // Exponential backoff: 1s, 2s, 4s
      const delay = Math.pow(2, attempt - 1) * 1000;
      await new Promise((resolve) => setTimeout(resolve, delay));
    }
  }
}
```

**Claim Persistence:**

```typescript
// Store sent claims for dispute resolution and audit trail
function persistSentClaim(
  db: Database,
  peerId: string,
  messageId: string,
  claim: BTPClaimMessage
): void {
  db.prepare(
    `
    INSERT INTO sent_claims (
      message_id, peer_id, blockchain, claim_data, sent_at
    ) VALUES (?, ?, ?, ?, ?)
  `
  ).run(messageId, peerId, claim.blockchain, JSON.stringify(claim), Date.now());
}
```

**Telemetry Events:**

```typescript
telemetryEmitter.emit({
  type: 'CLAIM_SENT',
  nodeId: this.nodeId ?? 'unknown',
  peerId,
  blockchain: claimMessage.blockchain,
  messageId: claimMessage.messageId,
  amount: getClaimAmount(claimMessage),
  success: true,
  timestamp: new Date().toISOString(),
});
```

### Database Schema

```sql
-- Add to existing schema migrations
CREATE TABLE IF NOT EXISTS sent_claims (
  message_id TEXT PRIMARY KEY,
  peer_id TEXT NOT NULL,
  blockchain TEXT NOT NULL,  -- 'xrp', 'evm', 'aptos'
  claim_data TEXT NOT NULL,  -- JSON-encoded claim message
  sent_at INTEGER NOT NULL,  -- Unix timestamp ms
  ack_received_at INTEGER,   -- Unix timestamp ms (NULL until ack)
  FOREIGN KEY (peer_id) REFERENCES peers(id)
);

CREATE INDEX idx_sent_claims_peer ON sent_claims(peer_id);
CREATE INDEX idx_sent_claims_sent_at ON sent_claims(sent_at);
```

---

## Story 17.3: Claim Receiver and Verification

As a connector receiving claims,
I want to receive, validate, and verify claims from peers via BTP,
so that I can securely redeem settlement payments on-chain.

**Prerequisites:** Story 17.1 (BTP claim types), Story 17.2 (Claim sender)

### Acceptance Criteria

1. `ClaimReceiver` class implemented in `packages/connector/src/settlement/claim-receiver.ts`
2. ClaimReceiver registers handler for `payment-channel-claim` protocol data
3. ClaimReceiver parses and validates incoming claim messages
4. ClaimReceiver routes claims to blockchain-specific verifiers
5. ClaimReceiver integrates with `XRPClaimSigner.verifyClaim()` for XRP claims
6. ClaimReceiver integrates with EVM balance proof verification for EVM claims
7. ClaimReceiver integrates with `AptosClaimSigner.verifyClaim()` for Aptos claims
8. ClaimReceiver stores verified claims in SQLite for later redemption
9. ClaimReceiver sends BTP acknowledgment response to sender
10. ClaimReceiver emits telemetry events for claim reception and verification

### ClaimReceiver Interface

```typescript
// packages/connector/src/settlement/claim-receiver.ts

import { Logger } from '../utils/logger';
import type { BTPServer } from '../btp/btp-server';
import type { BTPProtocolData } from '../btp/btp-types';
import type { XRPClaimSigner } from './xrp-claim-signer';
import type { AptosClaimSigner } from './aptos-claim-signer';
import type { PaymentChannelSDK } from './payment-channel-sdk';
import type { TelemetryEmitter } from '../telemetry/telemetry-emitter';
import type { BTPClaimMessage } from '../btp/btp-claim-types';
import { Database } from 'better-sqlite3';

export interface ClaimVerificationResult {
  valid: boolean;
  messageId: string;
  error?: string;
}

export class ClaimReceiver {
  constructor(
    private readonly db: Database,
    private readonly xrpClaimSigner: XRPClaimSigner,
    private readonly evmChannelSDK: PaymentChannelSDK,
    private readonly aptosClaimSigner: AptosClaimSigner,
    private readonly logger: Logger,
    private readonly telemetryEmitter?: TelemetryEmitter,
    private readonly nodeId?: string
  ) {}

  /**
   * Register claim handler with BTP server
   * Called during connector initialization
   */
  registerWithBTPServer(btpServer: BTPServer): void {
    btpServer.on('protocolData', async (peerId, protocolData) => {
      if (protocolData.protocolName === 'payment-channel-claim') {
        await this.handleClaimMessage(peerId, protocolData);
      }
    });
  }

  /**
   * Handle incoming claim message from peer
   * Validates, verifies, stores, and acknowledges
   */
  private async handleClaimMessage(peerId: string, protocolData: BTPProtocolData): Promise<void>;

  /**
   * Verify XRP claim signature and constraints
   */
  private async verifyXRPClaim(
    claim: XRPClaimMessage,
    peerId: string
  ): Promise<ClaimVerificationResult>;

  /**
   * Verify EVM balance proof signature and constraints
   */
  private async verifyEVMClaim(
    claim: EVMClaimMessage,
    peerId: string
  ): Promise<ClaimVerificationResult>;

  /**
   * Verify Aptos claim signature and constraints
   */
  private async verifyAptosClaim(
    claim: AptosClaimMessage,
    peerId: string
  ): Promise<ClaimVerificationResult>;

  /**
   * Get latest verified claim for a channel
   * Used to determine which claim to redeem on-chain
   */
  getLatestVerifiedClaim(
    peerId: string,
    blockchain: BlockchainType,
    channelId: string
  ): Promise<BTPClaimMessage | null>;
}
```

### Implementation Details

**Protocol Data Handler:**

```typescript
private async handleClaimMessage(
  peerId: string,
  protocolData: BTPProtocolData
): Promise<void> {
  const logger = this.logger.child({ peerId, protocol: 'claim-receiver' });

  try {
    // Parse JSON claim message
    const claimMessage = JSON.parse(protocolData.data.toString('utf8'));

    // Validate message structure
    validateClaimMessage(claimMessage);

    logger.info({
      messageId: claimMessage.messageId,
      blockchain: claimMessage.blockchain,
      amount: getClaimAmount(claimMessage)
    }, 'Received claim message');

    // Verify claim signature
    let verificationResult: ClaimVerificationResult;

    switch (claimMessage.blockchain) {
      case 'xrp':
        verificationResult = await this.verifyXRPClaim(claimMessage, peerId);
        break;
      case 'evm':
        verificationResult = await this.verifyEVMClaim(claimMessage, peerId);
        break;
      case 'aptos':
        verificationResult = await this.verifyAptosClaim(claimMessage, peerId);
        break;
    }

    if (verificationResult.valid) {
      // Store verified claim
      this.persistReceivedClaim(peerId, claimMessage);

      logger.info({ messageId: claimMessage.messageId }, 'Claim verified and stored');

      // Emit telemetry
      this.emitClaimReceivedTelemetry(peerId, claimMessage, true);
    } else {
      logger.warn({
        messageId: claimMessage.messageId,
        error: verificationResult.error
      }, 'Claim verification failed');

      this.emitClaimReceivedTelemetry(peerId, claimMessage, false, verificationResult.error);
    }

  } catch (error) {
    logger.error({
      error: error instanceof Error ? error.message : 'Unknown error'
    }, 'Failed to process claim message');
  }
}
```

**XRP Claim Verification:**

```typescript
private async verifyXRPClaim(
  claim: XRPClaimMessage,
  peerId: string
): Promise<ClaimVerificationResult> {
  try {
    // Use existing XRPClaimSigner verification
    const isValid = await this.xrpClaimSigner.verifyClaim(
      claim.channelId,
      claim.amount,
      claim.signature,
      claim.publicKey
    );

    if (!isValid) {
      return {
        valid: false,
        messageId: claim.messageId,
        error: 'Invalid signature'
      };
    }

    // Check if claim amount is greater than previous claims
    const latestClaim = await this.xrpClaimSigner.getLatestClaim(claim.channelId);
    if (latestClaim && BigInt(claim.amount) <= BigInt(latestClaim.amount)) {
      return {
        valid: false,
        messageId: claim.messageId,
        error: `Claim amount not monotonically increasing: ${claim.amount} <= ${latestClaim.amount}`
      };
    }

    return {
      valid: true,
      messageId: claim.messageId
    };

  } catch (error) {
    return {
      valid: false,
      messageId: claim.messageId,
      error: error instanceof Error ? error.message : 'Verification error'
    };
  }
}
```

**EVM Claim Verification:**

```typescript
private async verifyEVMClaim(
  claim: EVMClaimMessage,
  peerId: string
): Promise<ClaimVerificationResult> {
  try {
    // Use PaymentChannelSDK to verify EIP-712 signature
    const balanceProof = {
      channelId: claim.channelId,
      nonce: claim.nonce,
      transferredAmount: claim.transferredAmount,
      lockedAmount: claim.lockedAmount,
      locksRoot: claim.locksRoot
    };

    const isValid = await this.evmChannelSDK.verifyBalanceProof(
      balanceProof,
      claim.signature,
      claim.signerAddress
    );

    if (!isValid) {
      return {
        valid: false,
        messageId: claim.messageId,
        error: 'Invalid EIP-712 signature'
      };
    }

    // Check nonce monotonicity
    const latestProof = await this.getLatestVerifiedClaim(peerId, 'evm', claim.channelId);
    if (latestProof && isEVMClaim(latestProof)) {
      if (claim.nonce <= latestProof.nonce) {
        return {
          valid: false,
          messageId: claim.messageId,
          error: `Nonce not monotonically increasing: ${claim.nonce} <= ${latestProof.nonce}`
        };
      }
    }

    return {
      valid: true,
      messageId: claim.messageId
    };

  } catch (error) {
    return {
      valid: false,
      messageId: claim.messageId,
      error: error instanceof Error ? error.message : 'Verification error'
    };
  }
}
```

**Aptos Claim Verification:**

```typescript
private async verifyAptosClaim(
  claim: AptosClaimMessage,
  peerId: string
): Promise<ClaimVerificationResult> {
  try {
    // Use AptosClaimSigner verification
    const isValid = await this.aptosClaimSigner.verifyClaim(
      claim.channelOwner,
      claim.amount,
      claim.nonce,
      claim.signature,
      claim.publicKey
    );

    if (!isValid) {
      return {
        valid: false,
        messageId: claim.messageId,
        error: 'Invalid signature'
      };
    }

    // Check nonce monotonicity
    const latestClaim = await this.getLatestVerifiedClaim(peerId, 'aptos', claim.channelOwner);
    if (latestClaim && isAptosClaim(latestClaim)) {
      if (claim.nonce <= latestClaim.nonce) {
        return {
          valid: false,
          messageId: claim.messageId,
          error: `Nonce not monotonically increasing: ${claim.nonce} <= ${latestClaim.nonce}`
        };
      }
    }

    return {
      valid: true,
      messageId: claim.messageId
    };

  } catch (error) {
    return {
      valid: false,
      messageId: claim.messageId,
      error: error instanceof Error ? error.message : 'Verification error'
    };
  }
}
```

### Database Schema

```sql
-- Table for received claims (incoming from peers)
CREATE TABLE IF NOT EXISTS received_claims (
  message_id TEXT PRIMARY KEY,
  peer_id TEXT NOT NULL,
  blockchain TEXT NOT NULL,  -- 'xrp', 'evm', 'aptos'
  channel_id TEXT NOT NULL,  -- Channel/owner identifier
  claim_data TEXT NOT NULL,  -- JSON-encoded claim message
  verified BOOLEAN NOT NULL, -- Verification result
  received_at INTEGER NOT NULL,  -- Unix timestamp ms
  redeemed_at INTEGER,       -- Unix timestamp ms (NULL until redeemed on-chain)
  redemption_tx_hash TEXT,   -- On-chain transaction hash
  FOREIGN KEY (peer_id) REFERENCES peers(id)
);

CREATE INDEX idx_received_claims_peer ON received_claims(peer_id);
CREATE INDEX idx_received_claims_blockchain_channel ON received_claims(blockchain, channel_id);
CREATE INDEX idx_received_claims_redeemed ON received_claims(redeemed_at) WHERE redeemed_at IS NOT NULL;
```

---

## Story 17.4: UnifiedSettlementExecutor Integration

As a settlement system,
I want the UnifiedSettlementExecutor to use ClaimSender when settlement thresholds are reached,
so that signed claims are automatically sent to peers via BTP.

**Prerequisites:** Story 17.2 (ClaimSender), existing UnifiedSettlementExecutor

### Acceptance Criteria

1. `UnifiedSettlementExecutor` extended to inject `ClaimSender` dependency
2. Settlement executor calls `ClaimSender.sendXRPClaim()` after signing XRP claims
3. Settlement executor calls `ClaimSender.sendEVMClaim()` after creating EVM balance proofs
4. Settlement executor calls `ClaimSender.sendAptosClaim()` after signing Aptos claims
5. Settlement executor retrieves `BTPClient` for peer from connection manager
6. Settlement executor handles claim send failures gracefully (logs error, allows retry)
7. Settlement executor marks settlement complete only after claim successfully sent
8. TODO at `unified-settlement-executor.ts:273` removed and replaced with working implementation
9. Unit tests verify claim sending for all three blockchain types
10. Integration test demonstrates end-to-end settlement with claim exchange

### Code Changes

```typescript
// packages/connector/src/settlement/unified-settlement-executor.ts

import { ClaimSender } from './claim-sender';
import type { BTPConnectionManager } from '../btp/btp-connection-manager';

export class UnifiedSettlementExecutor extends EventEmitter {
  constructor(
    // ... existing dependencies
    private readonly claimSender: ClaimSender,
    private readonly btpConnectionManager: BTPConnectionManager
    // ...
  ) {
    super();
  }

  /**
   * Settle via XRP payment channels (updated)
   *
   * Creates channel if needed, signs claim, sends claim to peer via BTP.
   */
  private async settleViaXRP(peerId: string, amount: string, config: PeerConfig): Promise<void> {
    this.logger.info({ peerId, amount }, 'Settling via XRP payment channel...');

    if (!config.xrpAddress) {
      throw new Error(`Peer ${peerId} missing xrpAddress for XRP settlement`);
    }

    // Find or create XRP payment channel
    const channelId = await this.findOrCreateXRPChannel(config.xrpAddress, amount);

    // Sign claim for amount
    const signature = await this.xrpClaimSigner.signClaim(channelId, amount);
    const publicKey = await this.xrpClaimSigner.getPublicKey();

    // Get BTP client for peer
    const btpClient = this.btpConnectionManager.getClientForPeer(peerId);
    if (!btpClient) {
      throw new Error(`No BTP connection to peer ${peerId}`);
    }

    // Send claim to peer via BTP (replaces TODO)
    const result = await this.claimSender.sendXRPClaim(
      peerId,
      btpClient,
      channelId,
      amount,
      signature,
      publicKey
    );

    if (!result.success) {
      throw new Error(`Failed to send XRP claim to peer: ${result.error}`);
    }

    this.logger.info(
      {
        peerId,
        channelId,
        amount,
        messageId: result.messageId,
      },
      'XRP claim sent to peer successfully'
    );
  }

  /**
   * Settle via EVM payment channels (updated)
   *
   * Creates channel if needed, signs balance proof, sends to peer via BTP.
   */
  private async settleViaEVM(
    peerId: string,
    amount: string,
    tokenAddress: string,
    config: PeerConfig
  ): Promise<void> {
    this.logger.info({ peerId, amount, tokenAddress }, 'Settling via EVM payment channel...');

    if (!config.evmAddress) {
      throw new Error(`Peer ${peerId} missing evmAddress for EVM settlement`);
    }

    // Find or create EVM channel
    const channelId = await this.findOrCreateEVMChannel(config.evmAddress, tokenAddress, amount);

    // Create and sign balance proof
    const nonce = await this.evmChannelSDK.getNextNonce(channelId);
    const balanceProof = await this.evmChannelSDK.signBalanceProof({
      channelId,
      nonce,
      transferredAmount: amount,
      lockedAmount: '0',
      locksRoot: '0x' + '0'.repeat(64), // No locks
    });

    // Get BTP client for peer
    const btpClient = this.btpConnectionManager.getClientForPeer(peerId);
    if (!btpClient) {
      throw new Error(`No BTP connection to peer ${peerId}`);
    }

    // Send balance proof to peer via BTP
    const result = await this.claimSender.sendEVMClaim(
      peerId,
      btpClient,
      channelId,
      nonce,
      amount,
      '0',
      balanceProof.locksRoot,
      balanceProof.signature,
      await this.evmChannelSDK.getSignerAddress()
    );

    if (!result.success) {
      throw new Error(`Failed to send EVM claim to peer: ${result.error}`);
    }

    this.logger.info(
      {
        peerId,
        channelId,
        amount,
        messageId: result.messageId,
      },
      'EVM balance proof sent to peer successfully'
    );
  }

  /**
   * Settle via Aptos payment channels (updated)
   *
   * Creates channel if needed, signs claim, sends to peer via BTP.
   */
  private async settleViaAptos(peerId: string, amount: string, config: PeerConfig): Promise<void> {
    this.logger.info({ peerId, amount }, 'Settling via Aptos payment channel...');

    if (!config.aptosAddress) {
      throw new Error(`Peer ${peerId} missing aptosAddress for Aptos settlement`);
    }

    // Find or create Aptos channel
    const channelOwner = await this.findOrCreateAptosChannel(config.aptosAddress, amount);

    // Get next nonce and sign claim
    const nonce = await this.aptosClaimSigner.getNextNonce(channelOwner);
    const signature = await this.aptosClaimSigner.signClaim(channelOwner, amount, nonce);
    const publicKey = await this.aptosClaimSigner.getPublicKey();

    // Get BTP client for peer
    const btpClient = this.btpConnectionManager.getClientForPeer(peerId);
    if (!btpClient) {
      throw new Error(`No BTP connection to peer ${peerId}`);
    }

    // Send claim to peer via BTP
    const result = await this.claimSender.sendAptosClaim(
      peerId,
      btpClient,
      channelOwner,
      amount,
      nonce,
      signature,
      publicKey
    );

    if (!result.success) {
      throw new Error(`Failed to send Aptos claim to peer: ${result.error}`);
    }

    this.logger.info(
      {
        peerId,
        channelOwner,
        amount,
        messageId: result.messageId,
      },
      'Aptos claim sent to peer successfully'
    );
  }
}
```

### BTP Connection Manager

```typescript
// packages/connector/src/btp/btp-connection-manager.ts

/**
 * Manages BTP client connections to all peers
 * Provides lookup interface for ClaimSender
 */
export class BTPConnectionManager {
  private clients: Map<string, BTPClient> = new Map();

  /**
   * Get BTP client for a specific peer
   * @param peerId - Peer identifier
   * @returns BTPClient if connected, undefined if not
   */
  getClientForPeer(peerId: string): BTPClient | undefined {
    return this.clients.get(peerId);
  }

  /**
   * Check if peer is connected
   */
  isConnected(peerId: string): boolean {
    const client = this.clients.get(peerId);
    return client?.isConnected ?? false;
  }
}
```

---

## Story 17.5: Automatic Claim Redemption

As a connector receiving verified claims,
I want to automatically redeem claims on-chain when profitable,
so that settlement payments are finalized without manual intervention.

**Prerequisites:** Story 17.3 (ClaimReceiver), existing blockchain SDKs

### Acceptance Criteria

1. `ClaimRedemptionService` class implemented in `packages/connector/src/settlement/claim-redemption-service.ts`
2. Service polls for unredeemed verified claims every 60 seconds
3. Service calculates gas costs for redemption (chain-specific)
4. Service only redeems claims where `claimAmount - gasCost > threshold` (configurable)
5. Service calls XRP PayChan claim submission for XRP claims
6. Service calls EVM TokenNetwork closeChannel() with balance proof for EVM claims
7. Service calls Aptos claim() entry function for Aptos claims
8. Service updates database with redemption transaction hash
9. Service emits telemetry for successful/failed redemptions
10. Service handles redemption failures with exponential backoff retry

### ClaimRedemptionService Interface

```typescript
// packages/connector/src/settlement/claim-redemption-service.ts

import { Logger } from '../utils/logger';
import type { XRPChannelSDK } from './xrp-channel-sdk';
import type { PaymentChannelSDK } from './payment-channel-sdk';
import type { AptosChannelSDK } from './aptos-channel-sdk';
import type { TelemetryEmitter } from '../telemetry/telemetry-emitter';
import { Database } from 'better-sqlite3';

export interface RedemptionConfig {
  /** Minimum profit threshold (in native token units) */
  minProfitThreshold: bigint;

  /** Polling interval in milliseconds */
  pollingInterval: number;

  /** Maximum concurrent redemptions */
  maxConcurrentRedemptions: number;
}

export class ClaimRedemptionService {
  private pollingTimer?: NodeJS.Timer;
  private isRunning = false;

  constructor(
    private readonly db: Database,
    private readonly xrpChannelSDK: XRPChannelSDK,
    private readonly evmChannelSDK: PaymentChannelSDK,
    private readonly aptosChannelSDK: AptosChannelSDK,
    private readonly config: RedemptionConfig,
    private readonly logger: Logger,
    private readonly telemetryEmitter?: TelemetryEmitter,
    private readonly nodeId?: string
  ) {}

  /**
   * Start automatic claim redemption service
   */
  start(): void {
    if (this.isRunning) {
      this.logger.warn('Claim redemption service already running');
      return;
    }

    this.isRunning = true;
    this.logger.info(
      {
        pollingInterval: this.config.pollingInterval,
        minProfitThreshold: this.config.minProfitThreshold.toString(),
      },
      'Starting claim redemption service'
    );

    this.pollingTimer = setInterval(() => this.processRedemptions(), this.config.pollingInterval);

    // Run immediately on start
    this.processRedemptions();
  }

  /**
   * Stop automatic claim redemption service
   */
  stop(): void {
    if (this.pollingTimer) {
      clearInterval(this.pollingTimer);
      this.pollingTimer = undefined;
    }
    this.isRunning = false;
    this.logger.info('Claim redemption service stopped');
  }

  /**
   * Process pending redemptions (poll cycle)
   */
  private async processRedemptions(): Promise<void>;

  /**
   * Redeem XRP claim on XRP Ledger
   */
  private async redeemXRPClaim(claim: XRPClaimMessage): Promise<string>;

  /**
   * Redeem EVM balance proof on Base L2
   */
  private async redeemEVMClaim(claim: EVMClaimMessage): Promise<string>;

  /**
   * Redeem Aptos claim on Aptos blockchain
   */
  private async redeemAptosClaim(claim: AptosClaimMessage): Promise<string>;

  /**
   * Calculate estimated gas cost for redemption
   */
  private async estimateRedemptionCost(
    blockchain: BlockchainType,
    claim: BTPClaimMessage
  ): Promise<bigint>;

  /**
   * Check if redemption is profitable
   */
  private isProfitable(claimAmount: bigint, gasCost: bigint): boolean {
    const profit = claimAmount - gasCost;
    return profit >= this.config.minProfitThreshold;
  }
}
```

### Implementation Details

**Redemption Polling:**

```typescript
private async processRedemptions(): Promise<void> {
  try {
    // Get unredeemed verified claims
    const claims = this.db.prepare(`
      SELECT message_id, peer_id, blockchain, claim_data
      FROM received_claims
      WHERE verified = 1
        AND redeemed_at IS NULL
      ORDER BY received_at ASC
      LIMIT ?
    `).all(this.config.maxConcurrentRedemptions);

    if (claims.length === 0) {
      return; // No claims to redeem
    }

    this.logger.info({ count: claims.length }, 'Processing claim redemptions');

    // Process claims in parallel (up to max concurrent)
    await Promise.allSettled(
      claims.map(row => this.redeemClaim(JSON.parse(row.claim_data)))
    );

  } catch (error) {
    this.logger.error({
      error: error instanceof Error ? error.message : 'Unknown error'
    }, 'Failed to process redemptions');
  }
}
```

**Gas Estimation:**

```typescript
private async estimateRedemptionCost(
  blockchain: BlockchainType,
  claim: BTPClaimMessage
): Promise<bigint> {
  switch (blockchain) {
    case 'xrp':
      // XRP transaction fee is fixed at 10 drops (0.00001 XRP)
      return 10n;

    case 'evm':
      // Estimate EVM gas for closeChannel() transaction
      const evmClaim = claim as EVMClaimMessage;
      const gasPrice = await this.evmChannelSDK.getGasPrice();
      const gasLimit = await this.evmChannelSDK.estimateCloseChannelGas(
        evmClaim.channelId,
        evmClaim
      );
      return gasPrice * gasLimit;

    case 'aptos':
      // Aptos gas fees are very low (estimate ~0.0001 APT = 10,000 octas)
      return 10000n;
  }
}
```

**XRP Redemption:**

```typescript
private async redeemXRPClaim(claim: XRPClaimMessage): Promise<string> {
  this.logger.info({
    messageId: claim.messageId,
    channelId: claim.channelId,
    amount: claim.amount
  }, 'Redeeming XRP claim on-chain');

  // Submit PaymentChannelClaim transaction to XRPL
  const txHash = await this.xrpChannelSDK.submitClaim(
    claim.channelId,
    claim.amount,
    claim.signature,
    claim.publicKey
  );

  // Update database
  this.db.prepare(`
    UPDATE received_claims
    SET redeemed_at = ?, redemption_tx_hash = ?
    WHERE message_id = ?
  `).run(Date.now(), txHash, claim.messageId);

  this.logger.info({
    messageId: claim.messageId,
    txHash
  }, 'XRP claim redeemed successfully');

  return txHash;
}
```

### Configuration

```typescript
// Environment variables
CLAIM_REDEMPTION_ENABLED = true; // Enable automatic redemption
CLAIM_REDEMPTION_MIN_PROFIT = 1000; // Minimum profit in drops/wei/octas
CLAIM_REDEMPTION_INTERVAL = 60000; // Poll every 60 seconds
CLAIM_REDEMPTION_MAX_CONCURRENT = 5; // Max parallel redemptions
```

---

## Story 17.6: Telemetry and Monitoring

As an operator,
I want comprehensive telemetry for claim exchange operations,
so that I can monitor settlement health and debug issues.

**Prerequisites:** All previous stories, existing telemetry infrastructure

### Acceptance Criteria

1. Telemetry events defined for all claim operations
2. Events include: CLAIM_SENT, CLAIM_RECEIVED, CLAIM_VERIFIED, CLAIM_REDEEMED
3. Explorer UI updated to display claim exchange events
4. Explorer UI shows claim timeline: sent → received → verified → redeemed
5. Explorer UI displays claim details (amount, signatures, blockchain)
6. Prometheus metrics added for claim send/receive/redemption rates
7. Prometheus metrics added for claim verification failures
8. Alert rules defined for high claim failure rates
9. Documentation updated with troubleshooting guide
10. Dashboard shows tri-chain claim exchange statistics

### Telemetry Event Definitions

```typescript
// packages/shared/src/types/telemetry.ts

export interface ClaimSentEvent extends TelemetryEvent {
  type: 'CLAIM_SENT';
  nodeId: string;
  peerId: string;
  blockchain: 'xrp' | 'evm' | 'aptos';
  messageId: string;
  channelId: string;
  amount: string;
  success: boolean;
  error?: string;
  timestamp: string;
}

export interface ClaimReceivedEvent extends TelemetryEvent {
  type: 'CLAIM_RECEIVED';
  nodeId: string;
  peerId: string;
  blockchain: 'xrp' | 'evm' | 'aptos';
  messageId: string;
  channelId: string;
  amount: string;
  verified: boolean;
  error?: string;
  timestamp: string;
}

export interface ClaimRedeemedEvent extends TelemetryEvent {
  type: 'CLAIM_REDEEMED';
  nodeId: string;
  peerId: string;
  blockchain: 'xrp' | 'evm' | 'aptos';
  messageId: string;
  channelId: string;
  amount: string;
  txHash: string;
  gasCost: string;
  success: boolean;
  error?: string;
  timestamp: string;
}
```

### Prometheus Metrics

```typescript
// packages/connector/src/telemetry/metrics.ts

// Claim send metrics
export const claimsSentTotal = new Counter({
  name: 'claims_sent_total',
  help: 'Total claims sent to peers',
  labelNames: ['peer_id', 'blockchain', 'success'],
});

export const claimsReceivedTotal = new Counter({
  name: 'claims_received_total',
  help: 'Total claims received from peers',
  labelNames: ['peer_id', 'blockchain', 'verified'],
});

export const claimsRedeemedTotal = new Counter({
  name: 'claims_redeemed_total',
  help: 'Total claims redeemed on-chain',
  labelNames: ['blockchain', 'success'],
});

export const claimRedemptionLatency = new Histogram({
  name: 'claim_redemption_latency_seconds',
  help: 'Time from claim receipt to on-chain redemption',
  labelNames: ['blockchain'],
  buckets: [1, 5, 10, 30, 60, 300, 600, 1800], // 1s to 30min
});

export const claimVerificationFailures = new Counter({
  name: 'claim_verification_failures_total',
  help: 'Total claim verification failures',
  labelNames: ['peer_id', 'blockchain', 'error_type'],
});
```

### Alert Rules

```yaml
# monitoring/prometheus/alert-rules.yml

groups:
  - name: claim_exchange_alerts
    interval: 30s
    rules:
      - alert: HighClaimSendFailureRate
        expr: |
          rate(claims_sent_total{success="false"}[5m])
          / rate(claims_sent_total[5m]) > 0.1
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: 'High claim send failure rate ({{ $value | humanizePercentage }})'
          description: 'More than 10% of claim sends are failing for 5 minutes'

      - alert: HighClaimVerificationFailureRate
        expr: |
          rate(claim_verification_failures_total[5m])
          / rate(claims_received_total[5m]) > 0.05
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: 'High claim verification failure rate ({{ $value | humanizePercentage }})'
          description: 'More than 5% of received claims are failing verification'

      - alert: ClaimRedemptionStalled
        expr: |
          (time() - claim_last_redemption_timestamp_seconds) > 600
          and rate(claims_received_total{verified="true"}[10m]) > 0
        for: 5m
        labels:
          severity: critical
        annotations:
          summary: 'Claim redemption service appears stalled'
          description: 'No claims redeemed in 10 minutes despite verified claims pending'

      - alert: ClaimRedemptionFailures
        expr: rate(claims_redeemed_total{success="false"}[5m]) > 0
        for: 2m
        labels:
          severity: warning
        annotations:
          summary: 'Claim redemptions are failing'
          description: 'On-chain claim redemptions are failing (check gas, RPC health)'
```

### Explorer UI Updates

```typescript
// packages/connector/explorer-ui/src/components/ClaimTimeline.tsx

/**
 * Display claim lifecycle from send to redemption
 */
export function ClaimTimeline({ messageId }: { messageId: string }) {
  const events = useClaimEvents(messageId);

  return (
    <div className="claim-timeline">
      {events.map(event => (
        <TimelineItem key={event.timestamp}>
          <TimelineBadge type={event.type} />
          <div>
            <h4>{formatEventType(event.type)}</h4>
            <p>{formatTimestamp(event.timestamp)}</p>
            {event.type === 'CLAIM_REDEEMED' && (
              <a href={getExplorerUrl(event.blockchain, event.txHash)}>
                View on blockchain explorer →
              </a>
            )}
          </div>
        </TimelineItem>
      ))}
    </div>
  );
}
```

---

## Epic Completion Criteria

- [ ] BTP claim message protocol defined for all three blockchains
- [ ] ClaimSender implemented and integrated with UnifiedSettlementExecutor
- [ ] ClaimReceiver implemented and integrated with BTP server
- [ ] Claim verification working for XRP, EVM, and Aptos
- [ ] Automatic claim redemption service functional
- [ ] Database schema supports claim persistence (sent and received)
- [ ] TODO at `unified-settlement-executor.ts:273` resolved
- [ ] Telemetry events emitted for all claim operations
- [ ] Explorer UI displays claim exchange timeline
- [ ] Prometheus metrics and alerts configured
- [ ] Integration tests verify end-to-end claim exchange for all chains
- [ ] Documentation updated with claim exchange architecture
- [ ] Troubleshooting guide created for operators

---

## Dependencies and Integration Points

**Depends On:**

- **Epic 2: BTP Protocol** - WebSocket connections and protocolData multiplexing
- **Epic 8: EVM Payment Channels** - EVM claim signing and verification
- **Epic 9: XRP Payment Channels** - XRP claim signing and verification
- **Epic 13: Aptos Payment Channels** - Aptos claim signing and verification
- Epic 6: TigerBeetle accounting and settlement thresholds

**Integrates With:**

- `BTPClient` (Epic 2) - Sending claims over WebSocket connections
- `BTPServer` (Epic 2) - Receiving claims from peers
- `UnifiedSettlementExecutor` (Epic 9) - Settlement routing and claim creation
- `XRPClaimSigner` (Epic 9) - XRP claim signature verification
- `PaymentChannelSDK` (Epic 8) - EVM balance proof verification
- `AptosClaimSigner` (Epic 13) - Aptos claim signature verification
- `TelemetryEmitter` - Claim event reporting for Explorer UI
- `SettlementMonitor` - Settlement threshold triggers

---

## Risk Management

### Breaking Change Risks

| Risk                               | Likelihood | Impact | Mitigation                                          |
| ---------------------------------- | ---------- | ------ | --------------------------------------------------- |
| BTP protocol compatibility issues  | Low        | Medium | Thorough testing with existing BTP infrastructure   |
| Claim verification failures        | Medium     | High   | Comprehensive unit tests for each blockchain type   |
| Database migration issues          | Low        | Medium | Backward-compatible schema changes, migration tests |
| Performance impact on BTP messages | Low        | Low    | Claims are infrequent compared to ILP packets       |
| Redemption transaction failures    | Medium     | Medium | Retry logic, gas estimation, profitability checks   |

### Rollback Strategy

**If claim exchange is failing:**

1. **Feature Flag:** `CLAIM_EXCHANGE_ENABLED=false` to disable claim sending
2. **Fallback:** Manual claim redemption via CLI tools (existing blockchain SDKs)
3. **No Impact on Packets:** ILP packet forwarding unaffected (independent protocol)
4. **Settlement Continues:** TigerBeetle accounting continues, claims accumulate for later manual redemption

### Compatibility Guarantees

- **BTP Protocol:** No changes to existing BTP message types (MESSAGE, PREPARE, FULFILL, REJECT)
- **ILP Packets:** ILP packet handling completely unaffected
- **Settlement Logic:** Existing settlement threshold triggers unchanged
- **Blockchain SDKs:** No changes to claim signing/verification interfaces
- **Database:** Additive schema only (new tables, no modifications to existing)

---

## Technical Architecture Notes

### Why Use BTP protocolData Instead of ILP Packet Data?

1. **Semantic Clarity:** Claims are not ILP payments, separate protocol is clearer
2. **Efficiency:** No need to wrap claims in ILP packet overhead (executionCondition, etc.)
3. **Multiplexing:** Can send claims alongside ILP packets in same BTP message
4. **Future-Proof:** Enables protocol versioning and extensions

### Claim Exchange Flow

```
Settlement Threshold Reached
         ↓
UnifiedSettlementExecutor
         ↓
Sign Claim (XRP/EVM/Aptos Signer)
         ↓
ClaimSender.send[XRP|EVM|Aptos]Claim()
         ↓
BTPClient.sendMessage()
         ↓
───────────────────── BTP WebSocket ─────────────────────
         ↓
BTPServer receives protocolData
         ↓
ClaimReceiver.handleClaimMessage()
         ↓
Verify Signature (blockchain-specific)
         ↓
Store Verified Claim (SQLite)
         ↓
ClaimRedemptionService polls
         ↓
Estimate Gas Cost
         ↓
Profitable? → Submit to Blockchain
         ↓
Update Claim as Redeemed
```

### Security Considerations

1. **Signature Verification:** Every received claim signature verified before acceptance
2. **Nonce/Amount Monotonicity:** Enforce monotonic increases to prevent replay attacks
3. **Claim Persistence:** All sent/received claims stored for dispute resolution
4. **Message ID Uniqueness:** Prevent duplicate processing via unique message IDs
5. **BTP Authentication:** Claims only accepted from authenticated BTP peers
6. **Gas Safety:** Profitability check prevents unprofitable redemptions

### Performance Requirements

- Claim sending latency: <100ms per claim
- Claim verification latency: <50ms per claim
- BTP message overhead: <1KB per claim message
- Database writes: <10ms per claim persistence
- Redemption polling: 60-second intervals (configurable)

---

## Success Metrics

- Claim send success rate: >99.9%
- Claim verification accuracy: 100%
- Claim redemption success rate: >99%
- Average claim exchange latency (send → received): <500ms
- Average redemption latency (received → on-chain): <2 minutes
- Zero claim exchange impact on ILP packet throughput
- Zero false positive claim verifications
