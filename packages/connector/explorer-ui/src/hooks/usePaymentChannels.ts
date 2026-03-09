import { useState, useEffect, useCallback, useRef, useMemo } from 'react';
import { TelemetryEvent, ChannelState } from '../lib/event-types';

interface UsePaymentChannelsOptions {
  /** Reconnect delay in milliseconds */
  reconnectDelay?: number;
  /** Maximum reconnect attempts */
  maxReconnectAttempts?: number;
}

interface UsePaymentChannelsResult {
  /** List of channels sorted by lastActivityAt (most recent first) */
  channels: ChannelState[];
  /** Map of channels by channelId */
  channelsMap: Map<string, ChannelState>;
  /** Connection status including hydration */
  status: 'hydrating' | 'connecting' | 'connected' | 'disconnected' | 'error';
  /** Error message if status is 'error' */
  error: string | null;
  /** Total number of channels */
  totalChannels: number;
  /** Number of active channels */
  activeChannelCount: number;
  /** Clear all channel data */
  clearChannels: () => void;
  /** Manually reconnect */
  reconnect: () => void;
}

const DEFAULT_RECONNECT_DELAY = 1000;
const DEFAULT_MAX_RECONNECT_ATTEMPTS = 10;

/**
 * usePaymentChannels hook - tracks payment channel state from WebSocket events
 * Story 14.6 Task 10 - Full implementation
 */
