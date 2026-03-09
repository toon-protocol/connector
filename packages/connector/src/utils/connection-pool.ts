import { Logger } from 'pino';
import { EventEmitter } from 'events';

export interface Connection<T> {
  endpoint: string;
  client: T;
  isHealthy: boolean;
  lastHealthCheck: number;
}

export interface ConnectionPoolConfig {
  poolSize: number;
  endpoints: string[];
  healthCheckIntervalMs?: number; // Default: 30000 (30 seconds)
  reconnectDelayMs?: number; // Default: 1000 (1 second)
  maxReconnectAttempts?: number; // Default: 5
}

export interface ConnectionFactory<T> {
  create(endpoint: string): Promise<T>;
  disconnect(client: T): Promise<void>;
  healthCheck(client: T): Promise<boolean>;
}

/**
 * Generic connection pool with round-robin selection.
 * Supports automatic reconnection on failure and periodic health checks.
 */
export class ConnectionPool<T> extends EventEmitter {
  private readonly logger: Logger;
  private readonly config: Required<ConnectionPoolConfig>;
  private readonly factory: ConnectionFactory<T>;
  private readonly connections: Connection<T>[];
  private currentIndex: number;
  private healthCheckTimer?: NodeJS.Timeout;
  private isShuttingDown: boolean;

  constructor(config: ConnectionPoolConfig, factory: ConnectionFactory<T>, logger: Logger) {
    super();
    this.logger = logger.child({ component: 'connection-pool' });
    this.config = {
      poolSize: config.poolSize,
      endpoints: config.endpoints,
      healthCheckIntervalMs: config.healthCheckIntervalMs || 30000,
      reconnectDelayMs: config.reconnectDelayMs || 1000,
      maxReconnectAttempts: config.maxReconnectAttempts || 5,
    };
    this.factory = factory;
    this.connections = [];
    this.currentIndex = 0;
    this.isShuttingDown = false;

    this.logger.info(
      {
        poolSize: this.config.poolSize,
        endpoints: this.config.endpoints,
        healthCheckIntervalMs: this.config.healthCheckIntervalMs,
      },
      'ConnectionPool initialized'
    );
  }

  /**
   * Initialize the connection pool by creating all connections
   */
  async initialize(): Promise<void> {
    this.logger.info('Initializing connection pool');

    const endpointsToUse = this.config.endpoints.slice(0, this.config.poolSize);

    for (const endpoint of endpointsToUse) {
      try {
        const client = await this.factory.create(endpoint);
        this.connections.push({
          endpoint,
          client,
          isHealthy: true,
          lastHealthCheck: Date.now(),
        });

        this.logger.info({ endpoint }, 'Connection created successfully');
      } catch (error) {
        this.logger.error(
          { endpoint, error: (error as Error).message },
          'Failed to create connection'
        );

        // Add placeholder for failed connection (will be reconnected by health check)
        this.connections.push({
          endpoint,
          client: null as unknown as T,
          isHealthy: false,
          lastHealthCheck: Date.now(),
        });
      }
    }

    // Start periodic health checks
    this.startHealthChecks();

    this.logger.info(
      {
        totalConnections: this.connections.length,
        healthyConnections: this.connections.filter((c) => c.isHealthy).length,
      },
      'Connection pool initialized'
    );
  }

  /**
   * Get the next healthy connection using round-robin selection
   */
  getConnection(): Connection<T> | null {
    if (this.connections.length === 0) {
      this.logger.warn('No connections available in pool');
      return null;
    }

    // Try to find a healthy connection starting from current index
    let attempts = 0;

    do {
      const connection = this.connections[this.currentIndex];

      // Move to next connection for next call (round-robin)
      this.currentIndex = (this.currentIndex + 1) % this.connections.length;

      if (connection && connection.isHealthy) {
        this.logger.trace({ endpoint: connection.endpoint }, 'Connection selected from pool');
        return connection;
      }

      attempts++;
    } while (attempts < this.connections.length);

    this.logger.warn('No healthy connections available in pool');
    return null;
  }

  /**
   * Get connection statistics
   */
  getStats(): {
    totalConnections: number;
    healthyConnections: number;
    unhealthyConnections: number;
  } {
    const healthyCount = this.connections.filter((c) => c.isHealthy).length;

    return {
      totalConnections: this.connections.length,
      healthyConnections: healthyCount,
      unhealthyConnections: this.connections.length - healthyCount,
    };
  }

