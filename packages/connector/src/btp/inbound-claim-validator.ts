/**
 * Inbound Claim Validator
 *
 * Validates that inbound BTP packets carrying ILP PREPARE contain a valid
 * per-packet payment channel claim before they are forwarded to the packet handler.
 *
 * This is a security gate that prevents unpaid writes: without a valid signed claim,
 * the packet is rejected with F06 (Unexpected Payment) and never reaches the
 * local delivery handler or event store.
 *
 * @module inbound-claim-validator
 * @see RFC-0023 - Bilateral Transfer Protocol (BTP)
 */

import type { BTPProtocolData } from './btp-types';
import { BTP_CLAIM_PROTOCOL, validateClaimMessage } from './btp-claim-types';
import type { EVMClaimMessage } from './btp-claim-types';
import type { ILPPreparePacket, ILPRejectPacket } from '@toon-protocol/shared';
import { PacketType, ILPErrorCode } from '@toon-protocol/shared';
import type { BalanceProof } from '@toon-protocol/shared';
import type { PaymentChannelSDK } from '../settlement/payment-channel-sdk';
import type { ChannelManager } from '../settlement/channel-manager';
import type { Logger } from '../utils/logger';

/**
 * Callback type for inbound claim validation.
 * Returns null if the packet should proceed, or an ILPRejectPacket to reject it.
 */
export type InboundClaimValidatorFn = (
  protocolData: BTPProtocolData[],
  ilpPacket: ILPPreparePacket,
  peerId: string
) => Promise<ILPRejectPacket | null>;

/**
 * InboundClaimValidator - Validates per-packet claims on inbound BTP packets
 *
 * Sits in the BTP server's message handling path, before the packet handler.
 * Ensures every ILP PREPARE arriving via BTP carries a valid signed payment
 * channel claim. Rejects packets without valid claims immediately.
 */
export class InboundClaimValidator {
  private readonly logger: Logger;
  private readonly paymentChannelSDK: PaymentChannelSDK;
  private readonly channelManager?: ChannelManager;
  private readonly nodeId: string;

  constructor(
    paymentChannelSDK: PaymentChannelSDK,
    nodeId: string,
    logger: Logger,
    channelManager?: ChannelManager
  ) {
    this.paymentChannelSDK = paymentChannelSDK;
    this.nodeId = nodeId;
    this.logger = logger.child({ component: 'InboundClaimValidator' });
    this.channelManager = channelManager;
  }

  /**
   * Validate that an inbound BTP packet has a valid per-packet claim.
   *
   * @param protocolData - BTP protocol data array from the message
   * @param ilpPacket - Deserialized ILP PREPARE packet
   * @param peerId - Authenticated peer ID
   * @returns null if valid (proceed), or ILPRejectPacket to reject
   */
  async validate(
    protocolData: BTPProtocolData[],
    ilpPacket: ILPPreparePacket,
    peerId: string
  ): Promise<ILPRejectPacket | null> {
    // Zero-amount packets carry no value — skip claim validation
    if (ilpPacket.amount === 0n) {
      this.logger.debug(
        { event: 'inbound_claim_skip_zero', peerId, destination: ilpPacket.destination },
        'Skipping claim validation for zero-amount packet'
      );
      return null;
    }

    // Find the payment-channel-claim in protocol data
    const claimData = protocolData.find((pd) => pd.protocolName === BTP_CLAIM_PROTOCOL.NAME);

    if (!claimData) {
      this.logger.warn(
        {
          event: 'inbound_claim_missing',
          peerId,
          destination: ilpPacket.destination,
        },
        'Rejecting ILP PREPARE: no payment channel claim attached'
      );
      return this.createReject('No payment channel claim attached to packet');
    }

    // Parse and validate claim structure
    let claim: EVMClaimMessage;
    try {
      const parsed = JSON.parse(claimData.data.toString('utf8'));
      validateClaimMessage(parsed);
      claim = parsed as EVMClaimMessage;
    } catch (error) {
      this.logger.warn(
        {
          event: 'inbound_claim_invalid_structure',
          peerId,
          error: error instanceof Error ? error.message : String(error),
        },
        'Rejecting ILP PREPARE: invalid claim structure'
      );
      return this.createReject(
        `Invalid claim structure: ${error instanceof Error ? error.message : String(error)}`
      );
    }

    // Verify EIP-712 signature
    const balanceProof: BalanceProof = {
      channelId: claim.channelId,
      nonce: claim.nonce,
      transferredAmount: BigInt(claim.transferredAmount),
      lockedAmount: BigInt(claim.lockedAmount),
      locksRoot: claim.locksRoot,
    };

    let signatureValid: boolean;
    try {
      // Prefer self-describing claims with explicit domain (Epic 31)
      if (claim.chainId !== undefined && claim.tokenNetworkAddress) {
        signatureValid = await this.paymentChannelSDK.verifyBalanceProofWithDomain(
          balanceProof,
          claim.signature,
          claim.signerAddress,
          claim.chainId,
          claim.tokenNetworkAddress
        );
      } else {
        // Fall back to known-channel verification
        const knownChannel = this.channelManager?.getChannelById(claim.channelId);
        if (!knownChannel) {
          this.logger.warn(
            {
              event: 'inbound_claim_unknown_channel',
              peerId,
              channelId: claim.channelId,
            },
            'Rejecting ILP PREPARE: unknown channel and no self-describing fields'
          );
          return this.createReject(
            'Unknown channel: claim must include chainId and tokenNetworkAddress'
          );
        }
        signatureValid = await this.paymentChannelSDK.verifyBalanceProof(
          balanceProof,
          claim.signature,
          claim.signerAddress
        );
      }
    } catch (error) {
      this.logger.warn(
        {
          event: 'inbound_claim_signature_error',
          peerId,
          channelId: claim.channelId,
          error: error instanceof Error ? error.message : String(error),
        },
        'Rejecting ILP PREPARE: signature verification error'
      );
      return this.createReject('Signature verification failed');
    }

    if (!signatureValid) {
      this.logger.warn(
        {
          event: 'inbound_claim_invalid_signature',
          peerId,
          channelId: claim.channelId,
          signerAddress: claim.signerAddress,
        },
        'Rejecting ILP PREPARE: invalid EIP-712 signature'
      );
      return this.createReject('Invalid EIP-712 signature on claim');
    }

    this.logger.debug(
      {
        event: 'inbound_claim_validated',
        peerId,
        channelId: claim.channelId,
        transferredAmount: claim.transferredAmount,
        nonce: claim.nonce,
      },
      'Inbound claim validated successfully'
    );

    return null; // Claim is valid, proceed to packet handler
  }

  private createReject(message: string): ILPRejectPacket {
    return {
      type: PacketType.REJECT,
      code: ILPErrorCode.F06_UNEXPECTED_PAYMENT,
      triggeredBy: this.nodeId,
      message,
      data: Buffer.alloc(0),
    };
  }
}
