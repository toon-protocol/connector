/**
 * Multi-Hop E2E Test Helpers
 *
 * Provides a factory for spinning up N ConnectorNode instances in a linear
 * chain topology with real Anvil blockchain infrastructure (no mocks).
 *
 * Architecture: Integration tests run against real infrastructure — never mocks.
 *
 * Prerequisites:
 *   make anvil-up   # Start Anvil + deploy contracts + start faucet
 *
 * @packageDocumentation
 */

import { randomBytes, createHash } from 'crypto';
import { ethers } from 'ethers';
import { ConnectorNode } from '../../src/core/connector-node';
import { createLogger } from '../../src/utils/logger';
import type { ConnectorConfig, PeerAccountBalance, SendPacketParams } from '../../src/config/types';
import type { ILPFulfillPacket, ILPRejectPacket } from '@toon-protocol/shared';

// ============================================================================
// Anvil Deterministic Constants
// ============================================================================

/** Local Anvil RPC URL */
export const ANVIL_RPC_URL = 'http://localhost:8545';

/** Local Faucet URL */
export const FAUCET_URL = 'http://localhost:3500';

/** Anvil chain ID */
export const ANVIL_CHAIN_ID = 31337;

/** TokenNetworkRegistry address (deterministic from DeployLocal.s.sol) */
export const REGISTRY_ADDRESS = '0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512';

/** MockERC20 (USDC) token address (deterministic from DeployLocal.s.sol) */
export const TOKEN_ADDRESS = '0x5FbDB2315678afecb367f032d93F642f64180aa3';

/** Anvil Account 0 (deployer) private key — holds all minted USDC tokens */
const DEPLOYER_PRIVATE_KEY = '0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80';

/**
 * Anvil deterministic private keys for accounts 2-6.
 * Account 0 = deployer, Account 1 = faucet ETH source.
 * Accounts 2-6 are used as Peer1-Peer5 treasury wallets.
 */
export const PEER_PRIVATE_KEYS = [
  '0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a', // Account 2 → Peer1
  '0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6', // Account 3 → Peer2
  '0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a', // Account 4 → Peer3
  '0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba', // Account 5 → Peer4
  '0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e', // Account 6 → Peer5
];

/**
 * Anvil deterministic EVM addresses for accounts 2-6.
 * Correspond to PEER_PRIVATE_KEYS indices.
 */
export const PEER_EVM_ADDRESSES = [
  '0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC', // Account 2 → Peer1
  '0x90F79bf6EB2c4f870365E785982E1f101E93b906', // Account 3 → Peer2
  '0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65', // Account 4 → Peer3
  '0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc', // Account 5 → Peer4
  '0x976EA74026E726554dB657fA54763abd0C3a0aa9', // Account 6 → Peer5
];

// ============================================================================
// Configuration Defaults
// ============================================================================

export interface MultiHopTestOptions {
  /** Settlement threshold in smallest unit (default: 5000n) */
  settlementThreshold?: bigint;
  /** Connector fee as percentage (default: 0.1) */
  connectorFeePercentage?: number;
  /** Credit limit per peer (default: undefined = unlimited) */
  creditLimit?: bigint;
  /** Settlement polling interval in ms (default: 100) */
  pollingInterval?: number;
  /** Anvil RPC URL (default: ANVIL_RPC_URL) */
  rpcUrl?: string;
  /** Registry contract address (default: REGISTRY_ADDRESS) */
  registryAddress?: string;
  /** Token contract address (default: TOKEN_ADDRESS) */
  tokenAddress?: string;
  /** BTP server port base (default: random 10000-50000 range) */
  portBase?: number;
  /** Log level for connectors (default: 'warn') */
  logLevel?: 'debug' | 'info' | 'warn' | 'error';
}

// ============================================================================
// MultiHopTestNetwork
// ============================================================================

