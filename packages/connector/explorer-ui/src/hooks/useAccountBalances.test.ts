import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, act, waitFor } from '@testing-library/react';
import { useAccountBalances } from './useAccountBalances';
import { createRAFMock } from '@/test/raf-helpers';

// Mock WebSocket
class MockWebSocket {
  static instances: MockWebSocket[] = [];
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  readyState: number = 0; // WebSocket.CONNECTING

  constructor() {
    MockWebSocket.instances.push(this);
  }

  close() {
    this.readyState = 3; // WebSocket.CLOSED
  }

  simulateOpen() {
    this.readyState = 1; // WebSocket.OPEN
    this.onopen?.();
  }

  simulateMessage(data: object) {
    this.onmessage?.({ data: JSON.stringify(data) });
  }

  simulateClose() {
    this.readyState = 3; // WebSocket.CLOSED
    this.onclose?.();
  }
}

const rafMock = createRAFMock();
const flushRAF = rafMock.flush;

/**
 * Create a mock fetch that returns stored events for hydration
 */
function createMockFetch(events: object[] = []) {
  return vi.fn().mockResolvedValue({
    ok: true,
    json: async () => ({
      events: events.map((e) => ({ payload: e })),
      total: events.length,
    }),
  });
}

describe('useAccountBalances', () => {
  beforeEach(() => {
    MockWebSocket.instances = [];
    rafMock.reset();
    vi.stubGlobal('WebSocket', MockWebSocket);
    vi.stubGlobal('requestAnimationFrame', rafMock.requestAnimationFrame);
    vi.stubGlobal('cancelAnimationFrame', rafMock.cancelAnimationFrame);
    vi.stubGlobal('fetch', createMockFetch());
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  describe('initialization', () => {
    it('initializes with empty accounts map', () => {
      const { result } = renderHook(() => useAccountBalances());

      expect(result.current.accounts).toEqual([]);
      expect(result.current.totalAccounts).toBe(0);
    });

    it('starts with hydrating status', () => {
      const { result } = renderHook(() => useAccountBalances());

      expect(result.current.status).toBe('hydrating');
    });

    it('transitions to connected after hydration and WebSocket opens', async () => {
      const { result } = renderHook(() => useAccountBalances());

      // Wait for hydration to complete and WS to be created
      await waitFor(() => {
        expect(MockWebSocket.instances.length).toBeGreaterThan(0);
      });

      await act(async () => {
        MockWebSocket.instances[0].simulateOpen();
      });

      expect(result.current.status).toBe('connected');
    });
  });

  describe('hydration', () => {
    it('populates accounts from REST API before WebSocket connects', async () => {
      const mockFetch = createMockFetch([
        {
          type: 'ACCOUNT_BALANCE',
          nodeId: 'connector-a',
          peerId: 'peer-b',
          tokenId: 'M2M',
          debitBalance: '0',
          creditBalance: '1000',
          netBalance: '-1000',
          settlementState: 'IDLE',
          timestamp: new Date().toISOString(),
        },
      ]);
      vi.stubGlobal('fetch', mockFetch);

      const { result } = renderHook(() => useAccountBalances());

      // Wait for hydration to complete
      await waitFor(() => {
        expect(result.current.totalAccounts).toBe(1);
      });

      expect(result.current.accounts[0].peerId).toBe('peer-b');
      expect(result.current.accounts[0].creditBalance).toBe(1000n);
      expect(mockFetch).toHaveBeenCalledWith(
        expect.stringContaining(
          '/api/accounts/events?types=ACCOUNT_BALANCE,AGENT_CHANNEL_PAYMENT_SENT'
        )
      );
    });

    it('hydrates AGENT_CHANNEL_PAYMENT_SENT events', async () => {
      const mockFetch = createMockFetch([
        {
          type: 'AGENT_CHANNEL_PAYMENT_SENT',
          nodeId: 'agent-0',
          peerId: 'agent-1',
          amount: '500',
          packetType: 'fulfill',
          channelId: 'ch-1',
          timestamp: new Date().toISOString(),
        },
      ]);
      vi.stubGlobal('fetch', mockFetch);

      const { result } = renderHook(() => useAccountBalances());

      await waitFor(() => {
        expect(result.current.totalAccounts).toBe(1);
      });

      expect(result.current.accounts[0].peerId).toBe('agent-1');
      expect(result.current.accounts[0].tokenId).toBe('AGENT');
      expect(result.current.accounts[0].debitBalance).toBe(500n);
    });

    it('falls back to WebSocket-only if hydration fetch fails', async () => {
      const mockFetch = vi.fn().mockRejectedValue(new Error('Network error'));
      vi.stubGlobal('fetch', mockFetch);

      const { result } = renderHook(() => useAccountBalances());

      // Wait for WebSocket to be created after failed hydration
      await waitFor(() => {
        expect(MockWebSocket.instances.length).toBeGreaterThan(0);
      });

      await act(async () => {
        MockWebSocket.instances[0].simulateOpen();
      });

      expect(result.current.status).toBe('connected');
      expect(result.current.totalAccounts).toBe(0);
    });

    it('merges WebSocket events on top of hydrated state', async () => {
      const mockFetch = createMockFetch([
        {
          type: 'ACCOUNT_BALANCE',
          nodeId: 'connector-a',
          peerId: 'peer-b',
          tokenId: 'M2M',
          debitBalance: '0',
          creditBalance: '1000',
          netBalance: '-1000',
          settlementState: 'IDLE',
          timestamp: new Date(Date.now() - 10000).toISOString(),
        },
      ]);
      vi.stubGlobal('fetch', mockFetch);

      const { result } = renderHook(() => useAccountBalances());

      // Wait for hydration
      await waitFor(() => {
        expect(result.current.totalAccounts).toBe(1);
      });

      expect(result.current.accounts[0].creditBalance).toBe(1000n);

      // Wait for WebSocket to be created
      await waitFor(() => {
        expect(MockWebSocket.instances.length).toBeGreaterThan(0);
      });

      await act(async () => {
        MockWebSocket.instances[0].simulateOpen();
      });

      // Send updated balance via WebSocket
      await act(async () => {
        MockWebSocket.instances[0].simulateMessage({
          type: 'ACCOUNT_BALANCE',
          nodeId: 'connector-a',
          peerId: 'peer-b',
          tokenId: 'M2M',
          debitBalance: '0',
          creditBalance: '5000',
          netBalance: '-5000',
          settlementState: 'IDLE',
          timestamp: new Date().toISOString(),
        });
      });

      await flushRAF();

      // Latest data should win
      expect(result.current.totalAccounts).toBe(1);
      expect(result.current.accounts[0].creditBalance).toBe(5000n);
    });

    it('does not re-hydrate after reconnect', async () => {
      const mockFetch = createMockFetch();
      vi.stubGlobal('fetch', mockFetch);

      const { result } = renderHook(() => useAccountBalances());

      // Wait for hydration and WS creation
      await waitFor(() => {
        expect(MockWebSocket.instances.length).toBeGreaterThan(0);
      });

      await act(async () => {
        MockWebSocket.instances[0].simulateOpen();
      });

      expect(mockFetch).toHaveBeenCalledTimes(1);

      // Trigger reconnect
      act(() => {
        result.current.reconnect();
      });

      // Fetch should NOT be called again
      expect(mockFetch).toHaveBeenCalledTimes(1);
    });
  });

  describe('account state updates', () => {
    it('updates account state on ACCOUNT_BALANCE event', async () => {
      const { result } = renderHook(() => useAccountBalances());

      await waitFor(() => {
        expect(MockWebSocket.instances.length).toBeGreaterThan(0);
      });

      await act(async () => {
        MockWebSocket.instances[0].simulateOpen();
      });

      await act(async () => {
        MockWebSocket.instances[0].simulateMessage({
          type: 'ACCOUNT_BALANCE',
          nodeId: 'connector-a',
          peerId: 'peer-b',
          tokenId: 'M2M',
          debitBalance: '0',
          creditBalance: '1000',
          netBalance: '-1000',
          settlementState: 'IDLE',
          timestamp: new Date().toISOString(),
        });
      });

      await flushRAF();

      expect(result.current.totalAccounts).toBe(1);
      expect(result.current.accounts[0].peerId).toBe('peer-b');
      expect(result.current.accounts[0].tokenId).toBe('M2M');
      expect(result.current.accounts[0].creditBalance).toBe(1000n);
    });

    it('ignores non-ACCOUNT_BALANCE events', async () => {
      const { result } = renderHook(() => useAccountBalances());

      await waitFor(() => {
        expect(MockWebSocket.instances.length).toBeGreaterThan(0);
      });

      await act(async () => {
        MockWebSocket.instances[0].simulateOpen();
      });

      await act(async () => {
        MockWebSocket.instances[0].simulateMessage({
          type: 'PACKET_RECEIVED',
          nodeId: 'connector-a',
          timestamp: new Date().toISOString(),
        });
      });

      await flushRAF();

      expect(result.current.totalAccounts).toBe(0);
    });

    it('batches multiple balance updates into single state update', async () => {
      const { result } = renderHook(() => useAccountBalances());

      await waitFor(() => {
        expect(MockWebSocket.instances.length).toBeGreaterThan(0);
      });

      await act(async () => {
        MockWebSocket.instances[0].simulateOpen();
      });

      // Send multiple events in quick succession
      await act(async () => {
        MockWebSocket.instances[0].simulateMessage({
          type: 'ACCOUNT_BALANCE',
          peerId: 'peer-a',
          tokenId: 'M2M',
          debitBalance: '0',
          creditBalance: '1000',
          netBalance: '-1000',
          settlementState: 'IDLE',
          timestamp: new Date().toISOString(),
        });
        MockWebSocket.instances[0].simulateMessage({
          type: 'ACCOUNT_BALANCE',
          peerId: 'peer-b',
          tokenId: 'M2M',
          debitBalance: '0',
          creditBalance: '2000',
          netBalance: '-2000',
          settlementState: 'IDLE',
          timestamp: new Date().toISOString(),
        });
      });

      // Before flush, no updates
      expect(result.current.totalAccounts).toBe(0);

      await flushRAF();

      // After single flush, both accounts present
      expect(result.current.totalAccounts).toBe(2);
    });
  });

  describe('balance history tracking', () => {
    it('tracks balance history entries', async () => {
      const { result } = renderHook(() => useAccountBalances());

      await waitFor(() => {
        expect(MockWebSocket.instances.length).toBeGreaterThan(0);
      });

      await act(async () => {
        MockWebSocket.instances[0].simulateOpen();
      });

      // Send multiple balance updates
      await act(async () => {
        MockWebSocket.instances[0].simulateMessage({
          type: 'ACCOUNT_BALANCE',
          peerId: 'peer-b',
          tokenId: 'M2M',
          debitBalance: '0',
          creditBalance: '1000',
          netBalance: '-1000',
          settlementState: 'IDLE',
          timestamp: new Date().toISOString(),
        });
      });

      await flushRAF();

      await act(async () => {
        MockWebSocket.instances[0].simulateMessage({
          type: 'ACCOUNT_BALANCE',
          peerId: 'peer-b',
          tokenId: 'M2M',
          debitBalance: '0',
          creditBalance: '2000',
          netBalance: '-2000',
          settlementState: 'IDLE',
          timestamp: new Date().toISOString(),
        });
      });

      await flushRAF();

      expect(result.current.accounts[0].balanceHistory.length).toBe(2);
    });
  });

  describe('sorting', () => {
    it('returns accounts sorted by net balance (highest first)', async () => {
      const { result } = renderHook(() => useAccountBalances());

      await waitFor(() => {
        expect(MockWebSocket.instances.length).toBeGreaterThan(0);
      });

      await act(async () => {
        MockWebSocket.instances[0].simulateOpen();
      });

      await act(async () => {
        MockWebSocket.instances[0].simulateMessage({
          type: 'ACCOUNT_BALANCE',
          peerId: 'peer-low',
          tokenId: 'M2M',
          debitBalance: '0',
          creditBalance: '0',
          netBalance: '-1000',
          settlementState: 'IDLE',
          timestamp: new Date().toISOString(),
        });
        MockWebSocket.instances[0].simulateMessage({
          type: 'ACCOUNT_BALANCE',
          peerId: 'peer-high',
          tokenId: 'M2M',
          debitBalance: '0',
          creditBalance: '0',
          netBalance: '5000',
          settlementState: 'IDLE',
          timestamp: new Date().toISOString(),
        });
      });

      await flushRAF();

      expect(result.current.totalAccounts).toBe(2);
      // Highest net balance should be first
      expect(result.current.accounts[0].peerId).toBe('peer-high');
      expect(result.current.accounts[1].peerId).toBe('peer-low');
    });
  });

  describe('near threshold count', () => {
    it('counts accounts near settlement threshold (>70%)', async () => {
      const { result } = renderHook(() => useAccountBalances());

      await waitFor(() => {
        expect(MockWebSocket.instances.length).toBeGreaterThan(0);
      });

      await act(async () => {
        MockWebSocket.instances[0].simulateOpen();
      });

      await act(async () => {
        MockWebSocket.instances[0].simulateMessage({
          type: 'ACCOUNT_BALANCE',
          peerId: 'peer-near',
          tokenId: 'M2M',
          debitBalance: '0',
          creditBalance: '8000',
          netBalance: '-8000',
          settlementThreshold: '10000',
          settlementState: 'IDLE',
          timestamp: new Date().toISOString(),
        });
      });

      await flushRAF();

      expect(result.current.nearThresholdCount).toBe(1);
    });
  });

  describe('clear accounts', () => {
    it('clears all account data', async () => {
      const { result } = renderHook(() => useAccountBalances());

      await waitFor(() => {
        expect(MockWebSocket.instances.length).toBeGreaterThan(0);
      });

      await act(async () => {
        MockWebSocket.instances[0].simulateOpen();
      });

      await act(async () => {
        MockWebSocket.instances[0].simulateMessage({
          type: 'ACCOUNT_BALANCE',
          peerId: 'peer-b',
          tokenId: 'M2M',
          debitBalance: '0',
          creditBalance: '1000',
          netBalance: '-1000',
          settlementState: 'IDLE',
          timestamp: new Date().toISOString(),
        });
      });

      await flushRAF();

      expect(result.current.totalAccounts).toBe(1);

      act(() => {
        result.current.clearAccounts();
      });

      expect(result.current.totalAccounts).toBe(0);
    });
  });
});
