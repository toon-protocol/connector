/**
 * useFeeStatistics Hook
 *
 * Aggregates connector fees collected per network from packet telemetry events.
 * Calculates fees by comparing PACKET_RECEIVED and PACKET_FORWARDED amounts
 * for the same packetId, then groups by peer's settlement network.
 */

import { useMemo } from 'react';
import type { TelemetryEvent, StoredEvent } from '../lib/event-types';
import type { PeerInfo } from './usePeers';

/**
 * Token configuration for amount formatting
 */
export interface TokenConfig {
  /** Token symbol (e.g., "M2M", "USDC", "ETH") */
  symbol: string;
  /** Number of decimal places (e.g., 18 for most ERC20, 6 for USDC/XRP) */
  decimals: number;
}

/**
 * Default token configurations by network
 */
export const DEFAULT_TOKEN_CONFIGS: Record<string, TokenConfig> = {
  evm: { symbol: 'ETH', decimals: 18 },
  // Default for ILP/unknown - assumes M2M token with 18 decimals
  unknown: { symbol: 'M2M', decimals: 18 },
  // Common tokens
  usdc: { symbol: 'USDC', decimals: 6 },
  m2m: { symbol: 'M2M', decimals: 18 },
};

/**
 * Fee statistics for a single network
 */
export interface NetworkFeeStats {
  /** Network type: 'evm' or 'unknown' */
  network: 'evm' | 'unknown';
  /** Total fees collected (smallest unit) */
  totalFees: bigint;
  /** Total fees as formatted string with token symbol */
  totalFeesFormatted: string;
  /** Number of packets that collected fees */
  packetCount: number;
  /** Average fee per packet */
  averageFee: bigint;
  /** Average fee as formatted string with token symbol */
  averageFeeFormatted: string;
  /** Token configuration used for formatting */
  tokenConfig: TokenConfig;
}

/**
 * Fee aggregation result
 */
export interface FeeStatisticsResult {
  /** Fee stats per network */
  stats: NetworkFeeStats[];
  /** Grand total fees across all networks */
  grandTotal: bigint;
  /** Grand total formatted */
  grandTotalFormatted: string;
  /** Total packets processed */
  totalPackets: number;
  /** Whether data is being calculated */
  loading: boolean;
  /** Token configuration used */
  tokenConfig: TokenConfig;
}

/**
 * Packet data extracted from events
 */
interface PacketData {
  packetId: string;
  peerId: string;
  amount: bigint;
  timestamp: number;
}

/**
 * Determine primary network for a peer based on their configured addresses
 */
function getPeerNetwork(peer: PeerInfo | undefined): 'evm' | 'unknown' {
  if (!peer) return 'unknown';

  // Check if EVM address is configured
  const hasEvm = Boolean(peer.evmAddress);

  return hasEvm ? 'evm' : 'unknown';
}

/**
 * Format amount with token decimals and symbol
 * @param amount - Amount in smallest unit (wei, drops, octas, etc.)
 * @param tokenConfig - Token configuration with symbol and decimals
 * @returns Formatted amount with token symbol
 */
function formatAmount(amount: bigint, tokenConfig: TokenConfig): string {
  try {
    if (amount === 0n) return `0 ${tokenConfig.symbol}`;

    // Convert to decimal value
    const divisor = 10 ** tokenConfig.decimals;
    const decimalValue = Number(amount) / divisor;

    // Format based on magnitude
    if (decimalValue >= 1000000) {
      return `${(decimalValue / 1000000).toFixed(2)}M ${tokenConfig.symbol}`;
    }
    if (decimalValue >= 1000) {
      return `${(decimalValue / 1000).toFixed(2)}K ${tokenConfig.symbol}`;
    }
    if (decimalValue >= 1) {
      return `${decimalValue.toFixed(4)} ${tokenConfig.symbol}`;
    }
    if (decimalValue >= 0.0001) {
      return `${decimalValue.toFixed(6)} ${tokenConfig.symbol}`;
    }

    // For very small amounts, show in smallest unit
    const smallestUnit = tokenConfig.decimals === 18 ? 'wei' : 'units';
    return `${amount.toString()} ${smallestUnit}`;
  } catch {
    return `${amount.toString()} ${tokenConfig.symbol}`;
  }
}

/**
 * Extract packet data from a telemetry event
 */
