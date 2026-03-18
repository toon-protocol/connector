/**
 * Payment Handler — Simple payment handler for in-process delivery
 *
 * Provides a simplified DX for handling inbound payments without
 * requiring knowledge of ILP packet types, fulfillment computation,
 * or error code mappings.
 *
 * ## Fulfillment Scheme (Simplified, Non-STREAM)
 *
 * This connector uses a simplified data-based fulfillment scheme rather than
 * STREAM's HMAC-based approach:
 *
 *   fulfillment = SHA256(packet.data)
 *   condition   = SHA256(fulfillment)
 *
 * This is safe in trusted bilateral peering networks with on-chain settlement,
 * where both sides cooperate and the fulfillment serves as a consistency check
 * rather than a security boundary.
 *
 * @packageDocumentation
 */

import * as crypto from 'crypto';
import { Logger } from '../utils/logger';
import { LocalDeliveryHandler, LocalDeliveryRequest, LocalDeliveryResponse } from '../config/types';

/** Maximum ILP data field size per RFC-0027 (32KB) */
const ILP_MAX_DATA_BYTES = 32768;

// ────────────────────────────────────────────────────────────────────────────
// Types
// ────────────────────────────────────────────────────────────────────────────

/**
 * Simplified inbound payment request.
 * Drops executionCondition and sourcePeer — users don't need them.
 */
export interface PaymentRequest {
  /** Unique payment identifier (base64url) */
  paymentId: string;
  /** Full ILP destination address */
  destination: string;
  /** Amount in smallest unit (as string for precision) */
  amount: string;
  /** ISO 8601 expiration timestamp */
  expiresAt: string;
  /** Base64-encoded application data (optional) */
  data?: string;
  /**
   * Whether this is a transit notification at an intermediate hop.
   * When true, the BLS response is ignored (fire-and-forget notification).
   * When false or omitted, this is a final-hop delivery where the BLS
   * response determines accept/reject.
   */
  isTransit?: boolean;
}

/**
 * Simplified payment response.
 * Users return accept/reject decisions without ILP knowledge.
 */
export interface PaymentResponse {
  /** Whether to accept (fulfill) the payment */
  accept: boolean;
  /** Optional response data (base64) for fulfill or reject packet */
  data?: string;
  /** Rejection reason (only used when accept is false) */
  rejectReason?: {
    /** Business error code (e.g., 'insufficient_funds', 'invalid_amount') */
    code: string;
    /** Human-readable error message */
    message: string;
  };
}

/**
 * Simple payment handler function type.
 * Users implement this to handle inbound payments.
 */
export type PaymentHandler = (request: PaymentRequest) => Promise<PaymentResponse>;

// ────────────────────────────────────────────────────────────────────────────
// Constants
// ────────────────────────────────────────────────────────────────────────────

/**
 * Map business logic reject codes to ILP error codes.
 * Copied from @toon-protocol/connector to avoid cross-package dependency.
 */
export const REJECT_CODE_MAP: Record<string, string> = {
  insufficient_funds: 'T04',
  expired: 'R00',
  invalid_request: 'F00',
  invalid_amount: 'F03',
  unexpected_payment: 'F06',
  application_error: 'F99',
  internal_error: 'T00',
  timeout: 'T00',
};

// ────────────────────────────────────────────────────────────────────────────
// Utilities
// ────────────────────────────────────────────────────────────────────────────

/**
 * Compute fulfillment from raw packet data.
 * fulfillment = SHA256(data)
 *
 * @param data - Raw packet data bytes
 * @returns 32-byte SHA-256 hash (fulfillment preimage)
 */
export function computeFulfillmentFromData(data: Buffer): Buffer {
  return crypto.createHash('sha256').update(data).digest();
}

/**
 * Validate that a fulfillment matches its expected condition.
 * Checks: SHA256(fulfillment) === condition
 *
 * @param fulfillment - 32-byte fulfillment preimage
 * @param condition - 32-byte execution condition
 * @returns true if SHA256(fulfillment) equals condition
 */
export function validateFulfillment(fulfillment: Buffer, condition: Buffer): boolean {
  const expected = crypto.createHash('sha256').update(fulfillment).digest();
  return expected.equals(condition);
}

/**
 * Generate a random payment ID.
 *
 * @returns URL-safe base64 string (16 random bytes)
 */
export function generatePaymentId(): string {
  return crypto.randomBytes(16).toString('base64url');
}

