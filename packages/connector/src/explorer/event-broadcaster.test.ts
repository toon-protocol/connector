/**
 * EventBroadcaster Unit Tests
 *
 * Tests for WebSocket event broadcasting to Explorer UI clients.
 */

import WebSocket, { WebSocketServer } from 'ws';
import { EventBroadcaster } from './event-broadcaster';
import { TelemetryEvent, AccountBalanceEvent, SettlementState } from '@crosstown/shared';
import pino from 'pino';
import { createServer, Server as HTTPServer } from 'http';

// Create mock logger for testing
function createMockLogger(): pino.Logger {
  return pino({ level: 'silent' });
}

/**
 * Helper to create a test AccountBalanceEvent.
 */
function createTestEvent(overrides: Partial<AccountBalanceEvent> = {}): AccountBalanceEvent {
  return {
    type: 'ACCOUNT_BALANCE',
    nodeId: 'connector-a',
    peerId: 'peer-b',
    tokenId: 'M2M',
    debitBalance: '0',
    creditBalance: '1000',
    netBalance: '-1000',
    settlementState: SettlementState.IDLE,
    timestamp: '2026-01-24T12:00:00.000Z',
    ...overrides,
  };
}

describe('EventBroadcaster', () => {
  let httpServer: HTTPServer;
  let wss: WebSocketServer;
  let broadcaster: EventBroadcaster;
  let serverPort: number;

  beforeEach((done) => {
    const logger = createMockLogger();

    // Create HTTP server on random port
    httpServer = createServer();
    httpServer.listen(0, () => {
      const address = httpServer.address();
      serverPort = typeof address === 'object' && address ? address.port : 0;

      // Create WebSocket server
      wss = new WebSocketServer({ server: httpServer, path: '/ws' });

      // Create EventBroadcaster
      broadcaster = new EventBroadcaster(wss, logger);

      done();
    });
  });

  afterEach((done) => {
    broadcaster.closeAll();
    wss.close(() => {
      httpServer.close(() => {
        done();
      });
    });
  });

  describe('connection handling', () => {
    it('should track connected clients', (done) => {
      expect(broadcaster.getClientCount()).toBe(0);

      const client = new WebSocket(`ws://localhost:${serverPort}/ws`);

      client.on('open', () => {
        expect(broadcaster.getClientCount()).toBe(1);
        client.close();
      });

      client.on('close', () => {
        // Give the server time to process the close
        setTimeout(() => {
          expect(broadcaster.getClientCount()).toBe(0);
          done();
        }, 50);
      });
    });

    it('should remove disconnected clients', (done) => {
      const client = new WebSocket(`ws://localhost:${serverPort}/ws`);

      client.on('open', () => {
        expect(broadcaster.getClientCount()).toBe(1);
        client.close();
      });

      client.on('close', () => {
        setTimeout(() => {
          expect(broadcaster.getClientCount()).toBe(0);
          done();
        }, 50);
      });
    });

    it('should return correct client count with multiple clients', (done) => {
      const clients: WebSocket[] = [];
      let connectedCount = 0;

      for (let i = 0; i < 3; i++) {
        const client = new WebSocket(`ws://localhost:${serverPort}/ws`);
        clients.push(client);

        client.on('open', () => {
          connectedCount++;
          if (connectedCount === 3) {
            expect(broadcaster.getClientCount()).toBe(3);

            // Close all clients
            clients.forEach((c) => c.close());
          }
        });
      }

      // Wait for all to close
      let closedCount = 0;
      clients.forEach((client) => {
        client.on('close', () => {
          closedCount++;
          if (closedCount === 3) {
            setTimeout(() => {
              expect(broadcaster.getClientCount()).toBe(0);
              done();
            }, 50);
          }
        });
      });
    });
  });

  describe('broadcasting', () => {
    it('should send event to all connected clients', (done) => {
      const event = createTestEvent();
      const receivedEvents: TelemetryEvent[] = [];

      // Connect 2 clients
      const client1 = new WebSocket(`ws://localhost:${serverPort}/ws`);
      const client2 = new WebSocket(`ws://localhost:${serverPort}/ws`);

      let connectedCount = 0;

      const onOpen = (): void => {
        connectedCount++;
        if (connectedCount === 2) {
          // Broadcast event
          broadcaster.broadcast(event);
        }
      };

      client1.on('open', onOpen);
      client2.on('open', onOpen);

      client1.on('message', (data) => {
        receivedEvents.push(JSON.parse(data.toString()));
        checkComplete();
      });

      client2.on('message', (data) => {
        receivedEvents.push(JSON.parse(data.toString()));
        checkComplete();
      });

      function checkComplete(): void {
        if (receivedEvents.length === 2) {
          expect(receivedEvents[0]).toEqual(event);
          expect(receivedEvents[1]).toEqual(event);
          client1.close();
          client2.close();
          done();
        }
      }
    });

    it('should not throw when no clients connected', () => {
      const event = createTestEvent();

      expect(broadcaster.getClientCount()).toBe(0);

      // Should not throw
      expect(() => broadcaster.broadcast(event)).not.toThrow();
    });

    it('should handle individual send failures gracefully', (done) => {
      const event = createTestEvent();

      const client = new WebSocket(`ws://localhost:${serverPort}/ws`);

      client.on('open', () => {
        // Force close the underlying socket to simulate error
        client.terminate();

        // Broadcast should not throw even with failed client
        setTimeout(() => {
          expect(() => broadcaster.broadcast(event)).not.toThrow();
          done();
        }, 50);
      });
    });
  });

  describe('closeAll', () => {
    it('should close all connections', (done) => {
      const clients: WebSocket[] = [];
      let connectedCount = 0;

      for (let i = 0; i < 3; i++) {
        const client = new WebSocket(`ws://localhost:${serverPort}/ws`);
        clients.push(client);

        client.on('open', () => {
          connectedCount++;
          if (connectedCount === 3) {
            expect(broadcaster.getClientCount()).toBe(3);

            // Close all via broadcaster
            broadcaster.closeAll();
          }
        });
      }

      // Wait for all to close
      let closedCount = 0;
      clients.forEach((client) => {
        client.on('close', (code) => {
          // Should receive 1001 (Going Away)
          expect(code).toBe(1001);
          closedCount++;
          if (closedCount === 3) {
            expect(broadcaster.getClientCount()).toBe(0);
            done();
          }
        });
      });
    });

    it('should clear client set', (done) => {
      const client = new WebSocket(`ws://localhost:${serverPort}/ws`);

      client.on('open', () => {
        expect(broadcaster.getClientCount()).toBe(1);
        broadcaster.closeAll();
        expect(broadcaster.getClientCount()).toBe(0);
        done();
      });
    });
  });
});
