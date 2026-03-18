/**
 * BTP Client Manager
 * Manages multiple BTPClient instances for outbound peer connections
 */

import { Logger } from '../utils/logger';
import { BTPClient, Peer, BTPConnectionError } from './btp-client';
import { ILPPreparePacket, ILPFulfillPacket, ILPRejectPacket } from '@toon-protocol/shared';
import type { PacketHandler } from '../core/packet-handler';

/**
 * BTPClientManager - Orchestrates multiple BTP client connections
 * Maintains one BTPClient instance per peer and routes packets to appropriate clients
 */
export class BTPClientManager {
  private readonly _clients: Map<string, BTPClient> = new Map();
  private readonly _logger: Logger;
  private readonly _nodeId: string;
  private _packetHandler: PacketHandler | null = null;

  /**
   * Create BTPClientManager instance
   * @param nodeId - Local node identifier
   * @param logger - Pino logger instance
   */
  constructor(nodeId: string, logger: Logger) {
    this._nodeId = nodeId;
    this._logger = logger.child({ component: 'BTPClientManager' });
  }

  /**
   * Set PacketHandler reference (to handle incoming prepare packets from servers)
   * @param packetHandler - PacketHandler instance for routing incoming packets
   */
  setPacketHandler(packetHandler: PacketHandler): void {
    this._packetHandler = packetHandler;
    // Update existing clients
    for (const client of this._clients.values()) {
      client.setPacketHandler(packetHandler);
    }
  }

  /**
   * Add a peer and establish BTP connection
   * Creates BTPClient instance for the peer and initiates connection
   * @param peer - Peer configuration
   */
  async addPeer(peer: Peer): Promise<void> {
    this._logger.info(
      { event: 'btp_client_add_peer', peerId: peer.id, url: peer.url },
      'Adding peer'
    );

    // Check if peer already exists
    if (this._clients.has(peer.id)) {
      this._logger.warn(
        { event: 'btp_client_peer_exists', peerId: peer.id },
        'Peer already exists, skipping'
      );
      return;
    }

    // Create BTPClient for peer
    const client = new BTPClient(peer, this._nodeId, this._logger);

    // Set PacketHandler if available (for handling incoming prepare packets)
    if (this._packetHandler) {
      client.setPacketHandler(this._packetHandler);
    }

    // Set up event listeners for connection state tracking
    client.on('connected', () => {
      this._logger.info(
        { event: 'btp_client_connected', peerId: peer.id },
        'BTP client connected to peer'
      );
    });

    client.on('disconnected', () => {
      this._logger.warn(
        { event: 'btp_client_disconnected', peerId: peer.id },
        'BTP client disconnected from peer'
      );
    });

    client.on('error', (error: Error) => {
      this._logger.error(
        { event: 'btp_client_error', peerId: peer.id, error: error.message },
        'BTP client error'
      );
    });

    // Store client before connecting
    this._clients.set(peer.id, client);

    try {
      // Connect to peer
      await client.connect();
      this._logger.info(
        { event: 'btp_client_peer_added', peerId: peer.id },
        'Peer added and connected'
      );
    } catch (error) {
      // Don't remove client from map - BTPClient will retry in background
      // Client can still be used once retry succeeds
      const errorMessage = error instanceof Error ? error.message : String(error);
      this._logger.warn(
        { event: 'btp_client_add_peer_failed', peerId: peer.id, error: errorMessage },
        'Initial connection to peer failed (will retry in background)'
      );
      // Don't rethrow - allow connector to start even if initial connection fails
    }
  }

  /**
   * Remove a peer and disconnect BTP connection
   * Gracefully disconnects and removes BTPClient instance
   * @param peerId - Peer identifier
   */
  async removePeer(peerId: string): Promise<void> {
    this._logger.info({ event: 'btp_client_remove_peer', peerId }, 'Removing peer');

    const client = this._clients.get(peerId);
    if (!client) {
      this._logger.warn(
        { event: 'btp_client_peer_not_found', peerId },
        'Peer not found, cannot remove'
      );
      return;
    }

    try {
      // Disconnect from peer
      await client.disconnect();
      this._logger.info(
        { event: 'btp_client_peer_removed', peerId },
        'Peer disconnected and removed'
      );
    } finally {
      // Always remove from map, even if disconnect fails
      this._clients.delete(peerId);
    }
  }