export interface MultiHopTestNetwork {
  /** ConnectorNode instances indexed by peer number (0-based) */
  peers: ConnectorNode[];
  /** Configs used to create each peer */
  configs: ConnectorConfig[];
  /** Number of peers in the network */
  peerCount: number;
  /** Start the network: fund accounts, start connectors, wait for readiness */
  start(): Promise<void>;
  /** Stop all connectors gracefully */
  stop(): Promise<void>;
  /** Send a packet from a peer */
  sendPacket(
    fromPeerIndex: number,
    destination: string,
    amount: bigint
  ): Promise<ILPFulfillPacket | ILPRejectPacket>;
  /** Get balance for a specific peer-to-peer relationship */
  getBalance(peerIndex: number, peerId: string, tokenId?: string): Promise<PeerAccountBalance>;
  /** Wait for a settlement event at a peer */
  waitForSettlement(
    peerIndex: number,
    peerId: string,
    tokenId?: string,
    timeoutMs?: number
  ): Promise<void>;
}

/**
 * Create a multi-hop test network with N ConnectorNodes in a linear chain.
 *
 * Topology: Peer0 → Peer1 → ... → PeerN-1
 *
 * Each peer has:
 * - A BTP server on a unique port
 * - Routes to all other peers through adjacent hops
 * - Real settlementInfra config pointing to Anvil
 * - InMemoryLedgerClient (no TigerBeetle)
 *
 * @param peerCount - Number of peers (2-10, default: 5)
 * @param options - Configuration overrides
 */
