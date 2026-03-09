/**
 * Database Schema for Agent Wallet Metadata
 * Story 11.2: Agent Wallet Derivation and Address Generation
 *
 * This schema stores persistent wallet metadata for AI agents.
 * Private keys are NEVER stored - only public addresses and metadata.
 */

export const AGENT_WALLETS_TABLE_SCHEMA = `
CREATE TABLE IF NOT EXISTS agent_wallets (
  agent_id TEXT PRIMARY KEY,           -- Unique agent identifier
  derivation_index INTEGER UNIQUE NOT NULL,  -- BIP-44 index (prevents collisions)
  evm_address TEXT NOT NULL,           -- Ethereum/Base L2 address
  created_at INTEGER NOT NULL,         -- Unix timestamp
  metadata TEXT                        -- JSON-serialized optional metadata
);
`;

export const AGENT_WALLETS_INDEXES = [
  'CREATE INDEX IF NOT EXISTS idx_derivation_index ON agent_wallets(derivation_index);',
  'CREATE INDEX IF NOT EXISTS idx_evm_address ON agent_wallets(evm_address);',
];

/**
 * Database Schema for Agent Balance History
 * Story 11.3: Agent Wallet Balance Tracking and Monitoring
 *
 * This schema stores historical balance snapshots for agent wallets.
 * Balance stored as TEXT to preserve full precision (SQLite INTEGER is 64-bit, insufficient for uint256).
 */
export const AGENT_BALANCES_TABLE_SCHEMA = `
CREATE TABLE IF NOT EXISTS agent_balances (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  agent_id TEXT NOT NULL,              -- References agent_wallets.agent_id
  chain TEXT NOT NULL,                 -- 'evm'
  token TEXT NOT NULL,                 -- Token identifier
  balance TEXT NOT NULL,               -- Balance as string (bigint serialized)
  timestamp INTEGER NOT NULL           -- Unix timestamp
);
`;

export const AGENT_BALANCES_INDEXES = [
  'CREATE INDEX IF NOT EXISTS idx_agent_balances_lookup ON agent_balances(agent_id, chain, token);',
  'CREATE INDEX IF NOT EXISTS idx_agent_balances_timestamp ON agent_balances(timestamp);',
];

/**
 * Database Schema for Wallet Lifecycle Management
 * Story 11.5: Agent Wallet Lifecycle Management
 *
 * This schema stores lifecycle records for agent wallets.
 * Tracks state transitions, activity metrics, and suspension history.
 */
export const WALLET_LIFECYCLE_TABLE_SCHEMA = `
CREATE TABLE IF NOT EXISTS wallet_lifecycle (
  agent_id TEXT PRIMARY KEY,           -- References agent_wallets.agent_id
  state TEXT NOT NULL,                 -- Current lifecycle state
  created_at INTEGER NOT NULL,         -- Unix timestamp (wallet created)
  activated_at INTEGER,                -- Unix timestamp (wallet activated)
  suspended_at INTEGER,                -- Unix timestamp (wallet suspended)
  archived_at INTEGER,                 -- Unix timestamp (wallet archived)
  last_activity INTEGER,               -- Unix timestamp (last transaction)
  total_transactions INTEGER NOT NULL, -- Total transaction count
  total_volume TEXT,                   -- JSON: { token: volume_string }
  suspension_reason TEXT               -- Reason for suspension (if applicable)
);
`;

export const WALLET_LIFECYCLE_INDEXES = [
  'CREATE INDEX IF NOT EXISTS idx_lifecycle_state ON wallet_lifecycle(state);',
  'CREATE INDEX IF NOT EXISTS idx_lifecycle_last_activity ON wallet_lifecycle(last_activity);',
];

/**
 * Database Schema for Wallet Archives
 * Story 11.5: Agent Wallet Lifecycle Management
 *
 * This schema stores archived wallet data for audit trail.
 * Archives include final wallet state, balances, and lifecycle record.
 */
export const WALLET_ARCHIVES_TABLE_SCHEMA = `
CREATE TABLE IF NOT EXISTS wallet_archives (
  agent_id TEXT PRIMARY KEY,           -- Archived agent identifier
  wallet_data TEXT NOT NULL,           -- JSON-serialized AgentWallet
  balances TEXT NOT NULL,              -- JSON: { "chain:token": balance_string }
  lifecycle_data TEXT NOT NULL,        -- JSON-serialized WalletLifecycleRecord
  archived_at INTEGER NOT NULL         -- Unix timestamp (archived)
);
`;

export const WALLET_ARCHIVES_INDEXES = [
  'CREATE INDEX IF NOT EXISTS idx_archives_archived_at ON wallet_archives(archived_at);',
];

/**
 * Database Schema for Agent Payment Channels
 * Story 11.6: Payment Channel Integration for Agent Wallets
 *
 * This schema stores payment channel metadata for AI agents.
 * Tracks channels across EVM (Base L2).
 */
export const AGENT_CHANNELS_TABLE_SCHEMA = `
CREATE TABLE IF NOT EXISTS agent_channels (
  agent_id TEXT NOT NULL,              -- Agent identifier
  channel_id TEXT PRIMARY KEY,         -- On-chain channel ID (EVM: bytes32)
  chain TEXT NOT NULL,                 -- 'evm'
  peer_id TEXT NOT NULL,               -- Peer agent identifier
  token TEXT NOT NULL,                 -- Token symbol (EVM: USDC/DAI)
  opened_at INTEGER NOT NULL,          -- Unix timestamp (channel opened)
  last_activity_at INTEGER,            -- Unix timestamp (last payment)
  closed_at INTEGER                    -- Unix timestamp (channel closed, NULL if active)
);
`;

export const AGENT_CHANNELS_INDEXES = [
  'CREATE INDEX IF NOT EXISTS idx_channels_agent_id ON agent_channels(agent_id);',
  'CREATE INDEX IF NOT EXISTS idx_channels_chain ON agent_channels(chain);',
  'CREATE INDEX IF NOT EXISTS idx_channels_peer_id ON agent_channels(peer_id);',
  'CREATE INDEX IF NOT EXISTS idx_channels_closed_at ON agent_channels(closed_at);',
];
