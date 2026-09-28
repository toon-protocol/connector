/**
 * End-to-end: a mocked `GET /ilp/identity` (+ a mocked x402 greeting) response
 * from the Rust edge, run through the whole sidecar pipeline, produces the
 * exact expected signed kind:10032 event — the acceptance criterion
 * connector#681's re-scope asks for directly.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import pino from 'pino';
import { verifyEvent, getPublicKey } from 'nostr-tools';
import { loadConfig } from './config';
import { AnnouncerService } from './service';

const logger = pino({ level: 'silent' });
const SECRET_KEY_HEX = 'b'.repeat(64);

function jsonResponse(
  status: number,
  body: unknown,
  headers: Record<string, string> = {}
): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json', ...headers },
  });
}

function base64Header(json: unknown): string {
  return Buffer.from(JSON.stringify(json), 'utf8').toString('base64');
}

const GREETING = {
  x402Version: 2,
  resource: { url: 'g.toon.relay' },
  accepts: [
    {
      scheme: 'batch-settlement',
      network: 'eip155:84532',
      amount: '1000',
      asset: '0xToken',
      payTo: '0xSettlement',
      maxTimeoutSeconds: 60,
      extra: {
        receiverAuthorizer: '0xSettlement',
        withdrawDelay: 86400,
        name: 'USDC',
        version: '2',
      },
    },
  ],
  extensions: {
    toon: {
      info: { ilpAddress: 'g.toon.relay', amount: '1000', endpoint: '/ilp', price: '1000' },
      schema: { type: 'object' },
    },
  },
};

test('AnnouncerService.buildEvent: mocked /ilp/identity + greeting produce the exact expected signed event', async () => {
  const config = loadConfig({
    ANNOUNCER_IDENTITY_SECRET_KEY_HEX: SECRET_KEY_HEX,
    ANNOUNCER_RELAY_URLS: 'wss://relay.devnet.toonprotocol.dev',
    ANNOUNCER_ILP_ADDRESSES: 'g.toon,g.toon.relay,g.toon.store',
    ANNOUNCER_PROBE_ROUTES: 'g.toon.relay',
    ANNOUNCER_REFRESH_INTERVAL_SECS: '300',
  });

  const fetchImpl = (async (url: string) => {
    const u = String(url);
    if (u.endsWith('/ilp/identity')) {
      return jsonResponse(200, { keyId: 'edge-key-1', publicKey: '0x04deadbeef' });
    }
    if (u.endsWith('/ilp')) {
      return new Response(null, {
        status: 402,
        headers: { 'payment-required': base64Header(GREETING) },
      });
    }
    throw new Error(`unexpected fetch: ${u}`);
  }) as unknown as typeof fetch;

  const service = new AnnouncerService({ config, logger, fetchImpl });
  const fixedNow = 1_800_000_000_000;
  const originalDateNow = Date.now;
  Date.now = () => fixedNow;
  let event;
  try {
    event = await service.buildEvent();
  } finally {
    Date.now = originalDateNow;
  }

  // Signed by the sidecar's OWN dedicated announce identity (never the edge's ADR 0018 wrap key).
  assert.equal(event.pubkey, getPublicKey(config.secretKey));
  assert.equal(event.pubkey, service.announcePubkey);
  assert.equal(verifyEvent(event), true);

  assert.equal(event.kind, 10032);
  assert.equal(event.created_at, Math.floor(fixedNow / 1000));

  // Regular-replaceable (10000-19999): no d-tag, ever. TTL = 2x the refresh interval (600s).
  assert.deepEqual(event.tags, [['expiration', String(Math.floor(fixedNow / 1000) + 600)]]);

  const content: unknown = JSON.parse(event.content);
  assert.deepEqual(content, {
    ilpAddress: 'g.toon',
    ilpAddresses: ['g.toon', 'g.toon.relay', 'g.toon.store'],
    btpEndpoint: 'wss://proxy.devnet.toonprotocol.dev/rust/ilp/btp',
    httpEndpoint: 'https://proxy.devnet.toonprotocol.dev/rust/ilp',
    relayUrl: 'wss://relay.devnet.toonprotocol.dev',
    assetCode: 'USDC',
    assetScale: 6,
    supportedChains: ['evm:84532'],
    settlementAddresses: { 'evm:84532': '0xSettlement' },
    preferredTokens: { 'evm:84532': '0xToken' },
    routePrices: { 'g.toon.relay': '1000' },
    edgeIdentity: { keyId: 'edge-key-1', publicKey: '0x04deadbeef' },
    routes: { publish: 'g.toon.relay', store: 'g.toon.store' },
  });
});

test('AnnouncerService.buildEvent: a configured operator notice appears on the announce schema field', async () => {
  const config = loadConfig({
    ANNOUNCER_IDENTITY_SECRET_KEY_HEX: SECRET_KEY_HEX,
    ANNOUNCER_RELAY_URLS: 'wss://relay.devnet.toonprotocol.dev',
    ANNOUNCER_NOTICE_ID: 'maintenance-2026-08',
    ANNOUNCER_NOTICE_SUMMARY: 'Scheduled maintenance this weekend',
    ANNOUNCER_NOTICE_URL: 'https://example.com/notices/maintenance-2026-08',
  });
  const fetchImpl = (async () => new Response('boom', { status: 500 })) as unknown as typeof fetch;
  const service = new AnnouncerService({ config, logger, fetchImpl });

  const event = await service.buildEvent();
  const content: unknown = JSON.parse(event.content);
  assert.deepEqual((content as { notice?: unknown }).notice, {
    id: 'maintenance-2026-08',
    severity: 'info',
    summary: 'Scheduled maintenance this weekend',
    url: 'https://example.com/notices/maintenance-2026-08',
  });
});

test('AnnouncerService.buildEvent: still produces a valid (minimal) event when every edge poll fails', async () => {
  const config = loadConfig({
    ANNOUNCER_IDENTITY_SECRET_KEY_HEX: SECRET_KEY_HEX,
    ANNOUNCER_RELAY_URLS: 'wss://relay.devnet.toonprotocol.dev',
  });
  const fetchImpl = (async () => new Response('boom', { status: 500 })) as unknown as typeof fetch;
  const service = new AnnouncerService({ config, logger, fetchImpl });

  const event = await service.buildEvent();
  assert.equal(verifyEvent(event), true);
  const content: unknown = JSON.parse(event.content);
  assert.deepEqual(content, {
    ilpAddress: 'g.toon',
    btpEndpoint: 'wss://proxy.devnet.toonprotocol.dev/rust/ilp/btp',
    httpEndpoint: 'https://proxy.devnet.toonprotocol.dev/rust/ilp',
    relayUrl: 'wss://relay.devnet.toonprotocol.dev',
    assetCode: 'USDC',
    assetScale: 6,
    routes: { publish: 'g.toon', store: 'g.toon' },
  });
});
