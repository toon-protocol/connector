/**
 * EventStore - libSQL-based telemetry event persistence for Explorer UI
 *
 * Provides persistent storage for telemetry events with:
 * - Efficient indexed lookups by event type, timestamp, peer, packet
 * - Full event JSON preserved in payload column
 * - Configurable retention policies (max age, max count)
 * - Query API with filtering and pagination
 *
 * @packageDocumentation
 */

import type { Client } from '@libsql/client';
import { TelemetryEvent } from '@crosstown/shared';
import { Logger } from '../utils/logger';
import { requireOptional } from '../utils/optional-require';

/**
 * Configuration for EventStore.
 */
export interface EventStoreConfig {
  /** Database file path (e.g., './data/explorer.db') or ':memory:' for in-memory */
  path: string;
  /** Maximum number of events to retain (default: 1000000) */
  maxEventCount?: number;
  /** Maximum event age in milliseconds (default: 7 days) */
  maxAgeMs?: number;
}

/**
 * Query filter for retrieving events.
 */
export interface EventQueryFilter {
  /** Filter by event type(s) */
  eventTypes?: string[];
  /** Unix timestamp (ms) lower bound */
  since?: number;
  /** Unix timestamp (ms) upper bound */
  until?: number;
  /** Filter by peer ID */
  peerId?: string;
  /** Filter by packet ID */
  packetId?: string;
  /** Filter by direction */
  direction?: 'sent' | 'received' | 'internal';
  /** Results per page (default: 50) */
  limit?: number;
  /** Pagination offset (default: 0) */
  offset?: number;
}

/**
 * Stored event with extracted indexed fields and parsed payload.
 */
export interface StoredEvent {
  /** Auto-incrementing row ID */
  id: number;
  /** Event type discriminator */
  event_type: string;
  /** Unix timestamp (milliseconds) */
  timestamp: number;
  /** Connector node ID */
  node_id: string;
  /** Direction: 'sent', 'received', 'internal', or null */
  direction: string | null;
  /** Peer ID if applicable */
  peer_id: string | null;
  /** Packet/channel ID if applicable */
  packet_id: string | null;
  /** Amount as string (bigint) if applicable */
  amount: string | null;
  /** Destination address if applicable */
  destination: string | null;
  /** ILP packet type: 'prepare', 'fulfill', 'reject', or null */
  packet_type: string | null;
  /** From address (packet sender) */
  from_address: string | null;
  /** To address (next hop) */
  to_address: string | null;
  /** Full event payload (parsed JSON) */
  payload: TelemetryEvent;
}

/**
 * Extracted indexed fields from a TelemetryEvent.
 */
interface ExtractedFields {
  event_type: string;
  timestamp: number;
  node_id: string;
  direction: string | null;
  peer_id: string | null;
  packet_id: string | null;
  amount: string | null;
  destination: string | null;
  packet_type: string | null;
  from_address: string | null;
  to_address: string | null;
}

// Default configuration values
const DEFAULT_MAX_EVENT_COUNT = 1000000;
const DEFAULT_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000; // 7 days
const DEFAULT_QUERY_LIMIT = 50;
const DEFAULT_QUERY_OFFSET = 0;

/**
 * Normalize timestamp to Unix milliseconds.
 *
 * TelemetryEvent types use two formats:
 * - ISO 8601 string (e.g., '2026-01-24T12:00:00.000Z')
 * - Unix milliseconds number (e.g., 1737720000000)
 *
 * @param event - The telemetry event
 * @returns Unix milliseconds timestamp
 */
function normalizeTimestamp(event: TelemetryEvent): number {
  const ts = (event as { timestamp: string | number }).timestamp;
  if (typeof ts === 'number') {
    return ts; // Already Unix ms
  }
  return new Date(ts).getTime(); // ISO 8601 → Unix ms
}

/**
 * Extract indexed fields from a TelemetryEvent.
 *
 * Parses event type-specific fields for efficient database indexing.
 *
 * @param event - The telemetry event
 * @returns Extracted fields for database storage
 */
