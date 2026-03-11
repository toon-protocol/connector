/**
 * Unit tests for ClaimReceiver
 *
 * Tests claim reception, validation, EVM signature verification,
 * monotonicity checks, and database persistence.
 *
 * Epic 30 Story 30.4: Removed XRP/Aptos claim handling tests (EVM-only settlement).
 */

import { ClaimReceiver, ERRORS } from './claim-receiver';
import type { Database, Statement } from 'better-sqlite3';
import type { Logger } from 'pino';
import type { BTPServer } from '../btp/btp-server';
import type { BTPProtocolData, BTPMessage, BTPData } from '../btp/btp-types';
import type { PaymentChannelSDK } from './payment-channel-sdk';
import type { ChannelManager } from './channel-manager';
import type { EVMClaimMessage } from '../btp/btp-claim-types';

describe('ClaimReceiver', () => {
  let claimReceiver: ClaimReceiver;
  let mockDb: jest.Mocked<Database>;
  let mockLogger: jest.Mocked<Logger>;
  let mockBTPServer: jest.Mocked<BTPServer>;
  let mockPaymentChannelSDK: jest.Mocked<PaymentChannelSDK>;
  let mockStatement: jest.Mocked<Statement>;
  let btpMessageHandler: ((peerId: string, message: BTPMessage) => void) | null;

  beforeEach(() => {
    jest.clearAllMocks();
    btpMessageHandler = null;

    // Mock Database
    mockStatement = {
      run: jest.fn(),
      get: jest.fn(),
    } as unknown as jest.Mocked<Statement>;

    mockDb = {
      prepare: jest.fn().mockReturnValue(mockStatement),
      exec: jest.fn(),
    } as unknown as jest.Mocked<Database>;

    // Mock Logger
    mockLogger = {
      info: jest.fn(),
      error: jest.fn(),
      warn: jest.fn(),
      debug: jest.fn(),
      child: jest.fn().mockReturnThis(),
    } as unknown as jest.Mocked<Logger>;

    // Mock BTPServer
    mockBTPServer = {
      onMessage: jest.fn((handler) => {
        btpMessageHandler = handler;
      }),
    } as unknown as jest.Mocked<BTPServer>;

    // Mock PaymentChannelSDK
    mockPaymentChannelSDK = {
      verifyBalanceProof: jest.fn(),
    } as unknown as jest.Mocked<PaymentChannelSDK>;

    // Create ClaimReceiver instance (EVM-only)
    claimReceiver = new ClaimReceiver(mockDb, mockPaymentChannelSDK, mockLogger);
  });

  describe('registerWithBTPServer', () => {
    it('should register message handler with BTP server', () => {
      claimReceiver.registerWithBTPServer(mockBTPServer);

      expect(mockBTPServer.onMessage).toHaveBeenCalledTimes(1);
      expect(mockBTPServer.onMessage).toHaveBeenCalledWith(expect.any(Function));
      expect(mockLogger.info).toHaveBeenCalledWith('ClaimReceiver registered with BTP server');
    });
  });

  // XRP claim handling removed in Epic 30 Story 30.4 - EVM-only settlement

  describe('handleClaimMessage - EVM Claims', () => {
    let validEVMClaim: EVMClaimMessage;
    let protocolData: BTPProtocolData;
    let btpMessage: BTPMessage;

    beforeEach(() => {
      validEVMClaim = {
        version: '1.0',
        blockchain: 'evm',
        messageId: 'evm-0xabc123-5-1706889600000',
        timestamp: '2026-02-02T12:00:00.000Z',
        senderId: 'peer-bob',
        channelId: '0x' + 'a'.repeat(64),
        nonce: 5,
        transferredAmount: '1000000000000000000',
        lockedAmount: '0',
        locksRoot: '0x' + '0'.repeat(64),
        signature: '0x' + 'b'.repeat(130),
        signerAddress: '0x' + 'c'.repeat(40),
      };

      protocolData = {
        protocolName: 'payment-channel-claim',
        contentType: 1,
        data: Buffer.from(JSON.stringify(validEVMClaim), 'utf8'),
      };

      btpMessage = {
        type: 6,
        requestId: 1,
        data: {
          protocolData: [protocolData],
          transfer: {
            amount: '0',
            expiresAt: new Date(Date.now() + 30000).toISOString(),
          },
        } as BTPData,
      };
    });

    it('should verify valid EVM claim and store with verified=true', async () => {
      mockPaymentChannelSDK.verifyBalanceProof.mockResolvedValue(true);
      mockStatement.get.mockReturnValue(undefined); // No previous claim

      claimReceiver.registerWithBTPServer(mockBTPServer);
      await btpMessageHandler!('peer-bob', btpMessage);
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Verify balance proof verification
      expect(mockPaymentChannelSDK.verifyBalanceProof).toHaveBeenCalledWith(
        {
          channelId: validEVMClaim.channelId,
          nonce: validEVMClaim.nonce,
          transferredAmount: BigInt(validEVMClaim.transferredAmount),
          lockedAmount: BigInt(validEVMClaim.lockedAmount),
          locksRoot: validEVMClaim.locksRoot,
        },
        validEVMClaim.signature,
        validEVMClaim.signerAddress
      );

      // Verify database insert with verified=true
      expect(mockStatement.run).toHaveBeenCalledWith(
        validEVMClaim.messageId,
        'peer-bob',
        'evm',
        validEVMClaim.channelId,
        JSON.stringify(validEVMClaim),
        1, // verified=true
        expect.any(Number),
        null,
        null
      );
    });

    it('should reject EVM claim with invalid EIP-712 signature', async () => {
      mockPaymentChannelSDK.verifyBalanceProof.mockResolvedValue(false);

      claimReceiver.registerWithBTPServer(mockBTPServer);
      await btpMessageHandler!('peer-bob', btpMessage);
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Verify database insert with verified=false
      expect(mockStatement.run).toHaveBeenCalledWith(
        validEVMClaim.messageId,
        'peer-bob',
        'evm',
        validEVMClaim.channelId,
        JSON.stringify(validEVMClaim),
        0, // verified=false
        expect.any(Number),
        null,
        null
      );
    });

    it('should reject EVM claim with non-increasing nonce (monotonicity check)', async () => {
      mockPaymentChannelSDK.verifyBalanceProof.mockResolvedValue(true);

      // Mock previous claim with same nonce
      const previousClaim: EVMClaimMessage = {
        ...validEVMClaim,
        nonce: 5, // Same nonce
      };

      mockStatement.get.mockReturnValue({
        claim_data: JSON.stringify(previousClaim),
      });

      claimReceiver.registerWithBTPServer(mockBTPServer);
      await btpMessageHandler!('peer-bob', btpMessage);
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Verify database insert with verified=false
      expect(mockStatement.run).toHaveBeenCalledWith(
        validEVMClaim.messageId,
        'peer-bob',
        'evm',
        validEVMClaim.channelId,
        JSON.stringify(validEVMClaim),
        0, // verified=false
        expect.any(Number),
        null,
        null
      );
    });
  });

  // Aptos claim handling removed in Epic 30 Story 30.4 - EVM-only settlement

  describe('Error Handling', () => {
    it('should handle invalid JSON parsing gracefully', async () => {
      const protocolData: BTPProtocolData = {
        protocolName: 'payment-channel-claim',
        contentType: 1,
        data: Buffer.from('invalid json', 'utf8'),
      };

      const btpMessage: BTPMessage = {
        type: 6,
        requestId: 1,
        data: {
          protocolData: [protocolData],
          transfer: {
            amount: '0',
            expiresAt: new Date(Date.now() + 30000).toISOString(),
          },
        } as BTPData,
      };

      claimReceiver.registerWithBTPServer(mockBTPServer);
      await btpMessageHandler!('peer-bob', btpMessage);
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Verify error logged
      expect(mockLogger.error).toHaveBeenCalledWith(
        { error: expect.any(Error) },
        'Failed to parse claim message'
      );

      // Verify no database insert
      expect(mockStatement.run).not.toHaveBeenCalled();
    });

    it('should handle database persistence failure gracefully', async () => {
      const validEVMClaim: EVMClaimMessage = {
        version: '1.0',
        blockchain: 'evm',
        messageId: 'evm-test-123',
        timestamp: '2026-02-02T12:00:00.000Z',
        senderId: 'peer-bob',
        channelId: '0x' + 'a'.repeat(64),
        nonce: 1,
        transferredAmount: '1000000',
        lockedAmount: '0',
        locksRoot: '0x' + '0'.repeat(64),
        signature: '0x' + 'b'.repeat(130),
        signerAddress: '0x' + 'c'.repeat(40),
      };

      const protocolData: BTPProtocolData = {
        protocolName: 'payment-channel-claim',
        contentType: 1,
        data: Buffer.from(JSON.stringify(validEVMClaim), 'utf8'),
      };

      const btpMessage: BTPMessage = {
        type: 6,
        requestId: 1,
        data: {
          protocolData: [protocolData],
          transfer: {
            amount: '0',
            expiresAt: new Date(Date.now() + 30000).toISOString(),
          },
        } as BTPData,
      };

      mockPaymentChannelSDK.verifyBalanceProof.mockResolvedValue(true);
      mockStatement.get.mockReturnValue(undefined);
      mockStatement.run.mockImplementation(() => {
        throw new Error('Database error');
      });

      claimReceiver.registerWithBTPServer(mockBTPServer);
      await btpMessageHandler!('peer-bob', btpMessage);
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Verify error logged
      expect(mockLogger.error).toHaveBeenCalledWith(
        { error: expect.any(Error) },
        'Failed to persist claim to database'
      );
    });

    it('should handle duplicate message IDs gracefully (idempotency)', async () => {
      const validEVMClaim: EVMClaimMessage = {
        version: '1.0',
        blockchain: 'evm',
        messageId: 'evm-test-123',
        timestamp: '2026-02-02T12:00:00.000Z',
        senderId: 'peer-bob',
        channelId: '0x' + 'a'.repeat(64),
        nonce: 1,
        transferredAmount: '1000000',
        lockedAmount: '0',
        locksRoot: '0x' + '0'.repeat(64),
        signature: '0x' + 'b'.repeat(130),
        signerAddress: '0x' + 'c'.repeat(40),
      };

      const protocolData: BTPProtocolData = {
        protocolName: 'payment-channel-claim',
        contentType: 1,
        data: Buffer.from(JSON.stringify(validEVMClaim), 'utf8'),
      };

      const btpMessage: BTPMessage = {
        type: 6,
        requestId: 1,
        data: {
          protocolData: [protocolData],
          transfer: {
            amount: '0',
            expiresAt: new Date(Date.now() + 30000).toISOString(),
          },
        } as BTPData,
      };

      mockPaymentChannelSDK.verifyBalanceProof.mockResolvedValue(true);
      mockStatement.get.mockReturnValue(undefined);
      mockStatement.run.mockImplementation(() => {
        const error = new Error('UNIQUE constraint failed: received_claims.message_id');
        throw error;
      });

      claimReceiver.registerWithBTPServer(mockBTPServer);
      await btpMessageHandler!('peer-bob', btpMessage);
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Verify warning logged for duplicate
      expect(mockLogger.warn).toHaveBeenCalledWith(
        { messageId: validEVMClaim.messageId },
        'Duplicate claim message ignored (idempotency)'
      );
    });
  });

  describe('dynamic on-chain verification (Epic 31.2)', () => {
    let dynamicReceiver: ClaimReceiver;
    let mockChannelManager: jest.Mocked<ChannelManager>;
    let dynamicBtpHandler: ((peerId: string, message: BTPMessage) => void) | null;
    let dynamicBTPServer: jest.Mocked<BTPServer>;

    const mockChannelId = '0x' + 'a'.repeat(64);
    const mockSignerAddress = '0x' + 'c'.repeat(40);
    const mockParticipant1 = '0x' + 'c'.repeat(40); // matches signerAddress
    const mockParticipant2 = '0x' + 'd'.repeat(40);
    const mockTokenNetworkAddress = '0x' + 'e'.repeat(40);
    const mockTokenAddress = '0x' + 'f'.repeat(40);

    function makeClaimWithSelfDescribing(
      overrides: Partial<EVMClaimMessage> = {}
    ): EVMClaimMessage {
      return {
        version: '1.0',
        blockchain: 'evm',
        messageId: 'evm-dynamic-test-1',
        timestamp: '2026-03-07T12:00:00.000Z',
        senderId: 'peer-new',
        channelId: mockChannelId,
        nonce: 1,
        transferredAmount: '1000000000000000000',
        lockedAmount: '0',
        locksRoot: '0x' + '0'.repeat(64),
        signature: '0x' + 'b'.repeat(130),
        signerAddress: mockSignerAddress,
        chainId: 31337,
        tokenNetworkAddress: mockTokenNetworkAddress,
        tokenAddress: mockTokenAddress,
        ...overrides,
      };
    }

    function makeBTPMessage(claim: EVMClaimMessage): BTPMessage {
      return {
        type: 6,
        requestId: 1,
        data: {
          protocolData: [
            {
              protocolName: 'payment-channel-claim',
              contentType: 1,
              data: Buffer.from(JSON.stringify(claim), 'utf8'),
            },
          ],
          transfer: {
            amount: '0',
            expiresAt: new Date(Date.now() + 30000).toISOString(),
          },
        } as BTPData,
      };
    }

    beforeEach(() => {
      dynamicBtpHandler = null;

      mockChannelManager = {
        getChannelById: jest.fn().mockReturnValue(null), // unknown channel by default
        registerExternalChannel: jest.fn().mockReturnValue({
          channelId: mockChannelId,
          peerId: 'peer-new',
          tokenId: mockTokenAddress,
          tokenAddress: mockTokenAddress,
          chain: 'evm:31337',
          createdAt: new Date(),
          lastActivityAt: new Date(),
          status: 'open',
        }),
      } as unknown as jest.Mocked<ChannelManager>;

      // Add new SDK methods to mock
      mockPaymentChannelSDK.getChannelStateByNetwork = jest.fn().mockResolvedValue({
        exists: true,
        state: 1,
        participant1: mockParticipant1,
        participant2: mockParticipant2,
        settlementTimeout: 3600,
      });
      mockPaymentChannelSDK.verifyBalanceProofWithDomain = jest.fn().mockResolvedValue(true);

      dynamicBTPServer = {
        onMessage: jest.fn((handler) => {
          dynamicBtpHandler = handler;
        }),
      } as unknown as jest.Mocked<BTPServer>;

      dynamicReceiver = new ClaimReceiver(
        mockDb,
        mockPaymentChannelSDK,
        mockLogger,
        mockChannelManager
      );

      dynamicReceiver.registerWithBTPServer(dynamicBTPServer);
    });

    it('should accept unknown channel with valid on-chain state and register it', async () => {
      const claim = makeClaimWithSelfDescribing();
      mockStatement.get.mockReturnValue(undefined); // No previous claim

      await dynamicBtpHandler!('peer-new', makeBTPMessage(claim));
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Verify on-chain query
      expect(mockPaymentChannelSDK.getChannelStateByNetwork).toHaveBeenCalledWith(
        mockChannelId,
        mockTokenNetworkAddress
      );

      // Verify signature with explicit domain
      expect(mockPaymentChannelSDK.verifyBalanceProofWithDomain).toHaveBeenCalledWith(
        expect.objectContaining({ channelId: mockChannelId }),
        claim.signature,
        claim.signerAddress,
        31337,
        mockTokenNetworkAddress
      );

      // Verify channel registered
      expect(mockChannelManager.registerExternalChannel).toHaveBeenCalledWith({
        channelId: mockChannelId,
        peerId: 'peer-new',
        tokenAddress: mockTokenAddress,
        tokenNetworkAddress: mockTokenNetworkAddress,
        chainId: 31337,
        status: 'open',
      });

      // Verify claim stored as verified
      expect(mockStatement.run).toHaveBeenCalledWith(
        claim.messageId,
        'peer-new',
        'evm',
        mockChannelId,
        JSON.stringify(claim),
        1, // verified=true
        expect.any(Number),
        null,
        null
      );
    });

    it('should reject unknown channel with non-existent channel (state 0)', async () => {
      mockPaymentChannelSDK.getChannelStateByNetwork.mockResolvedValueOnce({
        exists: false,
        state: 0,
        participant1: '0x' + '0'.repeat(40),
        participant2: '0x' + '0'.repeat(40),
        settlementTimeout: 0,
      });

      const claim = makeClaimWithSelfDescribing();
      await dynamicBtpHandler!('peer-new', makeBTPMessage(claim));
      await new Promise((resolve) => setTimeout(resolve, 50));

      expect(mockLogger.warn).toHaveBeenCalledWith(
        expect.objectContaining({
          channelId: mockChannelId,
        }),
        ERRORS.CHANNEL_NOT_FOUND
      );
    });

    it('should reject unknown channel with closed channel (state 2)', async () => {
      mockPaymentChannelSDK.getChannelStateByNetwork.mockResolvedValueOnce({
        exists: true,
        state: 2, // Closed
        participant1: mockParticipant1,
        participant2: mockParticipant2,
        settlementTimeout: 3600,
      });

      const claim = makeClaimWithSelfDescribing();
      await dynamicBtpHandler!('peer-new', makeBTPMessage(claim));
      await new Promise((resolve) => setTimeout(resolve, 50));

      expect(mockLogger.warn).toHaveBeenCalledWith(
        expect.objectContaining({
          channelId: mockChannelId,
        }),
        ERRORS.CHANNEL_NOT_OPENED
      );
    });

    it('should reject unknown channel where signerAddress is not participant', async () => {
      mockPaymentChannelSDK.getChannelStateByNetwork.mockResolvedValueOnce({
        exists: true,
        state: 1,
        participant1: '0x' + '1'.repeat(40),
        participant2: '0x' + '2'.repeat(40),
        settlementTimeout: 3600,
      });

      const claim = makeClaimWithSelfDescribing();
      await dynamicBtpHandler!('peer-new', makeBTPMessage(claim));
      await new Promise((resolve) => setTimeout(resolve, 50));

      expect(mockLogger.warn).toHaveBeenCalledWith(
        expect.objectContaining({
          channelId: mockChannelId,
        }),
        ERRORS.SIGNER_NOT_PARTICIPANT
      );
    });

    it('should skip RPC for second claim on same channel (caching)', async () => {
      // First claim: unknown channel → RPC
      mockStatement.get.mockReturnValue(undefined);

      const claim1 = makeClaimWithSelfDescribing({ nonce: 1 });
      await dynamicBtpHandler!('peer-new', makeBTPMessage(claim1));
      await new Promise((resolve) => setTimeout(resolve, 50));

      expect(mockPaymentChannelSDK.getChannelStateByNetwork).toHaveBeenCalledTimes(1);

      // Second claim: channel now known → no RPC
      mockChannelManager.getChannelById.mockReturnValue({
        channelId: mockChannelId,
        peerId: 'peer-new',
        tokenId: mockTokenAddress,
        tokenAddress: mockTokenAddress,
        chain: 'evm:31337',
        createdAt: new Date(),
        lastActivityAt: new Date(),
        status: 'open',
      });
      mockPaymentChannelSDK.verifyBalanceProof.mockResolvedValue(true);

      const claim2 = makeClaimWithSelfDescribing({
        nonce: 2,
        messageId: 'evm-dynamic-test-2',
      });
      // Return nonce-1 claim for monotonicity check
      mockStatement.get.mockReturnValue({
        claim_data: JSON.stringify(claim1),
      });
      await dynamicBtpHandler!('peer-new', makeBTPMessage(claim2));
      await new Promise((resolve) => setTimeout(resolve, 50));

      // getChannelStateByNetwork should NOT have been called again
      expect(mockPaymentChannelSDK.getChannelStateByNetwork).toHaveBeenCalledTimes(1);
      // verifyBalanceProof (not WithDomain) used for known channel
      expect(mockPaymentChannelSDK.verifyBalanceProof).toHaveBeenCalled();
    });

    it('should reject unknown channel missing self-describing fields', async () => {
      // Missing chainId
      const claim1 = makeClaimWithSelfDescribing({ chainId: undefined });
      await dynamicBtpHandler!('peer-new', makeBTPMessage(claim1));
      await new Promise((resolve) => setTimeout(resolve, 50));

      expect(mockLogger.warn).toHaveBeenCalledWith(
        expect.objectContaining({
          channelId: mockChannelId,
        }),
        ERRORS.MISSING_SELF_DESCRIBING_FIELDS
      );

      // Missing tokenNetworkAddress
      jest.clearAllMocks();
      mockChannelManager.getChannelById.mockReturnValue(null);
      const claim2 = makeClaimWithSelfDescribing({ tokenNetworkAddress: undefined });
      await dynamicBtpHandler!('peer-new', makeBTPMessage(claim2));
      await new Promise((resolve) => setTimeout(resolve, 50));

      expect(mockLogger.warn).toHaveBeenCalledWith(
        expect.objectContaining({
          channelId: mockChannelId,
        }),
        ERRORS.MISSING_SELF_DESCRIBING_FIELDS
      );

      // Missing tokenAddress
      jest.clearAllMocks();
      mockChannelManager.getChannelById.mockReturnValue(null);
      const claim3 = makeClaimWithSelfDescribing({ tokenAddress: undefined });
      await dynamicBtpHandler!('peer-new', makeBTPMessage(claim3));
      await new Promise((resolve) => setTimeout(resolve, 50));

      expect(mockLogger.warn).toHaveBeenCalledWith(
        expect.objectContaining({
          channelId: mockChannelId,
        }),
        ERRORS.MISSING_SELF_DESCRIBING_FIELDS
      );
    });

    it('should reject on RPC failure during verification', async () => {
      mockPaymentChannelSDK.getChannelStateByNetwork.mockRejectedValueOnce(
        new Error('network timeout')
      );

      const claim = makeClaimWithSelfDescribing();
      await dynamicBtpHandler!('peer-new', makeBTPMessage(claim));
      await new Promise((resolve) => setTimeout(resolve, 50));

      expect(mockLogger.warn).toHaveBeenCalledWith(
        expect.objectContaining({
          channelId: mockChannelId,
        }),
        ERRORS.ON_CHAIN_VERIFICATION_FAILED
      );
    });

    it('should reject when EIP-712 signature verification fails for unknown channel', async () => {
      mockPaymentChannelSDK.verifyBalanceProofWithDomain.mockResolvedValueOnce(false);

      const claim = makeClaimWithSelfDescribing();
      await dynamicBtpHandler!('peer-new', makeBTPMessage(claim));
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Claim should be stored as unverified
      expect(mockStatement.run).toHaveBeenCalledWith(
        claim.messageId,
        'peer-new',
        'evm',
        mockChannelId,
        expect.any(String),
        0, // verified=false
        expect.any(Number),
        null,
        null
      );

      // Channel should NOT be registered if signature fails
      expect(mockChannelManager.registerExternalChannel).not.toHaveBeenCalled();
    });

    it('should work with pre-registered channel without self-describing fields (backward compat)', async () => {
      // Channel is already known
      mockChannelManager.getChannelById.mockReturnValue({
        channelId: mockChannelId,
        peerId: 'peer-new',
        tokenId: 'TEST_TOKEN',
        tokenAddress: mockTokenAddress,
        chain: 'evm:31337',
        createdAt: new Date(),
        lastActivityAt: new Date(),
        status: 'open',
      });
      mockPaymentChannelSDK.verifyBalanceProof.mockResolvedValue(true);
      mockStatement.get.mockReturnValue(undefined);

      // Claim WITHOUT self-describing fields
      const claim = makeClaimWithSelfDescribing({
        chainId: undefined,
        tokenNetworkAddress: undefined,
        tokenAddress: undefined,
      });

      await dynamicBtpHandler!('peer-new', makeBTPMessage(claim));
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Should use existing verifyBalanceProof, not the dynamic path
      expect(mockPaymentChannelSDK.verifyBalanceProof).toHaveBeenCalled();
      expect(mockPaymentChannelSDK.getChannelStateByNetwork).not.toHaveBeenCalled();

      // Should store as verified
      expect(mockStatement.run).toHaveBeenCalledWith(
        claim.messageId,
        'peer-new',
        'evm',
        mockChannelId,
        expect.any(String),
        1, // verified
        expect.any(Number),
        null,
        null
      );
    });
  });

  describe('getLatestVerifiedClaim', () => {
    it('should return latest verified claim for peer and channel', async () => {
      const storedClaim: EVMClaimMessage = {
        version: '1.0',
        blockchain: 'evm',
        messageId: 'evm-test-123',
        timestamp: '2026-02-02T12:00:00.000Z',
        senderId: 'peer-bob',
        channelId: '0x' + 'a'.repeat(64),
        nonce: 1,
        transferredAmount: '1000000',
        lockedAmount: '0',
        locksRoot: '0x' + '0'.repeat(64),
        signature: '0x' + 'b'.repeat(130),
        signerAddress: '0x' + 'c'.repeat(40),
      };

      mockStatement.get.mockReturnValue({
        claim_data: JSON.stringify(storedClaim),
      });

      const result = await claimReceiver.getLatestVerifiedClaim(
        'peer-bob',
        'evm',
        '0x' + 'a'.repeat(64)
      );

      expect(result).toEqual(storedClaim);
      expect(mockDb.prepare).toHaveBeenCalledWith(expect.stringContaining('SELECT claim_data'));
      expect(mockStatement.get).toHaveBeenCalledWith('peer-bob', 'evm', '0x' + 'a'.repeat(64));
    });

    it('should return null if no verified claim found', async () => {
      mockStatement.get.mockReturnValue(undefined);

      const result = await claimReceiver.getLatestVerifiedClaim(
        'peer-bob',
        'evm',
        '0x' + 'a'.repeat(64)
      );

      expect(result).toBeNull();
    });

    it('should return null and log error on database failure', async () => {
      mockStatement.get.mockImplementation(() => {
        throw new Error('Database error');
      });

      const result = await claimReceiver.getLatestVerifiedClaim(
        'peer-bob',
        'evm',
        '0x' + 'a'.repeat(64)
      );

      expect(result).toBeNull();
      expect(mockLogger.error).toHaveBeenCalledWith(
        { error: expect.any(Error) },
        'Failed to query latest verified claim'
      );
    });
  });
});