  /**
   * Start periodic health checks
   */
  private startHealthChecks(): void {
    if (this.healthCheckTimer) {
      clearInterval(this.healthCheckTimer);
    }

    this.healthCheckTimer = setInterval(async () => {
      await this.performHealthChecks();
    }, this.config.healthCheckIntervalMs);

    // Prevent timer from keeping process alive
    this.healthCheckTimer.unref();
  }

  /**
   * Perform health checks on all connections
   */
  private async performHealthChecks(): Promise<void> {
    if (this.isShuttingDown) {
      return;
    }

    this.logger.debug('Performing health checks on all connections');

    for (const connection of this.connections) {
      try {
        if (connection.client && connection.isHealthy) {
          // Check healthy connections
          const isHealthy = await this.factory.healthCheck(connection.client);

          if (!isHealthy) {
            this.logger.warn({ endpoint: connection.endpoint }, 'Connection health check failed');
            connection.isHealthy = false;

            this.emit('connection-unhealthy', { endpoint: connection.endpoint });

            // Attempt to reconnect
            this.reconnectConnection(connection);
          }

          connection.lastHealthCheck = Date.now();
        } else if (!connection.isHealthy) {
          // Attempt to reconnect unhealthy connections
          await this.reconnectConnection(connection);
        }
      } catch (error) {
        this.logger.error(
          {
            endpoint: connection.endpoint,
            error: (error as Error).message,
          },
          'Error during health check'
        );

        connection.isHealthy = false;
      }
    }

    const stats = this.getStats();
    this.logger.debug(
      {
        healthyConnections: stats.healthyConnections,
        unhealthyConnections: stats.unhealthyConnections,
      },
      'Health checks completed'
    );
  }

  /**
   * Attempt to reconnect a failed connection
   */
  private async reconnectConnection(connection: Connection<T>): Promise<void> {
    let attempts = 0;

    while (attempts < this.config.maxReconnectAttempts && !this.isShuttingDown) {
      try {
        this.logger.info(
          {
            endpoint: connection.endpoint,
            attempt: attempts + 1,
            maxAttempts: this.config.maxReconnectAttempts,
          },
          'Attempting to reconnect'
        );

        // Disconnect old client if exists
        if (connection.client) {
          try {
            await this.factory.disconnect(connection.client);
          } catch (error) {
            this.logger.warn(
              {
                endpoint: connection.endpoint,
                error: (error as Error).message,
              },
              'Error disconnecting old client'
            );
          }
        }

        // Create new client
        const newClient = await this.factory.create(connection.endpoint);
        connection.client = newClient;
        connection.isHealthy = true;
        connection.lastHealthCheck = Date.now();

        this.logger.info({ endpoint: connection.endpoint }, 'Connection reconnected successfully');

        this.emit('connection-reconnected', { endpoint: connection.endpoint });

        return;
      } catch (error) {
        this.logger.warn(
          {
            endpoint: connection.endpoint,
            attempt: attempts + 1,
            error: (error as Error).message,
          },
          'Reconnection attempt failed'
        );

        attempts++;

        if (attempts < this.config.maxReconnectAttempts) {
          // Wait before next attempt (unref to avoid keeping process alive)
          await new Promise((resolve) => {
            const timer = setTimeout(resolve, this.config.reconnectDelayMs);
            if (typeof timer.unref === 'function') timer.unref();
          });
        }
      }
    }

    this.logger.error(
      {
        endpoint: connection.endpoint,
        attempts,
      },
      'Failed to reconnect after maximum attempts'
    );

    this.emit('connection-failed', { endpoint: connection.endpoint });
  }

  /**
   * Shutdown the connection pool
   */
  async shutdown(): Promise<void> {
    this.logger.info('Shutting down connection pool');
    this.isShuttingDown = true;

    // Stop health check timer
    if (this.healthCheckTimer) {
      clearInterval(this.healthCheckTimer);
      this.healthCheckTimer = undefined;
    }

    // Disconnect all connections
    for (const connection of this.connections) {
      if (connection.client) {
        try {
          await this.factory.disconnect(connection.client);
          this.logger.debug({ endpoint: connection.endpoint }, 'Connection disconnected');
        } catch (error) {
          this.logger.error(
            {
              endpoint: connection.endpoint,
              error: (error as Error).message,
            },
            'Error disconnecting connection'
          );
        }
      }
    }

    this.connections.length = 0;

    this.logger.info('Connection pool shutdown complete');
  }
}