function extractIndexedFields(event: TelemetryEvent): ExtractedFields {
  const base: ExtractedFields = {
    event_type: event.type,
    timestamp: normalizeTimestamp(event),
    node_id: (event as { nodeId: string }).nodeId,
    direction: null,
    peer_id: null,
    packet_id: null,
    amount: null,
    destination: null,
    packet_type: null,
    from_address: null,
    to_address: null,
  };

  // Extract fields based on event type
  switch (event.type) {
    case 'PACKET_RECEIVED':
      base.direction = 'received';
      base.packet_id = event.packetId;
      base.destination = event.destination;
      base.amount = event.amount;
      base.from_address = event.from;
      base.packet_type = 'prepare'; // PACKET_RECEIVED events are always ILP Prepare packets
      break;

    case 'PACKET_FORWARDED':
      base.direction = 'sent';
      base.packet_id = event.packetId;
      base.destination = event.destination;
      base.amount = event.amount;
      base.to_address = event.to;
      base.packet_type = 'prepare'; // PACKET_FORWARDED events are always ILP Prepare packets
      break;

    case 'PACKET_FULFILLED':
      base.direction = 'received';
      base.packet_id = event.packetId;
      base.destination = event.destination;
      base.amount = event.amount;
      base.from_address = event.from;
      base.packet_type = 'fulfill';
      break;

    case 'PACKET_REJECTED':
      base.direction = 'received';
      base.packet_id = event.packetId;
      base.destination = event.destination;
      base.amount = event.amount;
      base.from_address = event.from;
      base.packet_type = 'reject';
      break;

    case 'ACCOUNT_BALANCE':
      base.peer_id = event.peerId;
      base.amount = event.netBalance;
      break;

    case 'SETTLEMENT_TRIGGERED':
      base.peer_id = event.peerId;
      base.amount = event.currentBalance;
      break;

    case 'SETTLEMENT_COMPLETED':
      base.peer_id = event.peerId;
      base.amount = event.settledAmount;
      break;

    case 'PAYMENT_CHANNEL_OPENED':
      base.peer_id = event.peerId;
      base.packet_id = event.channelId;
      // Sum initial deposits as amount
      base.amount = Object.values(event.initialDeposits)
        .reduce((sum, val) => sum + BigInt(val), BigInt(0))
        .toString();
      break;

    case 'PAYMENT_CHANNEL_BALANCE_UPDATE':
      base.packet_id = event.channelId;
      base.amount = event.myTransferred;
      break;

    case 'PAYMENT_CHANNEL_SETTLED':
      base.packet_id = event.channelId;
      break;

    case 'AGENT_CHANNEL_OPENED':
      base.peer_id = event.peerId;
      base.packet_id = event.channelId;
      base.amount = event.amount;
      break;

    case 'AGENT_CHANNEL_PAYMENT_SENT':
      base.direction = 'sent';
      base.peer_id = event.to ?? event.peerId ?? null;
      // Use packetId for correlation, fall back to channelId
      base.packet_id = event.packetId ?? event.channelId ?? null;
      base.amount = event.amount;
      base.destination = event.destination;
      base.packet_type = event.packetType ?? null;
      base.from_address = event.from ?? null;
      base.to_address = event.to ?? null;
      break;

    case 'AGENT_CHANNEL_CLOSED':
      base.packet_id = event.channelId;
      break;

    case 'AGENT_BALANCE_CHANGED':
      base.amount = event.change;
      break;

    case 'AGENT_WALLET_FUNDED':
      // No specific indexed fields
      break;

    case 'AGENT_WALLET_STATE_CHANGED':
      base.direction = 'internal';
      break;

    case 'WALLET_BALANCE_MISMATCH':
      base.amount = event.actualBalance;
      break;

    case 'SUSPICIOUS_ACTIVITY_DETECTED':
      // No specific indexed fields beyond base
      break;

    case 'RATE_LIMIT_EXCEEDED':
      // No specific indexed fields beyond base
      break;

    case 'FUNDING_RATE_LIMIT_EXCEEDED':
      // No specific indexed fields beyond base
      break;

    case 'FUNDING_TRANSACTION_CONFIRMED':
      // No specific indexed fields beyond base
      break;

    case 'FUNDING_TRANSACTION_FAILED':
      // No specific indexed fields beyond base
      break;
  }

  return base;
}

/**
 * EventStore provides libSQL-based telemetry event persistence.
 *
 * Features:
 * - Store telemetry events with indexed field extraction
 * - Query events with filtering and pagination
 * - Configurable retention policies
 * - Batch storage for atomic transactions
 */
export class EventStore {
  private _client: Client | null = null;
  private readonly _config: Required<EventStoreConfig>;
  private readonly _logger: Logger;

  /**
   * Create an EventStore instance.
   *
   * @param config - EventStore configuration
   * @param logger - Pino logger instance
   */
  constructor(config: EventStoreConfig, logger: Logger) {
    this._config = {
      path: config.path,
      maxEventCount: config.maxEventCount ?? DEFAULT_MAX_EVENT_COUNT,
      maxAgeMs: config.maxAgeMs ?? DEFAULT_MAX_AGE_MS,
    };
    this._logger = logger;
  }