/**
 * Map a business reject code to an ILP error code.
 *
 * @param code - Business error code (e.g., 'insufficient_funds')
 * @returns ILP error code (e.g., 'T04'), defaults to 'F99'
 */
export function mapRejectCode(code: string): string {
  return REJECT_CODE_MAP[code] ?? 'F99';
}

/**
 * Validate response data for inclusion in ILP packets.
 * Returns the data unchanged if valid base64 and within 32KB limit.
 * Returns undefined (with warning log) if invalid.
 *
 * @param data - Base64-encoded response data
 * @param logger - Logger for warnings
 * @returns Validated data or undefined
 */
export function validateResponseData(data: string | undefined, logger: Logger): string | undefined {
  if (!data) return data;

  try {
    const decoded = Buffer.from(data, 'base64');
    // Verify round-trip (catches non-base64 strings that Buffer.from silently decodes)
    if (decoded.toString('base64') !== data) {
      logger.warn('Response data is not valid base64, omitting from ILP response');
      return undefined;
    }
    if (decoded.length > ILP_MAX_DATA_BYTES) {
      logger.warn(
        { size: decoded.length, limit: ILP_MAX_DATA_BYTES },
        'Response data exceeds 32KB ILP limit, omitting from ILP response'
      );
      return undefined;
    }
    return data;
  } catch {
    logger.warn('Response data failed base64 decode, omitting from ILP response');
    return undefined;
  }
}

// ────────────────────────────────────────────────────────────────────────────
// Adapter Factory
// ────────────────────────────────────────────────────────────────────────────

/**
 * Create a LocalDeliveryHandler adapter that wraps a simple PaymentHandler.
 *
 * The adapter handles:
 * 1. Packet expiry checks (→ R00 reject)
 * 2. LocalDeliveryRequest → PaymentRequest transformation
 * 3. User handler invocation (catches throws → T00 reject)
 * 4. PaymentResponse → LocalDeliveryResponse transformation
 *    (computing fulfillment on accept, mapping reject codes)
 *
 * @param handler - Simple payment handler function
 * @param logger - Logger instance
 * @returns LocalDeliveryHandler that can be passed to PacketHandler
 */
export function createPaymentHandlerAdapter(
  handler: PaymentHandler,
  logger: Logger
): LocalDeliveryHandler {
  return async (packet: LocalDeliveryRequest): Promise<LocalDeliveryResponse> => {
    // 1. Check if payment has expired
    const expiresAtDate = new Date(packet.expiresAt);
    if (expiresAtDate < new Date()) {
      logger.warn({ expiresAt: packet.expiresAt }, 'Payment expired');
      return {
        reject: {
          code: 'R00',
          message: 'Payment has expired',
        },
      };
    }

    // 2. Transform LocalDeliveryRequest → PaymentRequest
    const paymentId = generatePaymentId();
    const paymentRequest: PaymentRequest = {
      paymentId,
      destination: packet.destination,
      amount: packet.amount,
      expiresAt: packet.expiresAt,
      data: packet.data || undefined,
      isTransit: packet.isTransit,
    };

    // 3. Call user handler
    let response: PaymentResponse;
    try {
      response = await handler(paymentRequest);
    } catch (error) {
      const msg = error instanceof Error ? error.message : String(error);
      logger.error({ paymentId, error: msg }, 'Payment handler threw an error');
      return {
        reject: {
          code: 'T00',
          message: 'Internal error processing payment',
        },
      };
    }

    // 4. Transform PaymentResponse → LocalDeliveryResponse
    if (response.accept) {
      // Compute fulfillment as SHA256(data)
      const fulfillment = computeFulfillmentFromData(Buffer.from(packet.data, 'base64'));

      logger.info({ paymentId, amount: packet.amount }, 'Payment fulfilled');

      return {
        fulfill: {
          fulfillment: fulfillment.toString('base64'),
          data: validateResponseData(response.data, logger),
        },
      };
    } else {
      // Map reject code
      const ilpCode = response.rejectReason ? mapRejectCode(response.rejectReason.code) : 'F99';
      const message = response.rejectReason?.message ?? 'Payment rejected';

      logger.info({ paymentId, code: ilpCode, message }, 'Payment rejected');

      return {
        reject: {
          code: ilpCode,
          message,
          data: validateResponseData(response.data, logger),
        },
      };
    }
  };
}