export function usePaymentChannels(
  options: UsePaymentChannelsOptions = {}
): UsePaymentChannelsResult {
  const {
    reconnectDelay = DEFAULT_RECONNECT_DELAY,
    maxReconnectAttempts = DEFAULT_MAX_RECONNECT_ATTEMPTS,
  } = options;

  const [channelsMap, setChannelsMap] = useState<Map<string, ChannelState>>(new Map());
  const [status, setStatus] = useState<
    'hydrating' | 'connecting' | 'connected' | 'disconnected' | 'error'
  >('hydrating');
  const [error, setError] = useState<string | null>(null);

  const wsRef = useRef<WebSocket | null>(null);
  const reconnectAttemptsRef = useRef(0);
  const reconnectTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const hydratedRef = useRef(false);

  // RAF batching refs
  const bufferRef = useRef<TelemetryEvent[]>([]);
  const rafRef = useRef<number | null>(null);

  const CHANNEL_EVENT_TYPES = useMemo(
    () =>
      new Set([
        'PAYMENT_CHANNEL_OPENED',
        'PAYMENT_CHANNEL_BALANCE_UPDATE',
        'PAYMENT_CHANNEL_SETTLED',
        'AGENT_CHANNEL_OPENED',
        'AGENT_CHANNEL_BALANCE_UPDATE',
        'AGENT_CHANNEL_CLOSED',
      ]),
    []
  );

  /**
   * Apply a single channel event to the map (mutates the map in place)
   */
  const applyChannelEvent = useCallback((map: Map<string, ChannelState>, event: TelemetryEvent) => {
    const timestamp =
      typeof event.timestamp === 'string'
        ? event.timestamp
        : new Date(event.timestamp).toISOString();

    switch (event.type) {
      case 'PAYMENT_CHANNEL_OPENED': {
        const channelEvent = event as TelemetryEvent & {
          channelId: string;
          nodeId: string;
          peerId: string;
          participants: [string, string];
          tokenAddress: string;
          tokenSymbol: string;
          settlementTimeout: number;
          initialDeposits: Record<string, string>;
        };
        map.set(channelEvent.channelId, {
          channelId: channelEvent.channelId,
          nodeId: channelEvent.nodeId || '',
          peerId: channelEvent.peerId || '',
          participants: channelEvent.participants,
          tokenAddress: channelEvent.tokenAddress,
          tokenSymbol: channelEvent.tokenSymbol,
          settlementTimeout: channelEvent.settlementTimeout,
          deposits: channelEvent.initialDeposits,
          myNonce: 0,
          theirNonce: 0,
          myTransferred: '0',
          theirTransferred: '0',
          status: 'active',
          openedAt: timestamp,
          lastActivityAt: timestamp,
          settlementMethod: 'evm',
        });
        break;
      }

      case 'PAYMENT_CHANNEL_BALANCE_UPDATE': {
        const balanceEvent = event as TelemetryEvent & {
          channelId: string;
          myNonce: number;
          theirNonce: number;
          myTransferred: string;
          theirTransferred: string;
        };
        const existing = map.get(balanceEvent.channelId);
        if (existing) {
          map.set(balanceEvent.channelId, {
            ...existing,
            myNonce: balanceEvent.myNonce,
            theirNonce: balanceEvent.theirNonce,
            myTransferred: balanceEvent.myTransferred,
            theirTransferred: balanceEvent.theirTransferred,
            lastActivityAt: timestamp,
          });
        }
        break;
      }

      case 'PAYMENT_CHANNEL_SETTLED': {
        const settledEvent = event as TelemetryEvent & {
          channelId: string;
          finalBalances: Record<string, string>;
          settlementType: string;
        };
        const existing = map.get(settledEvent.channelId);
        if (existing) {
          map.set(settledEvent.channelId, {
            ...existing,
            status: 'settled',
            settledAt: timestamp,
            lastActivityAt: timestamp,
            deposits: settledEvent.finalBalances,
          });
        }
        break;
      }

      case 'AGENT_CHANNEL_OPENED': {
        const agentEvent = event as TelemetryEvent & {
          channelId: string;
          chain: 'evm';
          peerId: string;
          amount: string;
          nodeId?: string;
          agentId?: string;
        };
        map.set(agentEvent.channelId, {
          channelId: agentEvent.channelId,
          nodeId: agentEvent.nodeId || agentEvent.agentId || '',
          peerId: agentEvent.peerId || '',
          participants: [agentEvent.agentId || '', agentEvent.peerId],
          tokenAddress: 'AGENT',
          tokenSymbol: 'AGENT',
          settlementTimeout: 0,
          deposits: { [agentEvent.agentId || '']: agentEvent.amount },
          myNonce: 0,
          theirNonce: 0,
          myTransferred: '0',
          theirTransferred: '0',
          status: 'active',
          openedAt: timestamp,
          lastActivityAt: timestamp,
          settlementMethod: 'evm',
        });
        break;
      }

      case 'AGENT_CHANNEL_BALANCE_UPDATE': {
        const balanceEvent = event as TelemetryEvent & {
          channelId: string;
          peerId: string;
          previousBalance: string;
          newBalance: string;
          amount: string;
          direction: string;
        };
        const existing = map.get(balanceEvent.channelId);
        if (existing) {
          const transferred =
            balanceEvent.direction === 'outgoing'
              ? balanceEvent.newBalance
              : existing.myTransferred;
          const theirTransferred =
            balanceEvent.direction === 'incoming'
              ? balanceEvent.newBalance
              : existing.theirTransferred;
          map.set(balanceEvent.channelId, {
            ...existing,
            myTransferred: transferred,
            theirTransferred: theirTransferred,
            lastActivityAt: timestamp,
          });
        }
        break;
      }

      case 'AGENT_CHANNEL_CLOSED': {
        const closeEvent = event as TelemetryEvent & {
          channelId: string;
        };
        const existing = map.get(closeEvent.channelId);
        if (existing) {
          map.set(closeEvent.channelId, {
            ...existing,
            status: 'settled',
            settledAt: timestamp,
            lastActivityAt: timestamp,
          });
        }
        break;
      }
    }
  }, []);

  /**
   * Flush buffered channel events as a single state update
   */
  const flushBuffer = useCallback(() => {
    rafRef.current = null;
    const buffered = bufferRef.current;
    if (buffered.length === 0) return;
    bufferRef.current = [];

    setChannelsMap((prev) => {
      const newMap = new Map(prev);
      for (const event of buffered) {
        applyChannelEvent(newMap, event);
      }
      return newMap;
    });
  }, [applyChannelEvent]);

  /**
   * Connect to WebSocket
   */
  const connect = useCallback(() => {
    if (wsRef.current) {
      wsRef.current.close();
    }

    setStatus('connecting');
    setError(null);

    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const host = window.location.host;
    const wsUrl = `${protocol}//${host}/ws`;

    const ws = new WebSocket(wsUrl);

    ws.onopen = () => {
      setStatus('connected');
      setError(null);
      reconnectAttemptsRef.current = 0;
    };

    ws.onmessage = (messageEvent) => {
      try {
        const event = JSON.parse(messageEvent.data) as TelemetryEvent;
        if (CHANNEL_EVENT_TYPES.has(event.type)) {
          bufferRef.current.push(event);
          if (rafRef.current === null) {
            rafRef.current = requestAnimationFrame(flushBuffer);
          }
        }
      } catch {
        // Silently ignore parse errors
      }
    };

    ws.onerror = () => {
      setError('WebSocket connection error');
    };

    ws.onclose = () => {
      setStatus('disconnected');
      wsRef.current = null;

      if (reconnectAttemptsRef.current < maxReconnectAttempts) {
        const delay = reconnectDelay * Math.pow(2, reconnectAttemptsRef.current);
        reconnectAttemptsRef.current++;

        reconnectTimeoutRef.current = setTimeout(() => {
          connect();
        }, delay);
      } else {
        setStatus('error');
        setError('Max reconnect attempts reached');
      }
    };

    wsRef.current = ws;
  }, [CHANNEL_EVENT_TYPES, flushBuffer, reconnectDelay, maxReconnectAttempts]);

  const clearChannels = useCallback(() => {
    setChannelsMap(new Map());
  }, []);

  const reconnect = useCallback(() => {
    reconnectAttemptsRef.current = 0;
    connect();
  }, [connect]);

  /**
   * Hydrate channel state from historical events via REST API
   */
  const hydrate = useCallback(async () => {
    if (hydratedRef.current) {
      connect();
      return;
    }

    setStatus('hydrating');

    try {
      const baseUrl = `${window.location.protocol}//${window.location.host}`;
      const types = [
        'PAYMENT_CHANNEL_OPENED',
        'PAYMENT_CHANNEL_BALANCE_UPDATE',
        'PAYMENT_CHANNEL_SETTLED',
        'AGENT_CHANNEL_OPENED',
        'AGENT_CHANNEL_BALANCE_UPDATE',
        'AGENT_CHANNEL_CLOSED',
      ].join(',');
      const url = `${baseUrl}/api/accounts/events?types=${types}&limit=5000`;
      const response = await fetch(url);

      if (!response.ok) {
        throw new Error(`HTTP ${response.status}`);
      }

      const data = await response.json();
      const events = data.events as Array<{ payload: TelemetryEvent }>;

      if (events.length > 0) {
        setChannelsMap(() => {
          const newMap = new Map<string, ChannelState>();
          for (const storedEvent of events) {
            applyChannelEvent(newMap, storedEvent.payload);
          }
          return newMap;
        });
      }
    } catch {
      // Hydration failed — fall back to WebSocket-only behavior
    }

    hydratedRef.current = true;
    connect();
  }, [connect, applyChannelEvent]);

  // Hydrate on mount, then connect WebSocket
  useEffect(() => {
    hydrate();

    return () => {
      if (wsRef.current) {
        wsRef.current.close();
      }
      if (reconnectTimeoutRef.current) {
        clearTimeout(reconnectTimeoutRef.current);
      }
      // Flush remaining buffer on unmount
      if (rafRef.current !== null) {
        cancelAnimationFrame(rafRef.current);
      }
      if (bufferRef.current.length > 0) {
        const remaining = bufferRef.current;
        bufferRef.current = [];
        setChannelsMap((prev) => {
          const newMap = new Map(prev);
          for (const event of remaining) {
            applyChannelEvent(newMap, event);
          }
          return newMap;
        });
      }
    };
  }, [hydrate, applyChannelEvent]);

  // Sort channels by lastActivityAt (most recent first)
  const channels = useMemo(() => {
    return Array.from(channelsMap.values()).sort((a, b) => {
      return new Date(b.lastActivityAt).getTime() - new Date(a.lastActivityAt).getTime();
    });
  }, [channelsMap]);

  const totalChannels = channelsMap.size;

  const activeChannelCount = useMemo(() => {
    return Array.from(channelsMap.values()).filter((ch) => ch.status === 'active').length;
  }, [channelsMap]);

  return {
    channels,
    channelsMap,
    status,
    error,
    totalChannels,
    activeChannelCount,
    clearChannels,
    reconnect,
  };
}
