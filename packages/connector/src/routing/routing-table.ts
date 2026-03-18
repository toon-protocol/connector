/**
 * In-memory routing table for ILP connector
 * @packageDocumentation
 * @see {@link https://github.com/interledger/rfcs/blob/master/0027-interledger-protocol-4/0027-interledger-protocol-4.md|RFC-0027: Interledger Protocol v4}
 */

import { ILPAddress, RoutingTableEntry, isValidILPAddress } from '@toon-protocol/shared';

/**
 * In-memory routing table implementing longest-prefix matching per RFC-0027
 * @remarks
 * Maintains mappings from ILP address prefixes to next-hop peer identifiers.
 * Uses longest-prefix matching algorithm to determine packet forwarding destinations.
 * Thread-safe for concurrent reads (JavaScript single-threaded execution model).
 * Map operations are atomic at the JavaScript level, no explicit locking needed for MVP.
 *
 * @example
 * ```typescript
 * const routingTable = new RoutingTable([
 *   { prefix: 'g.alice', nextHop: 'peer-alice', priority: 10 },
 *   { prefix: 'g.bob', nextHop: 'peer-bob', priority: 5 }
 * ]);
 *
 * const nextHop = routingTable.getNextHop('g.alice.wallet.USD');
 * // Returns: 'peer-alice' (longest prefix match)
 * ```
 */
export class RoutingTable {
  /**
   * Internal storage for route entries
   * Key: ILP address prefix
   * Value: RoutingTableEntry
   */
  private readonly routes: Map<string, RoutingTableEntry>;

  /**
   * Optional logger instance for structured logging
   * @remarks
   * If provided, logs route additions/removals at INFO level.
   * Will be integrated with Pino logger in Story 1.6.
   */
  private readonly logger?: {
    info: (obj: object, msg?: string) => void;
    error: (obj: object, msg?: string) => void;
  };

  /**
   * Creates a new RoutingTable instance
   * @param initialRoutes - Optional array of routes to initialize the table
   * @param logger - Optional logger instance for structured logging
   * @throws {Error} If any initial route has invalid ILP address prefix
   */
  constructor(
    initialRoutes?: RoutingTableEntry[],
    logger?: {
      info: (obj: object, msg?: string) => void;
      error: (obj: object, msg?: string) => void;
    }
  ) {
    this.routes = new Map();
    this.logger = logger;

    if (initialRoutes && initialRoutes.length > 0) {
      for (const route of initialRoutes) {
        this.addRoute(route.prefix, route.nextHop, route.priority);
      }
      this.logger?.info(
        { routeCount: initialRoutes.length },
        `Initialized routing table with ${initialRoutes.length} routes`
      );
    }
  }

  /**
   * Add a routing entry to the table
   * @param prefix - ILP address prefix (e.g., "g.alice" or "g.bob.crypto")
   * @param nextHop - Peer identifier matching BTP connection
   * @param priority - Optional route priority for tie-breaking (default: 0, higher wins)
   * @throws {Error} If prefix is not a valid ILP address per RFC-0015
   * @remarks
   * Per RFC-0027, routing tables maintain mappings from address prefixes to next-hop peers.
   * Priority field enables tie-breaking when multiple routes have equal prefix lengths.
   */
  addRoute(prefix: ILPAddress, nextHop: string, priority: number = 0): void {
    if (!isValidILPAddress(prefix)) {
      const error = new Error(`Invalid ILP address prefix: ${prefix}`);
      this.logger?.error({ prefix, nextHop, priority }, error.message);
      throw error;
    }

    const entry: RoutingTableEntry = { prefix, nextHop, priority };
    this.routes.set(prefix, entry);

    this.logger?.info({ prefix, nextHop, priority }, `Added route: ${prefix} -> ${nextHop}`);
  }

  /**
   * Remove a routing entry from the table
   * @param prefix - ILP address prefix to remove
   * @remarks
   * Silently succeeds if prefix does not exist (idempotent operation).
   * Logs removal at INFO level if route existed.
   */
  removeRoute(prefix: string): void {
    const existed = this.routes.has(prefix);
    this.routes.delete(prefix);

    if (existed) {
      this.logger?.info({ prefix }, `Removed route: ${prefix}`);
    }
  }

  /**
   * Find next-hop peer for destination using longest-prefix matching
   * @param destination - Full ILP address of packet destination
   * @returns Next-hop peer identifier, or null if no route matches
   * @remarks
   * Per RFC-0027, implements longest-prefix matching algorithm:
   * 1. Find all route prefixes that match the destination
   * 2. Select the route with the longest matching prefix (most specific)
   * 3. If multiple routes have same prefix length, use priority field (higher wins)
   * 4. Return null if no route matches (caller generates F02 Unreachable error)
   *
   * Time complexity: O(n) where n is number of routes (acceptable for MVP).
   * Future optimization: Trie data structure for O(log n) lookup.
   *
   * @example
   * ```typescript
   * // Routes: ['g', 'g.alice', 'g.alice.wallet']
   * getNextHop('g.alice.wallet.USD') // Returns nextHop for 'g.alice.wallet' (longest match)
   * getNextHop('g.bob.crypto')       // Returns nextHop for 'g' (only match)
   * getNextHop('test.invalid')       // Returns null (no match)
   * ```
   */
  getNextHop(destination: ILPAddress): string | null {
    let bestMatch: RoutingTableEntry | null = null;
    let longestPrefixLength = -1;

    for (const route of this.routes.values()) {
      // Check if destination starts with this route's prefix
      if (destination === route.prefix || destination.startsWith(route.prefix + '.')) {
        const prefixLength = route.prefix.length;

        // Update best match if this prefix is longer, or same length with higher priority
        if (
          prefixLength > longestPrefixLength ||
          (prefixLength === longestPrefixLength &&
            (route.priority ?? 0) > (bestMatch?.priority ?? 0))
        ) {
          bestMatch = route;
          longestPrefixLength = prefixLength;
        }
      }
    }

    return bestMatch?.nextHop ?? null;
  }

  /**
   * Export all current routes for inspection/debugging
   * @returns Array of all routing table entries (deep copy)
   * @remarks
   * Returns a deep copy to prevent external mutation of internal state.
   * Useful for telemetry export to dashboard and debugging.
   */
  getAllRoutes(): RoutingTableEntry[] {
    return Array.from(this.routes.values()).map((route) => ({
      prefix: route.prefix,
      nextHop: route.nextHop,
      priority: route.priority,
    }));
  }

  /**
   * Get the number of routes in the table
   * @returns Total number of routing entries
   */
  get size(): number {
    return this.routes.size;
  }
}
