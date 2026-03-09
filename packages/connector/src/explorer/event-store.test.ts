/**
 * EventStore Unit Tests
 *
 * Tests for libSQL-based telemetry event persistence.
 */

import { EventStore } from './event-store';
import {
  TelemetryEvent,
  AccountBalanceEvent,
  SettlementState,
  SettlementTriggeredEvent,
  AgentChannelOpenedEvent,
  AgentChannelPaymentSentEvent,
  PaymentChannelOpenedEvent,
} from '@crosstown/shared';
import pino from 'pino';

// Create mock logger for testing
function createMockLogger(): pino.Logger {
  return pino({ level: 'silent' });
}

/**
 * Helper to create a test AccountBalanceEvent with default values.
 */
function createAccountBalanceEvent(
  overrides: Partial<AccountBalanceEvent> = {}
): AccountBalanceEvent {
  return {
    type: 'ACCOUNT_BALANCE',
    nodeId: 'connector-a',
    peerId: 'peer-b',
    tokenId: 'ILP',
    debitBalance: '0',
    creditBalance: '1000',
    netBalance: '-1000',
    settlementState: SettlementState.IDLE,
    timestamp: '2026-01-24T12:00:00.000Z',
    ...overrides,
  };
}

/**
 * Helper to create a test SettlementTriggeredEvent.
 */
function createSettlementTriggeredEvent(
  overrides: Partial<SettlementTriggeredEvent> = {}
): SettlementTriggeredEvent {
  return {
    type: 'SETTLEMENT_TRIGGERED',
    nodeId: 'connector-a',
    peerId: 'peer-b',
    tokenId: 'ILP',
    currentBalance: '5500',
    threshold: '5000',
    exceedsBy: '500',
    triggerReason: 'THRESHOLD_EXCEEDED',
    timestamp: '2026-01-24T12:00:00.000Z',
    ...overrides,
  };
}

/**
 * Helper to create a test AgentChannelOpenedEvent with Unix ms timestamp.
 */
function createAgentChannelOpenedEvent(
  overrides: Partial<AgentChannelOpenedEvent> = {}
): AgentChannelOpenedEvent {
  return {
    type: 'AGENT_CHANNEL_OPENED',
    timestamp: 1737720000000,
    nodeId: 'connector-a',
    agentId: 'agent-001',
    channelId: '0xabc123',
    chain: 'evm',
    peerId: 'agent-002',
    amount: '1000000000000000000',
    ...overrides,
  };
}

/**
 * Helper to create a test AgentChannelPaymentSentEvent.
 */
function createAgentChannelPaymentSentEvent(
  overrides: Partial<AgentChannelPaymentSentEvent> = {}
): AgentChannelPaymentSentEvent {
  return {
    type: 'AGENT_CHANNEL_PAYMENT_SENT',
    timestamp: 1737720060000,
    nodeId: 'connector-a',
    agentId: 'agent-001',
    packetType: 'prepare',
    from: 'agent-001',
    to: 'peer-b',
    channelId: '0xabc123',
    amount: '100000000000000000',
    destination: 'g.agent.peer-b',
    ...overrides,
  };
}

/**
 * Helper to create a test PaymentChannelOpenedEvent.
 */
function createPaymentChannelOpenedEvent(
  overrides: Partial<PaymentChannelOpenedEvent> = {}
): PaymentChannelOpenedEvent {
  return {
    type: 'PAYMENT_CHANNEL_OPENED',
    timestamp: '2026-01-24T12:00:00.000Z',
    nodeId: 'connector-a',
    channelId: '0x1234567890abcdef',
    participants: [
      '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb',
      '0x8626f6940E2eb28930eFb4CeF49B2d1F2C9C1199',
    ],
    peerId: 'connector-b',
    tokenAddress: '0x5FbDB2315678afecb367f032d93F642f64180aa3',
    tokenSymbol: 'USDC',
    settlementTimeout: 86400,
    initialDeposits: {
      '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb': '1000000000000000000',
      '0x8626f6940E2eb28930eFb4CeF49B2d1F2C9C1199': '0',
    },
    ...overrides,
  };
}