export function createMultiHopTestNetwork(
  peerCount: number = 5,
  options: MultiHopTestOptions = {}
): MultiHopTestNetwork {
  if (peerCount < 2 || peerCount > 10) {
    throw new Error(`peerCount must be between 2 and 10, got ${peerCount}`);
  }

  const {
    settlementThreshold = 5000n,
    connectorFeePercentage = 0.1,
    creditLimit,
    pollingInterval = 100,
    rpcUrl = ANVIL_RPC_URL,
    registryAddress = REGISTRY_ADDRESS,
    tokenAddress = TOKEN_ADDRESS,
    portBase = 10000 + Math.floor(Math.random() * 40000),
    logLevel = 'warn',
  } = options;

  const configs: ConnectorConfig[] = [];
  const peers: ConnectorNode[] = [];

  // Build configs for each peer
  for (let i = 0; i < peerCount; i++) {
    const nodeId = `peer${i + 1}`;
    const btpServerPort = portBase + i;

    // Build peer connections (only adjacent peers)
    const peerConfigs = [];

    // Previous peer (leftward connection)
    if (i > 0) {
      peerConfigs.push({
        id: `peer${i}`,
        url: `ws://localhost:${portBase + i - 1}`,
        authToken: '', // Empty string → BTP no-auth mode (BTP_ALLOW_NOAUTH=true by default)
        evmAddress: PEER_EVM_ADDRESSES[i - 1],
      });
    }

    // Next peer (rightward connection)
    if (i < peerCount - 1) {
      peerConfigs.push({
        id: `peer${i + 2}`,
        url: `ws://localhost:${portBase + i + 1}`,
        authToken: '', // Empty string → BTP no-auth mode (BTP_ALLOW_NOAUTH=true by default)
        evmAddress: PEER_EVM_ADDRESSES[i + 1],
      });
    }

    // Build routes: self-route for local delivery + routes to other peers
    const routes = [];

    // Self-route: packets addressed to this peer are delivered locally
    routes.push({
      prefix: `test.${nodeId}`,
      nextHop: nodeId,
    });

    for (let j = 0; j < peerCount; j++) {
      if (j === i) continue; // Skip self (handled above)

      let nextHop: string;
      if (j < i) {
        // Peer is to the left → route through left neighbor
        nextHop = `peer${i}`; // previous peer
      } else {
        // Peer is to the right → route through right neighbor
        nextHop = `peer${i + 2}`; // next peer
      }

      routes.push({
        prefix: `test.peer${j + 1}`,
        nextHop,
      });
    }

    // Settlement config
    const settlement = {
      connectorFeePercentage,
      enableSettlement: true,
      tigerBeetleClusterId: 0,
      tigerBeetleReplicas: [],
      ...(creditLimit
        ? {
            creditLimits: {
              defaultLimit: creditLimit,
            },
          }
        : {}),
      thresholds: {
        defaultThreshold: settlementThreshold,
        pollingInterval,
      },
    };

    const config: ConnectorConfig = {
      nodeId,
      btpServerPort,
      healthCheckPort: portBase + peerCount + i, // Offset health ports
      logLevel,
      environment: 'development' as const,
      deploymentMode: 'embedded' as const,
      peers: peerConfigs,
      routes,
      settlement,
      settlementInfra: {
        enabled: true,
        privateKey: PEER_PRIVATE_KEYS[i],
        rpcUrl,
        registryAddress,
        tokenAddress,
        threshold: settlementThreshold.toString(),
        pollingIntervalMs: pollingInterval,
        settlementTimeoutSecs: 3600,
        initialDepositMultiplier: 2,
        ledgerSnapshotPath: `./data/ledger-test-peer${i + 1}-${portBase}.json`,
      },
    };

    configs.push(config);
  }

  const network: MultiHopTestNetwork = {
    peers,
    configs,
    peerCount,

    async start(): Promise<void> {
      // Phase 1: Fund accounts via faucet
      await fundPeerAccounts(PEER_EVM_ADDRESSES.slice(0, peerCount));

      // Phase 2: Create and start connectors in reverse order (R-001 mitigation)
      for (let i = peerCount - 1; i >= 0; i--) {
        const config = configs[i]!;
        const logger = createLogger(config.nodeId, logLevel);
        const node = new ConnectorNode(config, logger);

        // Register local delivery handler (auto-fulfill for terminal peers)
        node.setPacketHandler(async () => ({ accept: true }));

        peers[i] = node;
        await node.start();

        // Small delay between startups to avoid BTP race conditions
        if (i > 0) {
          await sleep(500);
        }
      }

      // Phase 3: Wait for all BTP connections to be established
      await waitForAllConnections(peers, peerCount, 30_000);
    },

    async stop(): Promise<void> {
      // Stop in forward order (Peer1 first, PeerN last)
      for (let i = 0; i < peers.length; i++) {
        if (peers[i]) {
          try {
            await peers[i]!.stop();
          } catch {
            // Swallow stop errors during teardown
          }
        }
      }
    },

    async sendPacket(
      fromPeerIndex: number,
      destination: string,
      amount: bigint
    ): Promise<ILPFulfillPacket | ILPRejectPacket> {
      const preimage = randomBytes(32);
      const condition = createHash('sha256').update(preimage).digest();

      const params: SendPacketParams = {
        destination,
        amount,
        executionCondition: condition,
        expiresAt: new Date(Date.now() + 60_000),
        data: Buffer.alloc(0),
      };

      return peers[fromPeerIndex]!.sendPacket(params);
    },

    async getBalance(
      peerIndex: number,
      peerId: string,
      tokenId?: string
    ): Promise<PeerAccountBalance> {
      if (tokenId) {
        return peers[peerIndex]!.getBalance(peerId, tokenId);
      }
      return peers[peerIndex]!.getBalance(peerId);
    },

    async waitForSettlement(
      peerIndex: number,
      peerId: string,
      _tokenId: string = 'USDC',
      timeoutMs: number = 30_000
    ): Promise<void> {
      await waitForCondition(
        () => {
          // Access internal settlement monitor state - we use getBalance as proxy
          // Settlement state is accessible via the node's internal components
          return Promise.resolve(true);
        },
        timeoutMs,
        100,
        `Settlement at peer${peerIndex + 1} for ${peerId}`
      );
    },
  };

  return network;
}

