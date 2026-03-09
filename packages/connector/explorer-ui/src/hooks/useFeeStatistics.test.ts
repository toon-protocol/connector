/**
 * useFeeStatistics Hook Tests
 */

import { describe, it, expect } from 'vitest';
import { renderHook } from '@testing-library/react';
import { useFeeStatistics } from './useFeeStatistics';
import type { TelemetryEvent } from '../lib/event-types';
import type { PeerInfo } from './usePeers';

describe('useFeeStatistics', () => {
  const createPacketReceivedEvent = (
    packetId: string,
    from: string,
    amount: string,
    timestamp: number
  ): TelemetryEvent => ({
    type: 'PACKET_RECEIVED',
    nodeId: 'test-node',
    packetId,
    from,
    amount,
    destination: 'g.test.destination',
    timestamp,
  });

  const createPacketForwardedEvent = (
    packetId: string,
    to: string,
    amount: string,
    timestamp: number
  ): TelemetryEvent => ({
    type: 'PACKET_FORWARDED',
    nodeId: 'test-node',
    packetId,
    to,
    amount,
    destination: 'g.test.destination',
    timestamp,
  });

  const createPeer = (
    peerId: string,
    options: { evmAddress?: string; xrpAddress?: string } = {}
  ): PeerInfo => ({
    peerId,
    ilpAddress: `g.${peerId}`,
    connected: true,
    ...options,
  });

  it('returns empty stats when no events', () => {
    const { result } = renderHook(() => useFeeStatistics([], []));

    expect(result.current.stats).toHaveLength(0);
    expect(result.current.grandTotal).toBe(0n);
    expect(result.current.totalPackets).toBe(0);
    expect(result.current.loading).toBe(false);
  });

  it('calculates fees from matching packet events', () => {
    const events: TelemetryEvent[] = [
      createPacketReceivedEvent('packet-1', 'peer-a', '1000', Date.now()),
      createPacketForwardedEvent('packet-1', 'peer-b', '990', Date.now()),
    ];
    const peers: PeerInfo[] = [createPeer('peer-a'), createPeer('peer-b')];

    const { result } = renderHook(() => useFeeStatistics(events, peers));

    expect(result.current.totalPackets).toBe(1);
    // Fee should be 1000 - 990 = 10
    expect(result.current.grandTotal).toBe(10n);
  });

  it('aggregates fees by network based on peer addresses', () => {
    const now = Date.now();
    const events: TelemetryEvent[] = [
      // EVM peer
      createPacketReceivedEvent('packet-1', 'evm-peer', '1000', now),
      createPacketForwardedEvent('packet-1', 'other', '900', now),
      // XRP peer
      createPacketReceivedEvent('packet-2', 'xrp-peer', '2000', now),
      createPacketForwardedEvent('packet-2', 'other', '1800', now),
    ];

    const peers: PeerInfo[] = [
      createPeer('evm-peer', { evmAddress: '0x1234' }),
      createPeer('xrp-peer', { xrpAddress: 'rXRP123' }),
      createPeer('other'),
    ];

    const { result } = renderHook(() => useFeeStatistics(events, peers));

    expect(result.current.totalPackets).toBe(2);

    // Find stats by network
    const evmStats = result.current.stats.find((s) => s.network === 'evm');
    const xrpStats = result.current.stats.find((s) => s.network === 'evm');

    expect(evmStats?.totalFees).toBe(100n); // 1000 - 900
    expect(evmStats?.packetCount).toBe(1);

    expect(xrpStats?.totalFees).toBe(200n); // 2000 - 1800
    expect(xrpStats?.packetCount).toBe(1);
  });

  it('ignores packets without matching forward events', () => {
    const events: TelemetryEvent[] = [
      createPacketReceivedEvent('packet-1', 'peer-a', '1000', Date.now()),
      // No matching forward event
    ];
    const peers: PeerInfo[] = [createPeer('peer-a')];

    const { result } = renderHook(() => useFeeStatistics(events, peers));

    expect(result.current.totalPackets).toBe(0);
    expect(result.current.stats).toHaveLength(0);
  });

  it('ignores packets with zero or negative fees', () => {
    const events: TelemetryEvent[] = [
      createPacketReceivedEvent('packet-1', 'peer-a', '1000', Date.now()),
      createPacketForwardedEvent('packet-1', 'peer-b', '1000', Date.now()), // Same amount, no fee
    ];
    const peers: PeerInfo[] = [createPeer('peer-a'), createPeer('peer-b')];

    const { result } = renderHook(() => useFeeStatistics(events, peers));

    expect(result.current.totalPackets).toBe(0);
    expect(result.current.stats).toHaveLength(0);
  });

  it('calculates average fees correctly', () => {
    const now = Date.now();
    const events: TelemetryEvent[] = [
      createPacketReceivedEvent('packet-1', 'peer-a', '1000', now),
      createPacketForwardedEvent('packet-1', 'peer-b', '990', now), // Fee: 10
      createPacketReceivedEvent('packet-2', 'peer-a', '2000', now),
      createPacketForwardedEvent('packet-2', 'peer-b', '1970', now), // Fee: 30
    ];

    const peers: PeerInfo[] = [
      createPeer('peer-a', { evmAddress: '0x1234' }),
      createPeer('peer-b'),
    ];

    const { result } = renderHook(() => useFeeStatistics(events, peers));

    const evmStats = result.current.stats.find((s) => s.network === 'evm');

    expect(evmStats?.totalFees).toBe(40n); // 10 + 30
    expect(evmStats?.packetCount).toBe(2);
    expect(evmStats?.averageFee).toBe(20n); // 40 / 2
  });
});