function extractPacketData(
  event: TelemetryEvent | StoredEvent
): { received?: PacketData; forwarded?: PacketData } | null {
  // Handle StoredEvent (has payload property)
  const isStoredEvent = 'payload' in event;
  const telemetryEvent: TelemetryEvent = isStoredEvent
    ? (event as StoredEvent).payload
    : (event as TelemetryEvent);
  const eventType = isStoredEvent ? (event as StoredEvent).event_type : telemetryEvent.type;

  if (eventType === 'PACKET_RECEIVED') {
    const packetId = telemetryEvent.packetId as string | undefined;
    const from = telemetryEvent.from as string | undefined;
    const amount = telemetryEvent.amount as string | undefined;
    const timestamp = telemetryEvent.timestamp;

    if (packetId && from && amount) {
      return {
        received: {
          packetId,
          peerId: from,
          amount: BigInt(amount),
          timestamp: typeof timestamp === 'number' ? timestamp : Date.parse(timestamp),
        },
      };
    }
  }

  if (eventType === 'PACKET_FORWARDED') {
    const packetId = telemetryEvent.packetId as string | undefined;
    const to = telemetryEvent.to as string | undefined;
    const amount = telemetryEvent.amount as string | undefined;
    const timestamp = telemetryEvent.timestamp;

    if (packetId && to && amount) {
      return {
        forwarded: {
          packetId,
          peerId: to,
          amount: BigInt(amount),
          timestamp: typeof timestamp === 'number' ? timestamp : Date.parse(timestamp),
        },
      };
    }
  }

  return null;
}

/**
 * Hook to calculate fee statistics per network
 *
 * @param events - Array of telemetry events
 * @param peers - Array of peer info with network addresses
 * @param tokenConfig - Optional token configuration for amount formatting (defaults to M2M with 18 decimals)
 * @returns Fee statistics aggregated by network
 *
 * @example
 * ```tsx
 * const { peers } = usePeers();
 * // Use default M2M token (18 decimals)
 * const feeStats = useFeeStatistics(events, peers);
 *
 * // Or specify custom token
 * const usdcFees = useFeeStatistics(events, peers, { symbol: 'USDC', decimals: 6 });
 *
 * return (
 *   <div>
 *     {feeStats.stats.map(stat => (
 *       <div key={stat.network}>
 *         {stat.network}: {stat.totalFeesFormatted} ({stat.packetCount} packets)
 *       </div>
 *     ))}
 *   </div>
 * );
 * ```
 */
export function useFeeStatistics(
  events: (TelemetryEvent | StoredEvent)[],
  peers: PeerInfo[],
  tokenConfig?: TokenConfig
): FeeStatisticsResult {
  return useMemo(() => {
    // Use provided token config or default to M2M (18 decimals)
    const activeTokenConfig = tokenConfig || DEFAULT_TOKEN_CONFIGS.m2m;

    // Build peer lookup map
    const peerMap = new Map<string, PeerInfo>();
    for (const peer of peers) {
      peerMap.set(peer.peerId, peer);
    }

    // Track received and forwarded packets by packetId
    const receivedPackets = new Map<string, PacketData>();
    const forwardedPackets = new Map<string, PacketData>();

    // Process all events
    for (const event of events) {
      const packetData = extractPacketData(event);
      if (!packetData) continue;

      if (packetData.received) {
        receivedPackets.set(packetData.received.packetId, packetData.received);
      }
      if (packetData.forwarded) {
        forwardedPackets.set(packetData.forwarded.packetId, packetData.forwarded);
      }
    }

    // Calculate fees by matching received and forwarded packets
    // Fee = received amount - forwarded amount
    const feesByNetwork = new Map<'evm' | 'unknown', { totalFees: bigint; packetCount: number }>();

    // Initialize all networks
    feesByNetwork.set('evm', { totalFees: 0n, packetCount: 0 });
    feesByNetwork.set('unknown', { totalFees: 0n, packetCount: 0 });

    // Match packets and calculate fees
    for (const [packetId, received] of receivedPackets) {
      const forwarded = forwardedPackets.get(packetId);
      if (!forwarded) continue;

      // Fee = received - forwarded
      const fee = received.amount - forwarded.amount;
      if (fee <= 0n) continue; // Skip if no fee collected

      // Determine network from the receiving peer (who paid the fee)
      const peer = peerMap.get(received.peerId);
      const network = getPeerNetwork(peer);

      // Accumulate fees
      const current = feesByNetwork.get(network)!;
      current.totalFees += fee;
      current.packetCount += 1;
    }

    // Build result stats
    const stats: NetworkFeeStats[] = [];
    let grandTotal = 0n;
    let totalPackets = 0;

    for (const [network, data] of feesByNetwork) {
      if (data.packetCount > 0) {
        // Use network-specific token config if available, otherwise use the global config
        const networkTokenConfig = DEFAULT_TOKEN_CONFIGS[network] || activeTokenConfig;

        const averageFee = data.totalFees / BigInt(data.packetCount);
        stats.push({
          network,
          totalFees: data.totalFees,
          totalFeesFormatted: formatAmount(data.totalFees, networkTokenConfig),
          packetCount: data.packetCount,
          averageFee,
          averageFeeFormatted: formatAmount(averageFee, networkTokenConfig),
          tokenConfig: networkTokenConfig,
        });
        grandTotal += data.totalFees;
        totalPackets += data.packetCount;
      }
    }

    // Sort by total fees descending
    stats.sort((a, b) => (b.totalFees > a.totalFees ? 1 : -1));

    return {
      stats,
      grandTotal,
      grandTotalFormatted: formatAmount(grandTotal, activeTokenConfig),
      totalPackets,
      loading: false,
      tokenConfig: activeTokenConfig,
    };
  }, [events, peers, tokenConfig]);
}