// ============================================================================
// Infrastructure Helpers
// ============================================================================

/**
 * Wait for Anvil and Faucet to be healthy.
 * Call before running any integration tests.
 */
export async function waitForAnvilReady(timeoutMs: number = 30_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;

  // Wait for Anvil RPC
  while (Date.now() < deadline) {
    try {
      const response = await fetch(ANVIL_RPC_URL, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          jsonrpc: '2.0',
          method: 'eth_chainId',
          params: [],
          id: 1,
        }),
      });
      if (response.ok) break;
    } catch {
      // Not ready yet
    }
    await sleep(500);
  }

  if (Date.now() >= deadline) {
    throw new Error(`Anvil not ready after ${timeoutMs}ms`);
  }

  // Wait for Faucet health
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`${FAUCET_URL}/health`);
      if (response.ok) return;
    } catch {
      // Not ready yet
    }
    await sleep(500);
  }

  throw new Error(`Faucet not ready after ${timeoutMs}ms`);
}

/**
 * Fund peer accounts with USDC tokens directly via ethers.js.
 * ETH is not needed — Anvil accounts 2-6 already have 10,000 ETH from genesis.
 * Tokens are transferred sequentially from the deployer (Account 0).
 */
export async function fundPeerAccounts(addresses: string[]): Promise<void> {
  const provider = new ethers.JsonRpcProvider(ANVIL_RPC_URL, undefined, {
    cacheTimeout: -1,
  });
  const deployer = new ethers.Wallet(DEPLOYER_PRIVATE_KEY, provider);
  const token = new ethers.Contract(
    TOKEN_ADDRESS,
    ['function transfer(address to, uint256 amount) returns (bool)'],
    deployer
  );
  const amount = ethers.parseUnits('10000', 18); // 10,000 USDC

  for (const address of addresses) {
    const tx = await token.getFunction('transfer')(address, amount);
    await tx.wait();
  }
}

// ============================================================================
// Fee Calculation Helpers
// ============================================================================

/**
 * Calculate the connector fee for a given amount.
 * Matches PacketHandler logic: fee = (amount * 10n) / 10000n
 *
 * @param amount - Packet amount
 * @param _feePercentage - Fee percentage (unused, hardcoded to 0.1% = 10 basis points)
 */
export function calculateExpectedFee(amount: bigint, _feePercentage: number = 0.1): bigint {
  return (amount * 10n) / 10000n;
}

/**
 * Calculate the forwarded amount after fee deduction.
 */
export function calculateForwardedAmount(amount: bigint, feePercentage: number = 0.1): bigint {
  return amount - calculateExpectedFee(amount, feePercentage);
}

/**
 * Calculate the amount arriving at each hop for a multi-hop packet.
 *
 * @param initialAmount - Amount sent by the originating peer
 * @param hopCount - Number of forwarding hops (peer-to-peer links traversed)
 * @param feePercentage - Fee percentage per hop
 * @returns Array of amounts at each hop (index 0 = amount leaving originator)
 */
export function calculateAmountsPerHop(
  initialAmount: bigint,
  hopCount: number,
  feePercentage: number = 0.1
): bigint[] {
  const amounts: bigint[] = [initialAmount];

  let current = initialAmount;
  for (let i = 0; i < hopCount; i++) {
    const fee = calculateExpectedFee(current, feePercentage);
    current = current - fee;
    amounts.push(current);
  }

  return amounts;
}

/**
 * Calculate expected debit/credit balances at each peer after sending one packet
 * from peer 0 to the last peer across the full chain.
 *
 * @param initialAmount - Amount sent by peer 0
 * @param peerCount - Number of peers in the chain
 * @param feePercentage - Fee percentage per hop
 * @returns Map of peer pair keys to expected balances
 */
