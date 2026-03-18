/**
 * ILP Packet Factory for Test Packet Generation
 *
 * Creates valid ILP packets (Prepare, Fulfill, Reject) for testing purposes.
 * Implements RFC-0027 packet format with proper SHA-256 execution conditions.
 */

import { createHash, randomBytes } from 'crypto';
import {
  ILPPreparePacket,
  ILPFulfillPacket,
  ILPRejectPacket,
  PacketType,
  ILPErrorCode,
  isValidILPAddress,
} from '@toon-protocol/shared';

/**
 * Result from creating a test Prepare packet
 * Includes both the packet and the preimage for potential Fulfill generation
 */
export interface PreparePacketResult {
  packet: ILPPreparePacket;
  preimage: Buffer;
}

/**
 * Create a test ILP Prepare packet with valid execution condition
 *
 * Generates a Prepare packet with:
 * - Random 32-byte preimage
 * - Execution condition = SHA-256(preimage)
 * - Future expiration timestamp
 * - Validated destination address
 *
 * @param destination - ILP destination address (e.g., g.connectora.dest)
 * @param amount - Transfer amount in smallest unit (uint64)
 * @param expirySeconds - Packet expiry time in seconds from now (default: 30)
 * @param data - Optional application data payload
 * @returns PreparePacketResult containing packet and preimage
 * @throws Error if destination address is invalid
 */
export function createTestPreparePacket(
  destination: string,
  amount: bigint,
  expirySeconds = 30,
  data?: Buffer
): PreparePacketResult {
  // Validate destination address
  if (!isValidILPAddress(destination)) {
    throw new Error(`Invalid ILP address: ${destination}`);
  }

  // Generate random 32-byte preimage
  const preimage = randomBytes(32);

  // Create execution condition: SHA-256(preimage)
  const executionCondition = createHash('sha256').update(preimage).digest();

  // Calculate expiry timestamp: current time + expirySeconds
  const expiresAt = new Date(Date.now() + expirySeconds * 1000);

  // Create ILP Prepare packet
  const packet: ILPPreparePacket = {
    type: PacketType.PREPARE,
    amount,
    destination,
    executionCondition,
    expiresAt,
    data: data ?? Buffer.alloc(0),
  };

  return { packet, preimage };
}

/**
 * Create a test ILP Fulfill packet
 *
 * @param preimage - 32-byte preimage (must hash to executionCondition from Prepare)
 * @param data - Optional return data payload
 * @returns ILPFulfillPacket
 * @throws Error if preimage is not exactly 32 bytes
 */
export function createTestFulfillPacket(preimage: Buffer, data?: Buffer): ILPFulfillPacket {
  // Validate preimage length
  if (preimage.length !== 32) {
    throw new Error(`Preimage must be 32 bytes, got ${preimage.length} bytes`);
  }

  return {
    type: PacketType.FULFILL,
    fulfillment: preimage,
    data: data ?? Buffer.alloc(0),
  };
}

/**
 * Create a test ILP Reject packet
 *
 * @param code - ILP error code (e.g., F02_UNREACHABLE, T01_PEER_UNREACHABLE)
 * @param message - Human-readable error description
 * @param triggeredBy - ILP address of connector that generated error
 * @param data - Optional error context data
 * @returns ILPRejectPacket
 * @throws Error if triggeredBy address is invalid
 */
export function createTestRejectPacket(
  code: ILPErrorCode,
  message: string,
  triggeredBy: string,
  data?: Buffer
): ILPRejectPacket {
  // Validate triggeredBy address
  if (!isValidILPAddress(triggeredBy)) {
    throw new Error(`Invalid ILP address for triggeredBy: ${triggeredBy}`);
  }

  return {
    type: PacketType.REJECT,
    code,
    triggeredBy,
    message,
    data: data ?? Buffer.alloc(0),
  };
}
