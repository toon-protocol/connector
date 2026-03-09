/**
 * Local Delivery Client
 *
 * HTTP client for forwarding ILP packets to an external business logic server
 * for local delivery handling. Sends simplified PaymentRequest/PaymentResponse
 * (no ILP knowledge required on the BLS side) and handles fulfillment
 * computation, reject code mapping, and data validation internally.
 */

import { Logger } from 'pino';
import {
  ILPPreparePacket,
  ILPFulfillPacket,
  ILPRejectPacket,
  PacketType,
  ILPErrorCode,
} from '@crosstown/shared';
import { LocalDeliveryConfig } from '../config/types';
import {
  PaymentRequest,
  PaymentResponse,
  computeFulfillmentFromData,
  generatePaymentId,
  mapRejectCode,
  validateResponseData,
} from './payment-handler';

// Re-export for backward compatibility
export type { LocalDeliveryRequest, LocalDeliveryResponse } from '../config/types';

/**
 * Default configuration values.
 */
const DEFAULT_TIMEOUT = 30000; // 30 seconds

/**
 * Client for forwarding local delivery to an external business logic server.
 */
export class LocalDeliveryClient {
  private readonly config: Required<LocalDeliveryConfig>;
  private readonly logger: Logger;

  constructor(config: LocalDeliveryConfig, logger: Logger) {
    this.config = {
      enabled: config.enabled ?? false,
      handlerUrl: config.handlerUrl ?? '',
      timeout: config.timeout ?? DEFAULT_TIMEOUT,
      authToken: config.authToken ?? '',
      perHopNotification: config.perHopNotification ?? false,
    };
    this.logger = logger.child({ component: 'LocalDeliveryClient' });

    if (this.config.enabled && !this.config.handlerUrl) {
      throw new Error('LOCAL_DELIVERY_URL is required when local delivery is enabled');
    }
  }

  /**
   * Check if local delivery is enabled.
   */
  isEnabled(): boolean {
    return this.config.enabled;
  }

  /**
   * Check if per-hop BLS notification is enabled for transit packets.
   */
  isPerHopNotificationEnabled(): boolean {
    return this.config.perHopNotification;
  }

  /**
   * Forward a packet to the business logic server for local delivery.
   *
   * Sends a simplified PaymentRequest (no ILP internals exposed) and maps
   * the PaymentResponse back to ILP fulfill/reject packets internally.
   *
   * @param packet - ILP Prepare packet
   * @param _sourcePeer - Peer that sent this packet (unused, kept for interface compat)
   * @param options - Optional delivery options
   * @returns ILP Fulfill or Reject packet
   */
  async deliver(
    packet: ILPPreparePacket,
    _sourcePeer: string,
    options?: { isTransit?: boolean }
  ): Promise<ILPFulfillPacket | ILPRejectPacket> {
    // Check expiry before making the HTTP call
    if (packet.expiresAt < new Date()) {
      this.logger.warn(
        { destination: packet.destination, expiresAt: packet.expiresAt.toISOString() },
        'Payment expired before delivery'
      );
      return {
        type: PacketType.REJECT,
        code: ILPErrorCode.R00_TRANSFER_TIMED_OUT,
        triggeredBy: '',
        message: 'Payment has expired',
        data: Buffer.alloc(0),
      };
    }

    const url = `${this.config.handlerUrl}/handle-packet`;
    const paymentId = generatePaymentId();

    const request: PaymentRequest = {
      paymentId,
      destination: packet.destination,
      amount: packet.amount.toString(),
      expiresAt: packet.expiresAt.toISOString(),
      data: packet.data.length > 0 ? packet.data.toString('base64') : undefined,
      isTransit: options?.isTransit,
    };

    this.logger.debug(
      { paymentId, destination: request.destination, amount: request.amount, url },
      'Forwarding packet to business logic server'
    );

    try {
      const controller = new AbortController();
      const timeoutId = setTimeout(() => controller.abort(), this.config.timeout);

      const response = await fetch(url, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
        },
        body: JSON.stringify(request),
        signal: controller.signal,
      });

      clearTimeout(timeoutId);

      if (!response.ok) {
        // Try to get error details from response body
        let errorDetails = '';
        try {
          const errorBody = await response.json();
          errorDetails = JSON.stringify(errorBody);
        } catch {
          errorDetails = await response.text().catch(() => '');
        }

        this.logger.error(
          {
            status: response.status,
            paymentId,
            destination: request.destination,
            errorBody: errorDetails,
          },
          'Business logic server returned error status'
        );

        return {
          type: PacketType.REJECT,
          code: ILPErrorCode.T00_INTERNAL_ERROR,
          triggeredBy: '',
          message: `Business logic server returned status ${response.status}: ${errorDetails}`,
          data: Buffer.alloc(0),
        };
      }

      const result = (await response.json()) as PaymentResponse;

      // Validate response shape
      if (typeof result.accept !== 'boolean') {
        this.logger.error(
          { paymentId, destination: request.destination },
          'Business logic server returned malformed response (missing accept field)'
        );

        return {
          type: PacketType.REJECT,
          code: ILPErrorCode.T00_INTERNAL_ERROR,
          triggeredBy: '',
          message: 'Malformed response from business logic server',
          data: Buffer.alloc(0),
        };
      }

      if (result.accept) {
        // Compute fulfillment = SHA256(data)
        const fulfillment = computeFulfillmentFromData(packet.data);
        const validatedData = validateResponseData(result.data, this.logger);

        this.logger.info(
          { paymentId, destination: request.destination, amount: request.amount },
          'Packet fulfilled by business logic server'
        );

        return {
          type: PacketType.FULFILL,
          fulfillment,
          data: validatedData ? Buffer.from(validatedData, 'base64') : Buffer.alloc(0),
        };
      } else {
        // Map business reject code to ILP error code
        const ilpCode = result.rejectReason ? mapRejectCode(result.rejectReason.code) : 'F99';
        const message = result.rejectReason?.message ?? 'Payment rejected';
        const validatedData = validateResponseData(result.data, this.logger);

        this.logger.info(
          { paymentId, destination: request.destination, code: ilpCode, message },
          'Packet rejected by business logic server'
        );

        return {
          type: PacketType.REJECT,
          code: ilpCode as ILPErrorCode,
          triggeredBy: '',
          message,
          data: validatedData ? Buffer.from(validatedData, 'base64') : Buffer.alloc(0),
        };
      }
    } catch (error) {
      this.logger.error(
        { paymentId, destination: request.destination, error },
        'Failed to forward packet to business logic server'
      );

      if (error instanceof Error && error.name === 'AbortError') {
        return {
          type: PacketType.REJECT,
          code: ILPErrorCode.R00_TRANSFER_TIMED_OUT,
          triggeredBy: '',
          message: 'Business logic server request timed out',
          data: Buffer.alloc(0),
        };
      }

      return {
        type: PacketType.REJECT,
        code: ILPErrorCode.T00_INTERNAL_ERROR,
        triggeredBy: '',
        message: error instanceof Error ? error.message : 'Unknown error',
        data: Buffer.alloc(0),
      };
    }
  }

  /**
   * Check if the business logic server is healthy.
   */
  async healthCheck(): Promise<boolean> {
    if (!this.config.enabled) {
      return true;
    }

    const url = `${this.config.handlerUrl}/health`;

    try {
      const controller = new AbortController();
      const timeoutId = setTimeout(() => controller.abort(), 5000);

      const response = await fetch(url, {
        method: 'GET',
        signal: controller.signal,
      });

      clearTimeout(timeoutId);

      return response.ok;
    } catch {
      return false;
    }
  }
}
