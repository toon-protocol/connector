/**
 * Claim Validation Gate Integration Tests
 *
 * Tests the inbound claim validation gate that prevents unpaid writes.
 * Verifies that ILP PREPARE packets arriving via BTP without valid signed
 * payment channel claims are rejected at the transport layer (F06) before
 * reaching the packet handler or local delivery.
 *
 * Uses a raw BTP WebSocket client to simulate attack scenarios.
 *
 * Prerequisites:
 *   make anvil-up   # Start Anvil + deploy contracts + start faucet
 *   EVM_INTEGRATION=true npx jest test/integration/claim-validation-gate.test.ts
 *
 * @packageDocumentation
 */

import WebSocket from 'ws';
import { randomBytes, createHash } from 'crypto';
import { ethers } from 'ethers';
import {
  createMultiHopTestNetwork,
  waitForAnvilReady,
  ANVIL_RPC_URL,
  ANVIL_CHAIN_ID,
  REGISTRY_ADDRESS,
  TOKEN_ADDRESS,
  PEER_PRIVATE_KEYS,
  PEER_EVM_ADDRESSES,
  type MultiHopTestNetwork,
} from './multi-hop-helpers';
import { serializeBTPMessage, parseBTPMessage } from '../../src/btp/btp-message-parser';
import {
  serializePacket,
  deserializePacket,
  PacketType,
  ILPErrorCode,
} from '@toon-protocol/shared';
import type { ILPPreparePacket, ILPRejectPacket, ILPFulfillPacket } from '@toon-protocol/shared';
import { BTPMessageType } from '../../src/btp/btp-types';
import type { BTPMessage, BTPData } from '../../src/btp/btp-types';
import { BTP_CLAIM_PROTOCOL } from '../../src/btp/btp-claim-types';
import type { EVMClaimMessage } from '../../src/btp/btp-claim-types';
import { getDomainSeparator, getBalanceProofTypes } from '../../src/settlement/eip712-helper';

// Gate: Only run when EVM_INTEGRATION=true and Anvil is available
const RUN_EVM_TESTS = process.env.EVM_INTEGRATION === 'true';
const describeEvm = RUN_EVM_TESTS ? describe : describe.skip;

// Extend Jest timeout for real EVM operations
jest.setTimeout(180_000);

// ============================================================================
// Raw BTP Client Helpers
// ============================================================================

/**
 * Connect a raw WebSocket BTP client to a connector's BTP server.
 * Performs the no-auth handshake and returns the authenticated connection.
 */
async function connectRawBTPClient(port: number, peerId: string): Promise<WebSocket> {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(`ws://localhost:${port}`);
    let authenticated = false;

    ws.on('open', () => {
      // Send auth message (no-auth mode — empty secret)
      const authData = {
        peerId,
        secret: '',
      };
      const authMessage: BTPMessage = {
        type: BTPMessageType.MESSAGE,
        requestId: 1,
        data: {
          protocolData: [
            {
              protocolName: 'auth',
              contentType: 0,
              data: Buffer.from(JSON.stringify(authData), 'utf8'),
            },
          ],
          ilpPacket: Buffer.alloc(0),
        } as BTPData,
      };
      ws.send(serializeBTPMessage(authMessage));
    });

    ws.on('message', (_data: Buffer) => {
      if (!authenticated) {
        // Auth response received — connection is ready
        authenticated = true;
        resolve(ws);
      }
    });

    ws.on('error', reject);

    setTimeout(() => {
      if (!authenticated) {
        ws.close();
        reject(new Error('BTP auth timeout'));
      }
    }, 10_000);
  });
}

/**
 * Send a raw BTP MESSAGE with an ILP PREPARE and optional claim protocol data.
 * Returns the ILP response packet from the server.
 */
async function sendRawBTPPrepare(
  ws: WebSocket,
  ilpPrepare: ILPPreparePacket,
  claimProtocolData?: { protocolName: string; contentType: number; data: Buffer }
): Promise<ILPFulfillPacket | ILPRejectPacket> {
  const requestId = Math.floor(Math.random() * 0xffffffff);
  const serializedPacket = serializePacket(ilpPrepare);

  const protocolData = claimProtocolData ? [claimProtocolData] : [];

  const btpMessage: BTPMessage = {
    type: BTPMessageType.MESSAGE,
    requestId,
    data: {
      protocolData,
      ilpPacket: serializedPacket,
    } as BTPData,
  };

  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => {
      reject(new Error('BTP response timeout'));
    }, 30_000);

    const handler = (data: Buffer): void => {
      try {
        const response = parseBTPMessage(data as Buffer);
        // Match by requestId
        if (response.requestId === requestId) {
          clearTimeout(timeout);
          ws.removeListener('message', handler);

          const responseData = response.data as BTPData;
          if (responseData.ilpPacket && responseData.ilpPacket.length > 0) {
            const ilpResponse = deserializePacket(responseData.ilpPacket);
            resolve(ilpResponse as ILPFulfillPacket | ILPRejectPacket);
          } else {
            reject(new Error('No ILP packet in BTP response'));
          }
        }
      } catch {
        // Not our response, keep listening
      }
    };

    ws.on('message', handler);
    ws.send(serializeBTPMessage(btpMessage));
  });
}