  /**
   * Initialize the database and create schema.
   */
  async initialize(): Promise<void> {
    const url = this._config.path === ':memory:' ? ':memory:' : `file:${this._config.path}`;

    const { createClient } = await requireOptional<typeof import('@libsql/client')>(
      '@libsql/client',
      'libSQL event storage for Explorer UI'
    );
    this._client = createClient({ url });

    // Create events table
    await this._client.execute(`
      CREATE TABLE IF NOT EXISTS events (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        event_type TEXT NOT NULL,
        timestamp INTEGER NOT NULL,
        node_id TEXT NOT NULL,
        direction TEXT,
        peer_id TEXT,
        packet_id TEXT,
        amount TEXT,
        destination TEXT,
        packet_type TEXT,
        from_address TEXT,
        to_address TEXT,
        payload TEXT NOT NULL
      )
    `);

    // Create indexes for efficient lookups
    await this._client.execute('CREATE INDEX IF NOT EXISTS idx_events_type ON events(event_type)');
    await this._client.execute(
      'CREATE INDEX IF NOT EXISTS idx_events_timestamp ON events(timestamp DESC)'
    );
    await this._client.execute('CREATE INDEX IF NOT EXISTS idx_events_packet ON events(packet_id)');
    await this._client.execute('CREATE INDEX IF NOT EXISTS idx_events_peer ON events(peer_id)');

    this._logger.info({ path: this._config.path }, 'EventStore initialized with telemetry schema');
  }

  /**
   * Get the database client, throwing if not initialized.
   */
  private _getClient(): Client {
    if (!this._client) {
      throw new Error('EventStore not initialized. Call initialize() first.');
    }
    return this._client;
  }

  /**
   * Store a single telemetry event.
   *
   * @param event - The telemetry event to store
   * @returns The inserted row ID
   */
  async storeEvent(event: TelemetryEvent): Promise<number> {
    const client = this._getClient();
    const fields = extractIndexedFields(event);

    const result = await client.execute({
      sql: `INSERT INTO events
        (event_type, timestamp, node_id, direction, peer_id, packet_id, amount, destination, packet_type, from_address, to_address, payload)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
      args: [
        fields.event_type,
        fields.timestamp,
        fields.node_id,
        fields.direction,
        fields.peer_id,
        fields.packet_id,
        fields.amount,
        fields.destination,
        fields.packet_type,
        fields.from_address,
        fields.to_address,
        JSON.stringify(event),
      ],
    });

    const rowId = Number(result.lastInsertRowid);

    this._logger.debug({ eventType: fields.event_type, rowId }, 'Stored telemetry event');

    return rowId;
  }

  /**
   * Store multiple telemetry events atomically.
   *
   * @param events - Array of telemetry events to store
   */
  async storeEvents(events: TelemetryEvent[]): Promise<void> {
    if (events.length === 0) {
      return;
    }

    const client = this._getClient();

    // Build batch statements
    const statements = events.map((event) => {
      const fields = extractIndexedFields(event);
      return {
        sql: `INSERT INTO events
          (event_type, timestamp, node_id, direction, peer_id, packet_id, amount, destination, packet_type, from_address, to_address, payload)
          VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
        args: [
          fields.event_type,
          fields.timestamp,
          fields.node_id,
          fields.direction,
          fields.peer_id,
          fields.packet_id,
          fields.amount,
          fields.destination,
          fields.packet_type,
          fields.from_address,
          fields.to_address,
          JSON.stringify(event),
        ],
      };
    });

    await client.batch(statements, 'write');