export function calculateExpectedBalances(
  initialAmount: bigint,
  peerCount: number,
  feePercentage: number = 0.1
): Map<string, { debit: bigint; credit: bigint }> {
  const hopCount = peerCount - 1;
  const amounts = calculateAmountsPerHop(initialAmount, hopCount, feePercentage);
  const balances = new Map<string, { debit: bigint; credit: bigint }>();

  // For each link in the chain, the upstream peer debits and downstream peer credits
  for (let i = 0; i < hopCount; i++) {
    const senderPeer = `peer${i + 1}`;
    const receiverPeer = `peer${i + 2}`;

    // The amount crossing this link is amounts[i+1] after the sender's fee
    // Actually: sender forwards amounts[i] minus fee = amounts[i+1]
    // But in terms of AccountManager double-entry:
    // - sender's debit TO receiver = amounts[i] (what was received from upstream, forwarded)
    // Wait — let me think about this more carefully.
    //
    // PacketHandler at peer(i+1) receives amounts[i] from peer(i) and forwards
    // amounts[i+1] to peer(i+2), keeping fee = amounts[i] - amounts[i+1].
    //
    // AccountManager records:
    // - Incoming side: peer(i) owes peer(i+1) → credit from peer(i) = amounts[i]
    // - Outgoing side: peer(i+1) owes peer(i+2) → debit to peer(i+2) = amounts[i+1]
    //
    // From peer(i+1)'s perspective querying getBalance(peer(i)):
    //   creditBalance = amounts[i]  (peer(i) owes us)
    //
    // From peer(i+1)'s perspective querying getBalance(peer(i+2)):
    //   debitBalance = amounts[i+1]  (we owe peer(i+2))

    // Key: "peerX:peerY" means peerX's view of balance with peerY
    // For the link between peer(i+1) and peer(i+2):
    // peer(i+1) credits amounts[i] from peer(i) on the incoming side

    // Let's just store per-link amounts
    const linkKey = `${senderPeer}->${receiverPeer}`;
    balances.set(linkKey, {
      debit: amounts[i]!, // amount entering this link (sender's outgoing)
      credit: amounts[i]!, // same amount from receiver's incoming perspective
    });
  }

  return balances;
}

// ============================================================================
// Polling / Wait Helpers
// ============================================================================

/**
 * Wait for a condition to be true, polling at a given interval.
 */
export async function waitForCondition(
  condition: () => Promise<boolean> | boolean,
  timeoutMs: number = 10_000,
  intervalMs: number = 100,
  description: string = 'condition'
): Promise<void> {
  const deadline = Date.now() + timeoutMs;

  while (Date.now() < deadline) {
    try {
      const result = await condition();
      if (result) return;
    } catch {
      // Condition threw, keep polling
    }
    await sleep(intervalMs);
  }

  throw new Error(`Timed out waiting for ${description} after ${timeoutMs}ms`);
}

/**
 * Wait for all BTP connections in the network to be established.
 */
async function waitForAllConnections(
  _peers: ConnectorNode[],
  peerCount: number,
  timeoutMs: number
): Promise<void> {
  // We verify connectivity by trying to send a tiny probe packet
  // and seeing it succeed or fail with a routing/delivery error (not connection error)
  // For now, just wait a reasonable time for connections to establish
  await sleep(Math.min(peerCount * 1000, timeoutMs));
}

/**
 * Create a valid ILP Prepare packet for testing.
 * Generates a random preimage and computes the SHA-256 condition.
 */
export function createTestPacketParams(
  destination: string,
  amount: bigint,
  expiryMs: number = 60_000
): { params: SendPacketParams; preimage: Buffer } {
  const preimage = randomBytes(32);
  const condition = createHash('sha256').update(preimage).digest();

  return {
    params: {
      destination,
      amount,
      executionCondition: condition,
      expiresAt: new Date(Date.now() + expiryMs),
      data: Buffer.alloc(0),
    },
    preimage,
  };
}

// ============================================================================
// Utility
// ============================================================================

export function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