/**
 * Create a test ILP PREPARE packet.
 *
 * The payment handler adapter computes: fulfillment = SHA256(data)
 * The packet handler validates: SHA256(fulfillment) === condition
 * So condition must be SHA256(SHA256(data)).
 */
function createTestPrepare(
  destination: string,
  amount: bigint
): { packet: ILPPreparePacket; fulfillment: Buffer } {
  const data = Buffer.alloc(0);
  const fulfillment = createHash('sha256').update(data).digest();
  const condition = createHash('sha256').update(fulfillment).digest();

  return {
    packet: {
      type: PacketType.PREPARE,
      destination,
      amount,
      executionCondition: condition,
      expiresAt: new Date(Date.now() + 60_000),
      data,
    },
    fulfillment,
  };
}

/**
 * Build a signed EVM claim message for BTP protocol data.
 */
async function buildSignedClaim(opts: {
  channelId: string;
  nonce: number;
  transferredAmount: bigint;
  signerPrivateKey: string;
  signerAddress: string;
  senderId: string;
  chainId: number;
  tokenNetworkAddress: string;
  tokenAddress: string;
}): Promise<{ protocolName: string; contentType: number; data: Buffer }> {
  const balanceProof = {
    channelId: opts.channelId,
    nonce: opts.nonce,
    transferredAmount: opts.transferredAmount,
    lockedAmount: 0n,
    locksRoot: '0x' + '0'.repeat(64),
  };

  // Sign with EIP-712
  const domain = getDomainSeparator(BigInt(opts.chainId), opts.tokenNetworkAddress);
  const types = getBalanceProofTypes();
  const wallet = new ethers.Wallet(opts.signerPrivateKey);
  const signature = await wallet.signTypedData(domain, types, balanceProof);

  const claim: EVMClaimMessage = {
    version: '1.0',
    blockchain: 'evm',
    messageId: `claim-test-${Date.now()}-${Math.random().toString(36).slice(2)}`,
    timestamp: new Date().toISOString(),
    senderId: opts.senderId,
    channelId: opts.channelId,
    nonce: opts.nonce,
    transferredAmount: opts.transferredAmount.toString(),
    lockedAmount: '0',
    locksRoot: '0x' + '0'.repeat(64),
    signature,
    signerAddress: opts.signerAddress,
    chainId: opts.chainId,
    tokenNetworkAddress: opts.tokenNetworkAddress,
    tokenAddress: opts.tokenAddress,
  };

  return {
    protocolName: BTP_CLAIM_PROTOCOL.NAME,
    contentType: BTP_CLAIM_PROTOCOL.CONTENT_TYPE,
    data: Buffer.from(JSON.stringify(claim), 'utf8'),
  };
}

/**
 * Resolve the TokenNetwork address for the test token from the registry.
 */
async function getTokenNetworkAddress(): Promise<string> {
  const provider = new ethers.JsonRpcProvider(ANVIL_RPC_URL);
  const registry = new ethers.Contract(
    REGISTRY_ADDRESS,
    ['function getTokenNetwork(address token) external view returns (address)'],
    provider
  );
  return await registry.getFunction('getTokenNetwork')(TOKEN_ADDRESS);
}

// ============================================================================
// Tests
// ============================================================================