    this._logger.debug({ batchSize: events.length }, 'Stored batch of telemetry events');
  }

  /**
   * Query events with filtering and pagination.
   *
   * @param filter - Query filter criteria
   * @returns Array of stored events matching the filter
   */
  async queryEvents(
    filter: EventQueryFilter,
    sortOrder: 'ASC' | 'DESC' = 'DESC'
  ): Promise<StoredEvent[]> {
    const client = this._getClient();

    const conditions: string[] = [];
    const args: (string | number)[] = [];

    // Build WHERE clause dynamically
    if (filter.eventTypes && filter.eventTypes.length > 0) {
      const placeholders = filter.eventTypes.map(() => '?').join(', ');
      conditions.push(`event_type IN (${placeholders})`);
      args.push(...filter.eventTypes);
    }

    if (filter.since !== undefined) {
      conditions.push('timestamp >= ?');
      args.push(filter.since);
    }

    if (filter.until !== undefined) {
      conditions.push('timestamp <= ?');
      args.push(filter.until);
    }

    if (filter.peerId !== undefined) {
      conditions.push('peer_id = ?');
      args.push(filter.peerId);
    }

    if (filter.packetId !== undefined) {
      conditions.push('packet_id = ?');
      args.push(filter.packetId);
    }

    if (filter.direction !== undefined) {
      conditions.push('direction = ?');
      args.push(filter.direction);
    }

    const whereClause = conditions.length > 0 ? `WHERE ${conditions.join(' AND ')}` : '';
    const limit = filter.limit ?? DEFAULT_QUERY_LIMIT;
    const offset = filter.offset ?? DEFAULT_QUERY_OFFSET;

    const sql = `SELECT * FROM events ${whereClause} ORDER BY timestamp ${sortOrder} LIMIT ? OFFSET ?`;
    args.push(limit, offset);

    const result = await client.execute({ sql, args });

    return result.rows.map((row) => ({
      id: row.id as number,
      event_type: row.event_type as string,
      timestamp: row.timestamp as number,
      node_id: row.node_id as string,
      direction: row.direction as string | null,
      peer_id: row.peer_id as string | null,
      packet_id: row.packet_id as string | null,
      amount: row.amount as string | null,
      destination: row.destination as string | null,
      packet_type: (row.packet_type as string | null) ?? null,
      from_address: (row.from_address as string | null) ?? null,
      to_address: (row.to_address as string | null) ?? null,
      payload: JSON.parse(row.payload as string) as TelemetryEvent,
    }));
  }

  /**
   * Count events matching a filter.
   *
   * @param filter - Query filter criteria
   * @returns Count of matching events
   */
  async countEvents(filter: EventQueryFilter): Promise<number> {
    const client = this._getClient();

    const conditions: string[] = [];
    const args: (string | number)[] = [];

    // Build WHERE clause (same logic as queryEvents)
    if (filter.eventTypes && filter.eventTypes.length > 0) {
      const placeholders = filter.eventTypes.map(() => '?').join(', ');
      conditions.push(`event_type IN (${placeholders})`);
      args.push(...filter.eventTypes);
    }

    if (filter.since !== undefined) {
      conditions.push('timestamp >= ?');
      args.push(filter.since);
    }

    if (filter.until !== undefined) {
      conditions.push('timestamp <= ?');
      args.push(filter.until);
    }

    if (filter.peerId !== undefined) {
      conditions.push('peer_id = ?');
      args.push(filter.peerId);
    }

    if (filter.packetId !== undefined) {
      conditions.push('packet_id = ?');
      args.push(filter.packetId);
    }

    if (filter.direction !== undefined) {
      conditions.push('direction = ?');
      args.push(filter.direction);
    }

    const whereClause = conditions.length > 0 ? `WHERE ${conditions.join(' AND ')}` : '';

    const result = await client.execute({
      sql: `SELECT COUNT(*) as count FROM events ${whereClause}`,
      args,
    });

    return (result.rows[0]?.count as number) ?? 0;
  }

  /**
   * Delete events older than maxAgeMs.
   *
   * @returns Count of deleted events
   */
  async pruneByAge(): Promise<number> {
    const client = this._getClient();
    const cutoff = Date.now() - this._config.maxAgeMs;

    const result = await client.execute({
      sql: 'DELETE FROM events WHERE timestamp < ?',
      args: [cutoff],
    });

    const deleted = result.rowsAffected;

    if (deleted > 0) {
      this._logger.info({ deleted, cutoffMs: cutoff }, 'Pruned old events by age');
    }

    return deleted;
  }

  /**
   * Delete oldest events exceeding maxEventCount.
   *
   * @returns Count of deleted events
   */
  async pruneByCount(): Promise<number> {
    const client = this._getClient();

    const result = await client.execute({
      sql: `DELETE FROM events WHERE id NOT IN (
        SELECT id FROM events ORDER BY timestamp DESC LIMIT ?
      )`,
      args: [this._config.maxEventCount],
    });

    const deleted = result.rowsAffected;

    if (deleted > 0) {
      this._logger.info(
        { deleted, maxCount: this._config.maxEventCount },
        'Pruned old events by count'
      );
    }

    return deleted;
  }

  /**
   * Run full retention policy (prune by age and count).
   */
  async runRetentionPolicy(): Promise<void> {
    const ageDeleted = await this.pruneByAge();
    const countDeleted = await this.pruneByCount();

    const totalDeleted = ageDeleted + countDeleted;
    if (totalDeleted > 0) {
      this._logger.info({ totalDeleted }, 'Retention policy completed');
    }
  }

  /**
   * Get the total count of events in the database.
   *
   * @returns Total event count
   */
  async getEventCount(): Promise<number> {
    const client = this._getClient();

    const result = await client.execute('SELECT COUNT(*) as count FROM events');

    return (result.rows[0]?.count as number) ?? 0;
  }

  /**
   * Get the database size in bytes.
   *
   * @returns Database size in bytes
   */
  async getDatabaseSize(): Promise<number> {
    const client = this._getClient();

    const result = await client.execute(
      'SELECT page_count * page_size as size FROM pragma_page_count(), pragma_page_size()'
    );

    if (result.rows.length === 0 || result.rows[0]?.size === null) {
      return 0;
    }

    return result.rows[0]!.size as number;
  }

  /**
   * Close the database connection.
   */
  async close(): Promise<void> {
    if (this._client) {
      this._client.close();
      this._client = null;
      this._logger.info('EventStore closed');
    }
  }
}
