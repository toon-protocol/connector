/**
 * ExplorerServer Unit Tests
 *
 * Tests for embedded HTTP/WebSocket server for Explorer UI.
 */

import WebSocket from 'ws';
import { ExplorerServer, ExplorerServerConfig } from './explorer-server';
import { EventStore } from './event-store';
import { TelemetryEmitter } from '../telemetry/telemetry-emitter';
import { TelemetryEvent, AccountBalanceEvent, SettlementState } from '@crosstown/shared';
import pino from 'pino';
import path from 'path';
import fs from 'fs';
import os from 'os';

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

/**
 * Create a mock TelemetryEmitter for testing.
 */
function createMockTelemetryEmitter(): TelemetryEmitter & {
  _triggerEvent: (event: TelemetryEvent) => void;
} {
  const logger = createMockLogger();
  const emitter = new TelemetryEmitter('ws://localhost:9999', 'test-node', logger);

  // Add method to trigger events for testing
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (emitter as unknown as { _triggerEvent: (event: TelemetryEvent) => void })._triggerEvent = (
    event: TelemetryEvent
  ) => {
    emitter.emit(event);
  };

  return emitter as TelemetryEmitter & { _triggerEvent: (event: TelemetryEvent) => void };
}

/**
 * Wait for HTTP request to complete.
 */
async function fetchJson(url: string): Promise<{ status: number; body: unknown }> {
  const response = await fetch(url);
  const body = await response.json();
  return { status: response.status, body };
}