  /**
   * Send ILP packet to specific peer
   * Routes packet to appropriate BTPClient based on peer ID
   * @param peerId - Target peer identifier
   * @param packet - ILP Prepare packet to send
   * @returns ILP response packet (Fulfill or Reject)
   * @throws Error if peer not found or connection fails
   */
  async sendToPeer(
    peerId: string,
    packet: ILPPreparePacket,
    protocolData?: Array<{ protocolName: string; contentType: number; data: Buffer }>
  ): Promise<ILPFulfillPacket | ILPRejectPacket> {
    this._logger.debug(
      { event: 'btp_client_send_to_peer', peerId, destination: packet.destination },
      'Sending packet to peer'
    );

    // Look up BTPClient for peer
    const client = this._clients.get(peerId);
    if (!client) {
      const errorMessage = `Peer not found: ${peerId}`;
      this._logger.error({ event: 'btp_client_peer_not_found', peerId }, errorMessage);
      throw new Error(errorMessage);
    }

    // Check connection state before sending
    if (!client.isConnected) {
      const errorMessage = `BTP connection to ${peerId} not established`;
      this._logger.error({ event: 'btp_client_not_connected', peerId }, errorMessage);
      throw new BTPConnectionError(errorMessage);
    }

    try {
      // Derive timeout from the ILP packet's expiresAt — the protocol-level timeout.
      // This ensures BTP waits as long as the packet is valid, regardless of hop count.
      // Fall back to env var only if expiresAt is missing (shouldn't happen for valid packets).
      let timeoutMs: number;
      if (packet.expiresAt) {
        const remaining = packet.expiresAt.getTime() - Date.now();
        // Use remaining time with a small buffer (500ms) for local processing
        timeoutMs = Math.max(remaining - 500, 1000);
      } else {
        timeoutMs = parseInt(process.env.BTP_SEND_TIMEOUT_MS ?? '30000', 10);
      }
      const timeoutPromise = new Promise<never>((_, reject) => {
        setTimeout(() => {
          reject(new BTPConnectionError(`BTP send timeout to ${peerId} (${timeoutMs}ms)`));
        }, timeoutMs);
      });

      // Race between sendPacket and timeout
      const response = await Promise.race([
        client.sendPacket(packet, protocolData),
        timeoutPromise,
      ]);

      this._logger.debug(
        { event: 'btp_client_packet_sent', peerId, destination: packet.destination },
        'Packet sent successfully to peer'
      );

      return response;
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      this._logger.error(
        { event: 'btp_client_send_failed', peerId, error: errorMessage },
        'Failed to send packet to peer'
      );
      throw error;
    }
  }

  /**
   * Get connection status for all peers
   * @returns Map of peer IDs to connection states
   */
  getPeerStatus(): Map<string, boolean> {
    const status = new Map<string, boolean>();
    for (const [peerId, client] of this._clients) {
      status.set(peerId, client.isConnected);
    }
    return status;
  }

  /**
   * Get list of all peer IDs
   * @returns Array of peer identifiers
   */
  getPeerIds(): string[] {
    return Array.from(this._clients.keys());
  }

  /**
   * Get BTPClient instance for a specific peer
   * @param peerId - Peer identifier
   * @returns BTPClient instance if peer exists, undefined otherwise
   * @remarks Used by settlement system to send off-chain claims via BTP protocolData
   */
  getClientForPeer(peerId: string): BTPClient | undefined {
    return this._clients.get(peerId);
  }

  /**
   * Check if a specific peer is currently connected
   * @param peerId - Peer identifier
   * @returns true if peer is connected, false otherwise
   * @remarks Returns false if peer doesn't exist or connection is not established
   */
  isConnected(peerId: string): boolean {
    const client = this._clients.get(peerId);
    return client ? client.isConnected : false;
  }

  /**
   * Get count of currently connected peers
   * @returns Number of peers with active BTP connections
   * @remarks Used by health check system to determine connector operational status
   */
  getConnectedPeerCount(): number {
    const peerStatus = this.getPeerStatus();
    return Array.from(peerStatus.values()).filter(Boolean).length;
  }

  /**
   * Get total number of configured peers
   * @returns Total count of peers regardless of connection state
   * @remarks Used by health check system to calculate connection percentage
   */
  getTotalPeerCount(): number {
    return this._clients.size;
  }

  /**
   * Get connection health percentage
   * @returns Percentage of connected peers (0-100)
   * @remarks Returns 100 if no peers are configured (standalone mode is considered healthy)
   */
  getConnectionHealth(): number {
    const totalCount = this.getTotalPeerCount();
    if (totalCount === 0) {
      return 100; // No peers configured is considered healthy
    }
    const connectedCount = this.getConnectedPeerCount();
    return (connectedCount / totalCount) * 100;
  }
}