describeEvm('Claim Validation Gate (BTP Transport Layer)', () => {
  let network: MultiHopTestNetwork;
  let portBase: number;
  let tokenNetworkAddress: string;

  beforeAll(async () => {
    // Verify Anvil + Faucet are healthy
    await waitForAnvilReady(30_000);

    // Resolve TokenNetwork address from registry
    tokenNetworkAddress = await getTokenNetworkAddress();

    // Create 2-peer network for isolated testing
    // Using random port range to avoid conflicts with other tests
    portBase = 10000 + Math.floor(Math.random() * 40000);
    network = createMultiHopTestNetwork(2, {
      settlementThreshold: 5000n,
      connectorFeePercentage: 0.1,
      pollingInterval: 100,
      logLevel: 'warn',
      portBase,
    });

    await network.start();
  });

  afterAll(async () => {
    if (network) {
      // Race the stop against a timeout to avoid afterAll hanging
      await Promise.race([network.stop(), new Promise((resolve) => setTimeout(resolve, 30_000))]);
    }
  });

  // T-CVG-001: No claim → F06 reject
  it('T-CVG-001: should reject ILP PREPARE with no payment channel claim (F06)', async () => {
    // Connect a raw BTP client to Peer1's server (simulating an attacker)
    const ws = await connectRawBTPClient(portBase, 'attacker-no-claim');

    try {
      const { packet } = createTestPrepare('test.peer1.receiver', 1000n);

      // Send PREPARE with NO claim in protocol data
      const result = await sendRawBTPPrepare(ws, packet);

      expect(result.type).toBe(PacketType.REJECT);
      const reject = result as ILPRejectPacket;
      expect(reject.code).toBe(ILPErrorCode.F06_UNEXPECTED_PAYMENT);
      expect(reject.message).toContain('No payment channel claim');
    } finally {
      ws.close();
    }
  });

  // T-CVG-002: Invalid signature → F06 reject
  it('T-CVG-002: should reject ILP PREPARE with invalid claim signature (F06)', async () => {
    const ws = await connectRawBTPClient(portBase, 'attacker-bad-sig');

    try {
      const { packet } = createTestPrepare('test.peer1.receiver', 1000n);

      // Sign claim with one key but claim a DIFFERENT signerAddress.
      // This makes the EIP-712 signature recovery mismatch: recovered != expected.
      const wrongPrivateKey = '0x' + randomBytes(32).toString('hex');
      const fakeSignerAddress = '0x' + 'aB'.repeat(20); // Address that doesn't match the key

      const claimProtocolData = await buildSignedClaim({
        channelId: '0x' + randomBytes(32).toString('hex'),
        nonce: 1,
        transferredAmount: 1000n,
        signerPrivateKey: wrongPrivateKey,
        signerAddress: fakeSignerAddress, // Mismatched: signed by wrongKey but claims fakeAddress
        senderId: 'attacker-bad-sig',
        chainId: ANVIL_CHAIN_ID,
        tokenNetworkAddress,
        tokenAddress: TOKEN_ADDRESS,
      });

      const result = await sendRawBTPPrepare(ws, packet, claimProtocolData);

      expect(result.type).toBe(PacketType.REJECT);
      const reject = result as ILPRejectPacket;
      expect(reject.code).toBe(ILPErrorCode.F06_UNEXPECTED_PAYMENT);
    } finally {
      ws.close();
    }
  });

  // T-CVG-003: Malformed claim structure → F06 reject
  it('T-CVG-003: should reject ILP PREPARE with malformed claim data (F06)', async () => {
    const ws = await connectRawBTPClient(portBase, 'attacker-malformed');

    try {
      const { packet } = createTestPrepare('test.peer1.receiver', 1000n);

      // Send malformed JSON as claim
      const malformedClaim = {
        protocolName: BTP_CLAIM_PROTOCOL.NAME,
        contentType: BTP_CLAIM_PROTOCOL.CONTENT_TYPE,
        data: Buffer.from(JSON.stringify({ garbage: true }), 'utf8'),
      };

      const result = await sendRawBTPPrepare(ws, packet, malformedClaim);

      expect(result.type).toBe(PacketType.REJECT);
      const reject = result as ILPRejectPacket;
      expect(reject.code).toBe(ILPErrorCode.F06_UNEXPECTED_PAYMENT);
      expect(reject.message).toContain('Invalid claim structure');
    } finally {
      ws.close();
    }
  });

  // T-CVG-004: Valid claim with valid signature → accepted (fulfill)
  it('T-CVG-004: should accept ILP PREPARE with valid signed claim', async () => {
    // Use Peer2's private key (Account 3) — a real account known to Peer1
    const signerPrivateKey = PEER_PRIVATE_KEYS[1]!;
    const signerAddress = PEER_EVM_ADDRESSES[1]!;

    const ws = await connectRawBTPClient(portBase, 'peer2');

    try {
      const { packet } = createTestPrepare('test.peer1.receiver', 1000n);

      // Build a properly signed claim
      // Use a deterministic channelId (doesn't need to exist on-chain for signature-only validation)
      const channelId = ethers.keccak256(
        ethers.solidityPacked(
          ['address', 'address', 'uint256'],
          [signerAddress, PEER_EVM_ADDRESSES[0], 1]
        )
      );

      const claimProtocolData = await buildSignedClaim({
        channelId,
        nonce: 1,
        transferredAmount: 1000n,
        signerPrivateKey,
        signerAddress,
        senderId: 'peer2',
        chainId: ANVIL_CHAIN_ID,
        tokenNetworkAddress,
        tokenAddress: TOKEN_ADDRESS,
      });

      const result = await sendRawBTPPrepare(ws, packet, claimProtocolData);

      // Should NOT be an F06 reject — the claim is valid
      if (result.type === PacketType.REJECT) {
        const reject = result as ILPRejectPacket;
        // It's OK to get other reject codes (e.g., T00 from fulfillment mismatch)
        // but NOT F06 — that would mean the claim was wrongly rejected
        expect(reject.code).not.toBe(ILPErrorCode.F06_UNEXPECTED_PAYMENT);
      }
      // If we get a FULFILL, even better — claim passed and local delivery succeeded
    } finally {
      ws.close();
    }
  });

  // T-CVG-005: Multi-hop forwarding still works (claims generated by per-packet claim service)
  it('T-CVG-005: should not reject multi-hop packets with F06 (auto-generated claims pass validation)', async () => {
    // Send a packet from Peer1 to Peer2 via the normal ConnectorNode.sendPacket() API.
    // The inter-peer forwarding goes over BTP and includes per-packet claims
    // generated by the PacketHandler's PerPacketClaimService.
    //
    // We verify the claim validation gate does NOT reject with F06.
    // The packet may still get a T00 reject (fulfillment mismatch from the
    // auto-fulfill handler) — that's a pre-existing issue unrelated to claim validation.
    const amount = 5000n;
    const result = await network.sendPacket(0, 'test.peer2.receiver', amount);

    if (result.type === PacketType.REJECT) {
      const reject = result as ILPRejectPacket;
      // Must NOT be F06 — that would mean the per-packet claim was wrongly rejected
      expect(reject.code).not.toBe(ILPErrorCode.F06_UNEXPECTED_PAYMENT);
    }
    // If FULFILL, the full pipeline worked end-to-end — even better
  });

  // T-CVG-006: Claim with missing self-describing fields on unknown channel → F06
  it('T-CVG-006: should reject claim without self-describing fields on unknown channel (F06)', async () => {
    const ws = await connectRawBTPClient(portBase, 'attacker-no-self-describe');

    try {
      const { packet } = createTestPrepare('test.peer1.receiver', 1000n);

      // Build a claim WITHOUT chainId/tokenNetworkAddress (missing self-describing fields)
      const signerPrivateKey = '0x' + randomBytes(32).toString('hex');
      const signerWallet = new ethers.Wallet(signerPrivateKey);

      const balanceProof = {
        channelId: '0x' + randomBytes(32).toString('hex'),
        nonce: 1,
        transferredAmount: 1000n,
        lockedAmount: 0n,
        locksRoot: '0x' + '0'.repeat(64),
      };

      // Sign with some domain (doesn't matter — the point is the claim lacks self-describing fields)
      const domain = getDomainSeparator(BigInt(ANVIL_CHAIN_ID), tokenNetworkAddress);
      const types = getBalanceProofTypes();
      const signature = await signerWallet.signTypedData(domain, types, balanceProof);

      const claim = {
        version: '1.0',
        blockchain: 'evm',
        messageId: `claim-test-${Date.now()}`,
        timestamp: new Date().toISOString(),
        senderId: 'attacker-no-self-describe',
        channelId: balanceProof.channelId,
        nonce: 1,
        transferredAmount: '1000',
        lockedAmount: '0',
        locksRoot: balanceProof.locksRoot,
        signature,
        signerAddress: signerWallet.address,
        // Intentionally NO chainId, tokenNetworkAddress, tokenAddress
      };

      const claimData = {
        protocolName: BTP_CLAIM_PROTOCOL.NAME,
        contentType: BTP_CLAIM_PROTOCOL.CONTENT_TYPE,
        data: Buffer.from(JSON.stringify(claim), 'utf8'),
      };

      const result = await sendRawBTPPrepare(ws, packet, claimData);

      expect(result.type).toBe(PacketType.REJECT);
      const reject = result as ILPRejectPacket;
      expect(reject.code).toBe(ILPErrorCode.F06_UNEXPECTED_PAYMENT);
      expect(reject.message).toContain('Unknown channel');
    } finally {
      ws.close();
    }
  });
});
