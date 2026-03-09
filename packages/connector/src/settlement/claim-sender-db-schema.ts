/**
 * Database Schema for Sent Claims Storage
 * Story 17.2: Claim Sender Implementation
 *
 * This schema stores sent payment channel claims for dispute resolution.
 * Claims are persisted after successful BTP transmission for audit trail.
 */

/**
 * Table: sent_claims
 *
 * Stores all claims sent to peers via BTP for dispute resolution.
 * Claims include EVM blockchain-specific signatures.
 *
 * Key Design Decisions:
 * - message_id as PRIMARY KEY ensures idempotency (no duplicate sends)
 * - claim_data stores full JSON claim for complete audit trail
 * - ack_received_at reserved for future Story 17.3 (Claim Receiver)
 */
export const SENT_CLAIMS_TABLE_SCHEMA = `
CREATE TABLE IF NOT EXISTS sent_claims (
  message_id TEXT PRIMARY KEY,     -- Unique message ID (blockchain-channelId-nonce-timestamp)
  peer_id TEXT NOT NULL,           -- Peer identifier (who received the claim)
  blockchain TEXT NOT NULL,        -- Blockchain type: 'evm'
  claim_data TEXT NOT NULL,        -- JSON-encoded BTPClaimMessage
  sent_at INTEGER NOT NULL,        -- Unix timestamp ms (when claim was sent)
  ack_received_at INTEGER          -- Unix timestamp ms (when ack received, NULL until Story 17.3)
);
`;

/**
 * Indexes for sent_claims table
 *
 * Optimized for common query patterns:
 * - idx_sent_claims_peer: Lookup all claims sent to a specific peer
 * - idx_sent_claims_sent_at: Time-based queries for cleanup or auditing
 */
export const SENT_CLAIMS_INDEXES = [
  'CREATE INDEX IF NOT EXISTS idx_sent_claims_peer ON sent_claims(peer_id);',
  'CREATE INDEX IF NOT EXISTS idx_sent_claims_sent_at ON sent_claims(sent_at);',
];