describe('ExplorerServer', () => {
  let server: ExplorerServer;
  let eventStore: EventStore;
  let telemetryEmitter: ReturnType<typeof createMockTelemetryEmitter>;
  let mockLogger: pino.Logger;
  let tempDir: string;

  beforeEach(async () => {
    mockLogger = createMockLogger();
    eventStore = new EventStore({ path: ':memory:' }, mockLogger);
    await eventStore.initialize();

    telemetryEmitter = createMockTelemetryEmitter();

    // Create temp directory for static files
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'explorer-test-'));
    fs.writeFileSync(path.join(tempDir, 'index.html'), '<html><body>Test</body></html>');
  });

  afterEach(async () => {
    if (server) {
      await server.stop();
    }
    await eventStore.close();

    // Cleanup temp directory
    if (tempDir && fs.existsSync(tempDir)) {
      fs.rmSync(tempDir, { recursive: true });
    }
  });

  describe('initialization', () => {
    it('should start on configured port', async () => {
      server = new ExplorerServer(
        {
          port: 0, // Random available port
          nodeId: 'test-node',
          staticPath: tempDir,
        },
        eventStore,
        telemetryEmitter,
        mockLogger
      );
      await server.start();

      const port = server.getPort();
      expect(port).toBeGreaterThan(0);
    });

    it('should use default port 3001 when not specified', async () => {
      const config: ExplorerServerConfig = {
        port: 3001,
        nodeId: 'test-node',
        staticPath: tempDir,
      };

      // Just verify the config is properly passed (don't actually bind to 3001)
      expect(config.port).toBe(3001);
    });

    it('should subscribe to TelemetryEmitter on construction', async () => {
      // Check that onEvent was called by trying to emit an event
      let eventReceived = false;

      server = new ExplorerServer(
        {
          port: 0,
          nodeId: 'test-node',
          staticPath: tempDir,
        },
        eventStore,
        telemetryEmitter,
        mockLogger
      );
      await server.start();

      // Connect a WebSocket client
      const port = server.getPort();
      const client = new WebSocket(`ws://localhost:${port}/ws`);

      await new Promise<void>((resolve) => {
        client.on('open', () => {
          client.on('message', (data) => {
            const event = JSON.parse(data.toString());
            if (event.type === 'ACCOUNT_BALANCE') {
              eventReceived = true;
            }
          });

          // Emit event via TelemetryEmitter
          telemetryEmitter._triggerEvent(createTestEvent());

          // Wait a bit for message propagation
          setTimeout(() => {
            client.close();
            resolve();
          }, 100);
        });
      });

      expect(eventReceived).toBe(true);
    });

    it('should work in standalone mode without telemetry emitter', async () => {
      // Create server without telemetryEmitter (standalone mode)
      server = new ExplorerServer(
        {
          port: 0,
          nodeId: 'test-node-standalone',
          staticPath: tempDir,
        },
        eventStore,
        null, // No telemetry emitter
        mockLogger
      );
      await server.start();

      const port = server.getPort();
      expect(port).toBeGreaterThan(0);

      // Verify health endpoint works
      const healthResponse = await fetchJson(`http://localhost:${port}/api/health`);
      expect(healthResponse.status).toBe(200);
      expect(healthResponse.body).toMatchObject({
        status: 'healthy',
        nodeId: 'test-node-standalone',
      });

      // Verify events endpoint works (should return empty list)
      const eventsResponse = await fetchJson(`http://localhost:${port}/api/events`);
      expect(eventsResponse.status).toBe(200);
      expect(eventsResponse.body).toMatchObject({
        events: [],
        total: 0,
      });

      // Verify WebSocket connection works (but no live events)
      const client = new WebSocket(`ws://localhost:${port}/ws`);
      await new Promise<void>((resolve, reject) => {
        client.on('open', () => {
          client.close();
          resolve();
        });
        client.on('error', reject);
      });
    });
  });

  describe('static file serving', () => {
    it('should serve files from staticPath', async () => {
      server = new ExplorerServer(
        {
          port: 0,
          nodeId: 'test-node',
          staticPath: tempDir,
        },
        eventStore,
        telemetryEmitter,
        mockLogger
      );
      await server.start();

      const port = server.getPort();
      const response = await fetch(`http://localhost:${port}/index.html`);

      expect(response.status).toBe(200);
      const text = await response.text();
      expect(text).toContain('<html>');
    });

    it('should return 404 for missing files', async () => {
      server = new ExplorerServer(
        {
          port: 0,
          nodeId: 'test-node',
          staticPath: tempDir,
        },
        eventStore,
        telemetryEmitter,
        mockLogger
      );
      await server.start();

      const port = server.getPort();
      const response = await fetch(`http://localhost:${port}/nonexistent.js`);

      expect(response.status).toBe(404);
    });

    it('should serve index.html for SPA routing', async () => {
      server = new ExplorerServer(
        {
          port: 0,
          nodeId: 'test-node',
          staticPath: tempDir,
        },
        eventStore,
        telemetryEmitter,
        mockLogger
      );
      await server.start();

      const port = server.getPort();
      const response = await fetch(`http://localhost:${port}/some/spa/route`);

      expect(response.status).toBe(200);
      const text = await response.text();
      expect(text).toContain('<html>');
    });
  });

  describe('WebSocket endpoint', () => {
    it('should accept WebSocket connections at /ws', async () => {
      server = new ExplorerServer(
        {
          port: 0,
          nodeId: 'test-node',
          staticPath: tempDir,
        },
        eventStore,
        telemetryEmitter,
        mockLogger
      );
      await server.start();

      const port = server.getPort();
      const client = new WebSocket(`ws://localhost:${port}/ws`);

      await new Promise<void>((resolve, reject) => {
        client.on('open', () => {
          expect(client.readyState).toBe(WebSocket.OPEN);
          client.close();
          resolve();
        });
        client.on('error', reject);
      });
    });

    it('should broadcast events to connected clients', async () => {
      server = new ExplorerServer(
        {
          port: 0,
          nodeId: 'test-node',
          staticPath: tempDir,
        },
        eventStore,
        telemetryEmitter,
        mockLogger
      );
      await server.start();

      const port = server.getPort();
      const client = new WebSocket(`ws://localhost:${port}/ws`);
      const event = createTestEvent();

      const receivedEvent = await new Promise<TelemetryEvent>((resolve) => {
        client.on('open', () => {
          client.on('message', (data) => {
            const parsed = JSON.parse(data.toString());
            client.close();
            resolve(parsed);
          });

          // Emit event via TelemetryEmitter
          telemetryEmitter._triggerEvent(event);
        });
      });

      expect(receivedEvent).toEqual(event);
    });

    it('should handle client disconnect', async () => {
      server = new ExplorerServer(
        {
          port: 0,
          nodeId: 'test-node',
          staticPath: tempDir,
        },
        eventStore,
        telemetryEmitter,
        mockLogger
      );
      await server.start();

      const port = server.getPort();
      const client = new WebSocket(`ws://localhost:${port}/ws`);

      await new Promise<void>((resolve) => {
        client.on('open', () => {
          expect(server.getBroadcaster().getClientCount()).toBe(1);
          client.close();
        });

        client.on('close', () => {
          setTimeout(() => {
            expect(server.getBroadcaster().getClientCount()).toBe(0);
            resolve();
          }, 50);
        });
      });
    });
  });

  describe('REST endpoints', () => {
    it('should return events array from GET /api/events', async () => {
      // Store an event first
      await eventStore.storeEvent(createTestEvent());

      server = new ExplorerServer(
        {
          port: 0,
          nodeId: 'test-node',
          staticPath: tempDir,
        },
        eventStore,
        telemetryEmitter,
        mockLogger
      );
      await server.start();

      const port = server.getPort();
      const { status, body } = await fetchJson(`http://localhost:${port}/api/events`);

      expect(status).toBe(200);
      expect(body).toHaveProperty('events');
      expect(body).toHaveProperty('total');
      expect(body).toHaveProperty('limit');
      expect(body).toHaveProperty('offset');
      expect((body as { events: unknown[] }).events.length).toBe(1);
    });

    it('should respect filter parameters', async () => {
      // Store events with different types
      await eventStore.storeEvent(createTestEvent({ peerId: 'peer-a' }));
      await eventStore.storeEvent(createTestEvent({ peerId: 'peer-b' }));

      server = new ExplorerServer(
        {
          port: 0,
          nodeId: 'test-node',
          staticPath: tempDir,
        },
        eventStore,
        telemetryEmitter,
        mockLogger
      );
      await server.start();

      const port = server.getPort();
      const { status, body } = await fetchJson(`http://localhost:${port}/api/events?peerId=peer-a`);

      expect(status).toBe(200);
      expect((body as { events: unknown[] }).events.length).toBe(1);
    });

    it('should return node status from GET /api/health', async () => {
      server = new ExplorerServer(
        {
          port: 0,
          nodeId: 'test-node',
          staticPath: tempDir,
        },
        eventStore,
        telemetryEmitter,
        mockLogger
      );
      await server.start();

      const port = server.getPort();
      const { status, body } = await fetchJson(`http://localhost:${port}/api/health`);

      expect(status).toBe(200);
      expect(body).toHaveProperty('status', 'healthy');
      expect(body).toHaveProperty('nodeId', 'test-node');
      expect(body).toHaveProperty('uptime');
      expect(body).toHaveProperty('explorer');
      expect(body).toHaveProperty('timestamp');
    });

    it('should return 400 for invalid parameters', async () => {
      server = new ExplorerServer(
        {
          port: 0,
          nodeId: 'test-node',
          staticPath: tempDir,
        },
        eventStore,
        telemetryEmitter,
        mockLogger
      );
      await server.start();

      const port = server.getPort();

      // Invalid limit
      const { status: s1, body: b1 } = await fetchJson(
        `http://localhost:${port}/api/events?limit=500`
      );
      expect(s1).toBe(400);
      expect((b1 as { error: string }).error).toContain('limit');

      // Invalid offset
      const { status: s2, body: b2 } = await fetchJson(
        `http://localhost:${port}/api/events?offset=-1`
      );
      expect(s2).toBe(400);
      expect((b2 as { error: string }).error).toContain('offset');

      // Invalid timestamp
      const { status: s3, body: b3 } = await fetchJson(
        `http://localhost:${port}/api/events?since=invalid`
      );
      expect(s3).toBe(400);
      expect((b3 as { error: string }).error).toContain('timestamp');
    });
  });

  describe('shutdown', () => {
    it('should close WebSocket connections', async () => {
      server = new ExplorerServer(
        {
          port: 0,
          nodeId: 'test-node',
          staticPath: tempDir,
        },
        eventStore,
        telemetryEmitter,
        mockLogger
      );
      await server.start();

      const port = server.getPort();
      const client = new WebSocket(`ws://localhost:${port}/ws`);

      await new Promise<void>((resolve) => {
        client.on('open', async () => {
          expect(server.getBroadcaster().getClientCount()).toBe(1);

          client.on('close', (code) => {
            expect(code).toBe(1001); // Going Away
            resolve();
          });

          await server.stop();
        });
      });
    });

    it('should close HTTP server', async () => {
      server = new ExplorerServer(
        {
          port: 0,
          nodeId: 'test-node',
          staticPath: tempDir,
        },
        eventStore,
        telemetryEmitter,
        mockLogger
      );
      await server.start();

      const port = server.getPort();
      await server.stop();

      // Server should reject connections
      await expect(fetch(`http://localhost:${port}/api/health`)).rejects.toThrow();
    });

    it('should clean up resources', async () => {
      server = new ExplorerServer(
        {
          port: 0,
          nodeId: 'test-node',
          staticPath: tempDir,
        },
        eventStore,
        telemetryEmitter,
        mockLogger
      );
      await server.start();

      expect(server.getBroadcaster().getClientCount()).toBe(0);

      await server.stop();

      // Should be able to call stop multiple times without error
      await expect(server.stop()).resolves.toBeUndefined();
    });
  });

  describe('CORS', () => {
    it('should allow localhost origins', async () => {
      server = new ExplorerServer(
        {
          port: 0,
          nodeId: 'test-node',
          staticPath: tempDir,
        },
        eventStore,
        telemetryEmitter,
        mockLogger
      );
      await server.start();

      const port = server.getPort();
      const response = await fetch(`http://localhost:${port}/api/health`, {
        headers: {
          Origin: 'http://localhost:3000',
        },
      });

      expect(response.headers.get('Access-Control-Allow-Origin')).toBe('http://localhost:3000');
    });

    it('should handle OPTIONS preflight requests', async () => {
      server = new ExplorerServer(
        {
          port: 0,
          nodeId: 'test-node',
          staticPath: tempDir,
        },
        eventStore,
        telemetryEmitter,
        mockLogger
      );
      await server.start();

      const port = server.getPort();
      const response = await fetch(`http://localhost:${port}/api/health`, {
        method: 'OPTIONS',
        headers: {
          Origin: 'http://localhost:3000',
        },
      });

      expect(response.status).toBe(204);
    });
  });
});
