import { test } from 'node:test';
import assert from 'node:assert/strict';
import { buildAnnouncementInfo } from './announce-builder';
import type { AnnounceStaticConfig } from './announce-builder';
import type { ClientEdgeIdentity, RouteGreeting } from './edge-client';

const CONFIG: AnnounceStaticConfig = {
  ilpAddress: 'g.toon',
  ilpAddresses: ['g.toon', 'g.toon.relay', 'g.toon.store'],
  httpEndpoint: 'https://proxy.devnet.toonprotocol.dev/rust/ilp',
  btpEndpoint: 'wss://proxy.devnet.toonprotocol.dev/rust/ilp/btp',
  relayUrl: 'wss://relay.devnet.toonprotocol.dev',
  assetCode: 'USDC',
  assetScale: 6,
  routePublish: 'g.toon.relay',
  routeStore: 'g.toon.store',
  solanaChainId: 'solana:devnet',
};

const IDENTITY: ClientEdgeIdentity = { keyId: 'edge-key-1', publicKey: '0x04deadbeef' };

const EVM_GREETING: RouteGreeting = {
  destination: 'g.toon.relay',
  price: '1000',
  endpoint: '/ilp',
  batchSettlements: [
    {
      network: 'eip155:84532',
      asset: '0xToken',
      payTo: '0xSettlement',
      extra: {
        receiverAuthorizer: '0xSettlement',
        withdrawDelay: 86400,
        name: 'USDC',
        version: '2',
      },
    },
    {
      network: 'solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1',
      asset: 'MintAddress111',
      payTo: 'SolSettlement111',
      extra: { feePayer: 'SolSettlement111', withdrawDelay: 86400 },
    },
  ],
};

test('buildAnnouncementInfo: given a mocked identity + greeting, produces the exact expected IlpPeerInfo shape', () => {
  const info = buildAnnouncementInfo(CONFIG, IDENTITY, [EVM_GREETING]);

  assert.deepEqual(info, {
    ilpAddress: 'g.toon',
    ilpAddresses: ['g.toon', 'g.toon.relay', 'g.toon.store'],
    btpEndpoint: 'wss://proxy.devnet.toonprotocol.dev/rust/ilp/btp',
    httpEndpoint: 'https://proxy.devnet.toonprotocol.dev/rust/ilp',
    relayUrl: 'wss://relay.devnet.toonprotocol.dev',
    assetCode: 'USDC',
    assetScale: 6,
    supportedChains: ['evm:84532', 'solana:devnet'],
    settlementAddresses: {
      'evm:84532': '0xSettlement',
      'solana:devnet': 'SolSettlement111',
    },
    preferredTokens: {
      'evm:84532': '0xToken',
      'solana:devnet': 'MintAddress111',
    },
    routePrices: { 'g.toon.relay': '1000' },
    edgeIdentity: { keyId: 'edge-key-1', publicKey: '0x04deadbeef' },
    routes: { publish: 'g.toon.relay', store: 'g.toon.store' },
  });
});

test('buildAnnouncementInfo: announces a CAIP-2 solana network under the configured solanaChainId, and eip155 as evm', () => {
  const info = buildAnnouncementInfo(CONFIG, null, [EVM_GREETING]);
  assert.deepEqual(info.supportedChains, ['evm:84532', 'solana:devnet']);
  assert.equal('tokenNetworks' in info, false);
});

test('buildAnnouncementInfo: degrades gracefully when identity and every greeting failed to resolve', () => {
  const info = buildAnnouncementInfo(CONFIG, null, []);
  assert.deepEqual(info, {
    ilpAddress: 'g.toon',
    ilpAddresses: ['g.toon', 'g.toon.relay', 'g.toon.store'],
    btpEndpoint: 'wss://proxy.devnet.toonprotocol.dev/rust/ilp/btp',
    httpEndpoint: 'https://proxy.devnet.toonprotocol.dev/rust/ilp',
    relayUrl: 'wss://relay.devnet.toonprotocol.dev',
    assetCode: 'USDC',
    assetScale: 6,
    routes: { publish: 'g.toon.relay', store: 'g.toon.store' },
  });
});

test('buildAnnouncementInfo: sets notice on the schema field when configured', () => {
  const notice = {
    id: 'maintenance-2026-08',
    severity: 'info' as const,
    summary: 'Scheduled maintenance this weekend',
    url: 'https://example.com/notices/maintenance-2026-08',
  };
  const info = buildAnnouncementInfo({ ...CONFIG, notice }, null, []);
  assert.deepEqual(info.notice, notice);
});

test('buildAnnouncementInfo: omits notice entirely when not configured — no key, no default', () => {
  const info = buildAnnouncementInfo(CONFIG, null, []);
  assert.equal('notice' in info, false);
});

test('buildAnnouncementInfo: omits ilpAddresses/relayUrl when there is only one address / no relayUrl configured', () => {
  const info = buildAnnouncementInfo(
    { ...CONFIG, ilpAddresses: ['g.toon'], relayUrl: undefined },
    null,
    []
  );
  assert.equal('ilpAddresses' in info, false);
  assert.equal('relayUrl' in info, false);
});