describe('EventStore', () => {
  let eventStore: EventStore;
  let mockLogger: pino.Logger;

  beforeEach(async () => {
    mockLogger = createMockLogger();
    eventStore = new EventStore(
      {
        path: ':memory:',
        maxEventCount: 1000,
        maxAgeMs: 86400000, // 1 day
      },
      mockLogger
    );
    await eventStore.initialize();
  });

  afterEach(async () => {
    await eventStore.close();
  });

  // ============================================
  // Initialization Tests
  // ============================================
  describe('initialization', () => {
    it('should create database with in-memory path', async () => {
      const testStore = new EventStore({ path: ':memory:' }, mockLogger);
      await testStore.initialize();

      const count = await testStore.getEventCount();
      expect(count).toBe(0);

      await testStore.close();
    });

    it('should create events table with correct schema', async () => {
      const event = createAccountBalanceEvent();
      const id = await eventStore.storeEvent(event);

      expect(id).toBeGreaterThan(0);

      const stored = await eventStore.queryEvents({});
      expect(stored).toHaveLength(1);
      expect(stored[0]!.event_type).toBe('ACCOUNT_BALANCE');
      expect(stored[0]!.peer_id).toBe('peer-b');
      expect(stored[0]!.payload.type).toBe('ACCOUNT_BALANCE');
    });

    it('should create all required indexes', async () => {
      // Store event to ensure table exists
      await eventStore.storeEvent(createAccountBalanceEvent());

      // Query by event type (uses idx_events_type)
      const byType = await eventStore.queryEvents({ eventTypes: ['ACCOUNT_BALANCE'] });
      expect(byType).toHaveLength(1);

      // Query by peer (uses idx_events_peer)
      const byPeer = await eventStore.queryEvents({ peerId: 'peer-b' });
      expect(byPeer).toHaveLength(1);
    });

    it('should throw error when accessing uninitialized database', async () => {
      const uninitStore = new EventStore({ path: ':memory:' }, mockLogger);

      await expect(uninitStore.getEventCount()).rejects.toThrow(
        'EventStore not initialized. Call initialize() first.'
      );
    });

    it('should apply default config values', async () => {
      const testStore = new EventStore({ path: ':memory:' }, mockLogger);
      await testStore.initialize();

      // Store a single event and verify defaults don't interfere
      await testStore.storeEvent(createAccountBalanceEvent());
      const count = await testStore.getEventCount();
      expect(count).toBe(1);

      await testStore.close();
    });
  });

  // ============================================
  // Event Storage Tests
  // ============================================
  describe('storeEvent', () => {
    it('should store event with all extracted fields', async () => {
      const event = createAccountBalanceEvent({
        peerId: 'test-peer',
        netBalance: '5000',
      });

      const id = await eventStore.storeEvent(event);
      expect(id).toBeGreaterThan(0);

      const stored = await eventStore.queryEvents({});
      expect(stored).toHaveLength(1);
      expect(stored[0]!.peer_id).toBe('test-peer');
      expect(stored[0]!.amount).toBe('5000');
    });

    it('should return inserted row ID', async () => {
      const id1 = await eventStore.storeEvent(createAccountBalanceEvent());
      const id2 = await eventStore.storeEvent(createAccountBalanceEvent());

      expect(id1).toBeGreaterThan(0);
      expect(id2).toBe(id1 + 1);
    });

    it('should extract routing fields from AgentChannelPaymentSentEvent', async () => {
      // AgentChannelPaymentSentEvent has packetType, from, to, destination
      const event = createAgentChannelPaymentSentEvent();

      const id = await eventStore.storeEvent(event);
      expect(id).toBeGreaterThan(0);

      const stored = await eventStore.queryEvents({});
      expect(stored).toHaveLength(1);
      expect(stored[0]!.direction).toBe('sent');
      expect(stored[0]!.peer_id).toBe('peer-b'); // Extracted from 'to' field
      expect(stored[0]!.packet_id).toBe('0xabc123');
      expect(stored[0]!.packet_type).toBe('prepare');
      expect(stored[0]!.from_address).toBe('agent-001');
      expect(stored[0]!.to_address).toBe('peer-b');
      expect(stored[0]!.destination).toBe('g.agent.peer-b');
    });

    it('should normalize ISO 8601 timestamp to Unix ms', async () => {
      const event = createAccountBalanceEvent({
        timestamp: '2026-01-24T12:00:00.000Z',
      });

      await eventStore.storeEvent(event);

      const stored = await eventStore.queryEvents({});
      expect(stored).toHaveLength(1);
      expect(stored[0]!.timestamp).toBe(new Date('2026-01-24T12:00:00.000Z').getTime());
    });

    it('should preserve Unix ms timestamp', async () => {
      const timestamp = 1737720000000;
      const event = createAgentChannelOpenedEvent({ timestamp });

      await eventStore.storeEvent(event);

      const stored = await eventStore.queryEvents({});
      expect(stored).toHaveLength(1);
      expect(stored[0]!.timestamp).toBe(timestamp);
    });

    it('should store full event JSON in payload', async () => {
      const event = createAccountBalanceEvent({
        creditLimit: '10000',
        settlementThreshold: '5000',
      });

      await eventStore.storeEvent(event);

      const stored = await eventStore.queryEvents({});
      expect(stored).toHaveLength(1);

      const payload = stored[0]!.payload as AccountBalanceEvent;
      expect(payload.type).toBe('ACCOUNT_BALANCE');
      expect(payload.creditLimit).toBe('10000');
      expect(payload.settlementThreshold).toBe('5000');
    });

    it('should extract direction for AGENT_CHANNEL_PAYMENT_SENT', async () => {
      const event = createAgentChannelPaymentSentEvent();
      await eventStore.storeEvent(event);

      const stored = await eventStore.queryEvents({});
      expect(stored[0]!.direction).toBe('sent');
    });

    it('should calculate sum of initialDeposits for PAYMENT_CHANNEL_OPENED', async () => {
      const event = createPaymentChannelOpenedEvent();
      await eventStore.storeEvent(event);

      const stored = await eventStore.queryEvents({});
      expect(stored[0]!.amount).toBe('1000000000000000000');
    });
  });

  // ============================================
  // Batch Storage Tests
  // ============================================
  describe('storeEvents', () => {
    it('should store multiple events atomically', async () => {
      const events: TelemetryEvent[] = [
        createAccountBalanceEvent({ peerId: 'peer-1' }),
        createAccountBalanceEvent({ peerId: 'peer-2' }),
        createAccountBalanceEvent({ peerId: 'peer-3' }),
      ];

      await eventStore.storeEvents(events);

      const count = await eventStore.getEventCount();
      expect(count).toBe(3);
    });

    it('should extract fields correctly for all events in batch', async () => {
      const events: TelemetryEvent[] = [
        createAccountBalanceEvent({ peerId: 'peer-1', netBalance: '100' }),
        createSettlementTriggeredEvent({ peerId: 'peer-2', currentBalance: '200' }),
        createAgentChannelOpenedEvent({ peerId: 'agent-3', amount: '300' }),
      ];

      await eventStore.storeEvents(events);

      const stored = await eventStore.queryEvents({});
      expect(stored).toHaveLength(3);

      // Verify fields are correctly extracted (order by timestamp DESC)
      const balanceEvent = stored.find((e) => e.event_type === 'ACCOUNT_BALANCE');
      expect(balanceEvent?.peer_id).toBe('peer-1');
      expect(balanceEvent?.amount).toBe('100');

      const triggeredEvent = stored.find((e) => e.event_type === 'SETTLEMENT_TRIGGERED');
      expect(triggeredEvent?.peer_id).toBe('peer-2');
      expect(triggeredEvent?.amount).toBe('200');

      const channelEvent = stored.find((e) => e.event_type === 'AGENT_CHANNEL_OPENED');
      expect(channelEvent?.peer_id).toBe('agent-3');
      expect(channelEvent?.amount).toBe('300');
    });

    it('should handle empty array', async () => {
      await eventStore.storeEvents([]);

      const count = await eventStore.getEventCount();
      expect(count).toBe(0);
    });
  });

  // ============================================
  // Query API Tests
  // ============================================
  describe('queryEvents', () => {
    // Use consistent timestamps for all events
    const baseTime = new Date('2026-01-24T10:00:00.000Z').getTime();

    beforeEach(async () => {
      // Store multiple events for query tests with consistent timestamps
      await eventStore.storeEvents([
        createAccountBalanceEvent({ peerId: 'peer-a', timestamp: '2026-01-24T10:00:00.000Z' }), // baseTime
        createAccountBalanceEvent({ peerId: 'peer-b', timestamp: '2026-01-24T11:00:00.000Z' }), // +1hr
        createSettlementTriggeredEvent({ peerId: 'peer-a', timestamp: '2026-01-24T12:00:00.000Z' }), // +2hr
        createAgentChannelOpenedEvent({ peerId: 'agent-1', timestamp: baseTime + 3 * 3600000 }), // +3hr (13:00)
        createAgentChannelPaymentSentEvent({
          channelId: 'ch-1',
          timestamp: baseTime + 4 * 3600000,
        }), // +4hr (14:00)
      ]);
    });

    it('should return all events when no filter', async () => {
      const events = await eventStore.queryEvents({});
      expect(events).toHaveLength(5);
    });

    it('should filter by eventTypes', async () => {
      const events = await eventStore.queryEvents({
        eventTypes: ['ACCOUNT_BALANCE'],
      });

      expect(events).toHaveLength(2);
      events.forEach((e) => expect(e.event_type).toBe('ACCOUNT_BALANCE'));
    });

    it('should filter by multiple eventTypes', async () => {
      const events = await eventStore.queryEvents({
        eventTypes: ['ACCOUNT_BALANCE', 'SETTLEMENT_TRIGGERED'],
      });

      expect(events).toHaveLength(3);
    });

    it('should filter by time range (since)', async () => {
      const since = new Date('2026-01-24T11:30:00.000Z').getTime();
      const events = await eventStore.queryEvents({ since });

      // 12:00, 13:00, 14:00 are after 11:30
      expect(events).toHaveLength(3);
    });

    it('should filter by time range (until)', async () => {
      const until = new Date('2026-01-24T11:30:00.000Z').getTime();
      const events = await eventStore.queryEvents({ until });

      // 10:00, 11:00 are before 11:30
      expect(events).toHaveLength(2);
    });

    it('should filter by time range (since and until)', async () => {
      const since = new Date('2026-01-24T11:00:00.000Z').getTime();
      const until = new Date('2026-01-24T13:00:00.000Z').getTime();
      const events = await eventStore.queryEvents({ since, until });

      // 11:00, 12:00, 13:00 are between 11:00 and 13:00
      expect(events).toHaveLength(3);
    });

    it('should filter by peerId', async () => {
      const events = await eventStore.queryEvents({ peerId: 'peer-a' });

      expect(events).toHaveLength(2);
      events.forEach((e) => expect(e.peer_id).toBe('peer-a'));
    });

    it('should filter by packetId', async () => {
      const events = await eventStore.queryEvents({ packetId: 'ch-1' });

      expect(events).toHaveLength(1);
      expect(events[0]!.packet_id).toBe('ch-1');
    });

    it('should filter by direction', async () => {
      const events = await eventStore.queryEvents({ direction: 'sent' });

      expect(events).toHaveLength(1);
      expect(events[0]!.direction).toBe('sent');
      expect(events[0]!.event_type).toBe('AGENT_CHANNEL_PAYMENT_SENT');
    });

    it('should apply pagination with limit', async () => {
      const events = await eventStore.queryEvents({ limit: 2 });

      expect(events).toHaveLength(2);
    });

    it('should apply pagination with offset', async () => {
      const firstPage = await eventStore.queryEvents({ limit: 2, offset: 0 });
      const secondPage = await eventStore.queryEvents({ limit: 2, offset: 2 });

      expect(firstPage).toHaveLength(2);
      expect(secondPage).toHaveLength(2);
      expect(firstPage[0]!.id).not.toBe(secondPage[0]!.id);
    });

    it('should order by timestamp DESC (newest first)', async () => {
      const events = await eventStore.queryEvents({});

      for (let i = 1; i < events.length; i++) {
        expect(events[i - 1]!.timestamp).toBeGreaterThanOrEqual(events[i]!.timestamp);
      }
    });

    it('should return empty array when no matches', async () => {
      const events = await eventStore.queryEvents({ peerId: 'nonexistent' });

      expect(events).toHaveLength(0);
    });

    it('should combine multiple filters', async () => {
      const events = await eventStore.queryEvents({
        eventTypes: ['ACCOUNT_BALANCE'],
        peerId: 'peer-a',
      });

      expect(events).toHaveLength(1);
      expect(events[0]!.peer_id).toBe('peer-a');
      expect(events[0]!.event_type).toBe('ACCOUNT_BALANCE');
    });
  });

  // ============================================
  // Count Events Tests
  // ============================================
  describe('countEvents', () => {
    beforeEach(async () => {
      await eventStore.storeEvents([
        createAccountBalanceEvent({ peerId: 'peer-a' }),
        createAccountBalanceEvent({ peerId: 'peer-b' }),
        createSettlementTriggeredEvent({ peerId: 'peer-a' }),
      ]);
    });

    it('should count all events when no filter', async () => {
      const count = await eventStore.countEvents({});
      expect(count).toBe(3);
    });

    it('should count events matching filter', async () => {
      const count = await eventStore.countEvents({ eventTypes: ['ACCOUNT_BALANCE'] });
      expect(count).toBe(2);
    });

    it('should count events by peerId', async () => {
      const count = await eventStore.countEvents({ peerId: 'peer-a' });
      expect(count).toBe(2);
    });

    it('should return 0 when no matches', async () => {
      const count = await eventStore.countEvents({ peerId: 'nonexistent' });
      expect(count).toBe(0);
    });
  });

  // ============================================
  // Retention Policy Tests
  // ============================================
  describe('retention policy', () => {
    describe('pruneByAge', () => {
      it('should delete events older than maxAgeMs', async () => {
        // Create store with 1 second max age
        const shortRetentionStore = new EventStore(
          { path: ':memory:', maxAgeMs: 1000 },
          mockLogger
        );
        await shortRetentionStore.initialize();

        // Store old event (by manipulating timestamp)
        const oldTimestamp = Date.now() - 5000; // 5 seconds ago
        const event = createAgentChannelOpenedEvent({ timestamp: oldTimestamp });
        await shortRetentionStore.storeEvent(event);

        // Verify event exists
        let count = await shortRetentionStore.getEventCount();
        expect(count).toBe(1);

        // Prune
        const deleted = await shortRetentionStore.pruneByAge();
        expect(deleted).toBe(1);

        count = await shortRetentionStore.getEventCount();
        expect(count).toBe(0);

        await shortRetentionStore.close();
      });

      it('should keep events newer than maxAgeMs', async () => {
        // Store recent event with current timestamp (within retention window)
        await eventStore.storeEvent(
          createAccountBalanceEvent({ timestamp: new Date().toISOString() })
        );

        const deleted = await eventStore.pruneByAge();
        expect(deleted).toBe(0);

        const count = await eventStore.getEventCount();
        expect(count).toBe(1);
      });

      it('should return 0 when no events to prune', async () => {
        const deleted = await eventStore.pruneByAge();
        expect(deleted).toBe(0);
      });
    });

    describe('pruneByCount', () => {
      it('should delete oldest events exceeding maxEventCount', async () => {
        // Create store with max 2 events
        const smallStore = new EventStore({ path: ':memory:', maxEventCount: 2 }, mockLogger);
        await smallStore.initialize();

        // Store 5 events
        for (let i = 0; i < 5; i++) {
          await smallStore.storeEvent(createAgentChannelOpenedEvent({ timestamp: Date.now() + i }));
        }

        // Verify 5 events
        let count = await smallStore.getEventCount();
        expect(count).toBe(5);

        // Prune - should keep newest 2
        const deleted = await smallStore.pruneByCount();
        expect(deleted).toBe(3);

        count = await smallStore.getEventCount();
        expect(count).toBe(2);

        await smallStore.close();
      });

      it('should keep all events when under limit', async () => {
        await eventStore.storeEvent(createAccountBalanceEvent());

        const deleted = await eventStore.pruneByCount();
        expect(deleted).toBe(0);
      });
    });

    describe('runRetentionPolicy', () => {
      it('should run both age and count pruning', async () => {
        // Store some events with current timestamp (within retention window)
        await eventStore.storeEvent(
          createAccountBalanceEvent({ timestamp: new Date().toISOString() })
        );

        // This should complete without error
        await eventStore.runRetentionPolicy();

        // Events should still exist (within retention limits)
        const count = await eventStore.getEventCount();
        expect(count).toBe(1);
      });
    });
  });

  // ============================================
  // Database Lifecycle Tests
  // ============================================
  describe('lifecycle', () => {
    it('should close database connection', async () => {
      await eventStore.close();

      // Subsequent operations should fail
      await expect(eventStore.getEventCount()).rejects.toThrow();
    });

    it('should return correct event count', async () => {
      expect(await eventStore.getEventCount()).toBe(0);

      await eventStore.storeEvent(createAccountBalanceEvent());
      expect(await eventStore.getEventCount()).toBe(1);

      await eventStore.storeEvent(createAccountBalanceEvent());
      expect(await eventStore.getEventCount()).toBe(2);
    });

    it('should return database size', async () => {
      const size = await eventStore.getDatabaseSize();

      // In-memory database still has a size
      expect(size).toBeGreaterThanOrEqual(0);
    });
  });

  // ============================================
  // Edge Cases
  // ============================================
  describe('edge cases', () => {
    it('should handle large payload', async () => {
      const largeContent = 'x'.repeat(100000); // 100KB content
      const event = createAccountBalanceEvent();
      // Add extra field using Object.assign to avoid type errors
      const eventWithLargeField = Object.assign({}, event, { largeField: largeContent });

      const id = await eventStore.storeEvent(eventWithLargeField as TelemetryEvent);
      expect(id).toBeGreaterThan(0);

      const stored = await eventStore.queryEvents({});
      expect(stored).toHaveLength(1);
      expect((stored[0]!.payload as unknown as { largeField: string }).largeField).toBe(
        largeContent
      );
    });

    it('should handle query with no results returning empty array', async () => {
      const events = await eventStore.queryEvents({ eventTypes: ['NONEXISTENT'] });
      expect(events).toEqual([]);
    });

    it('should handle invalid time range (since > until)', async () => {
      await eventStore.storeEvent(createAccountBalanceEvent());

      const events = await eventStore.queryEvents({
        since: 2000000000000,
        until: 1000000000000,
      });

      // Should return empty (no events match impossible range)
      expect(events).toHaveLength(0);
    });

    it('should parse payload JSON correctly', async () => {
      const event = createAccountBalanceEvent({
        creditLimit: '10000',
        settlementThreshold: '5000',
      });
      await eventStore.storeEvent(event);

      const stored = await eventStore.queryEvents({});
      const payload = stored[0]!.payload as AccountBalanceEvent;

      expect(payload.creditLimit).toBe('10000');
      expect(payload.settlementThreshold).toBe('5000');
      expect(payload.settlementState).toBe(SettlementState.IDLE);
    });
  });
});
