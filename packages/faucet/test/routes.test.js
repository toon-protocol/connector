// Boots the REAL faucet server (child process, no mocks) and proves the
// USDC-only route set from toon-meta#310 §4.6 / connector#898:
//   - the local-anvil EVM leg (`/api/request`) and the native-SOL/native-MINA
//     legs are GONE from the service (404, not merely 503-when-unconfigured),
//     as is the whole Mina leg, USDC route included (ADR 0065)
//   - the surviving USDC-only routes are still mounted
//   - /api/info's capability map stops advertising the dropped legs
//
// Every chain secret is left UNSET so the surviving routes exercise their
// "not configured" (503) path — this is what proves 404 means "route
// removed" rather than "chain disabled", since an unconfigured-but-present
// route answers 503, never 404.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const INDEX = path.join(__dirname, '..', 'src', 'index.js');
const PORT = 20000 + Math.floor(Math.random() * 20000);
const BASE_URL = `http://127.0.0.1:${PORT}`;

function startFaucet() {
  let stderr = '';
  const child = spawn(process.execPath, [INDEX], {
    cwd: path.join(__dirname, '..'),
    env: {
      ...process.env,
      PORT: String(PORT),
      RPC_URL: 'http://127.0.0.1:1', // unreachable on purpose — EVM balances are best-effort
      TOKEN_ADDRESS: '',
      SOLANA_USDC_MINT: '',
      BASE_SEPOLIA_FAUCET_KEY: '',
    },
    stdio: ['ignore', 'ignore', 'pipe'],
  });
  child.stderr.on('data', (chunk) => {
    stderr += chunk.toString();
  });
  return { child, getStderr: () => stderr };
}

async function waitForHealth(deadlineMs, getStderr) {
  const start = Date.now();
  while (Date.now() - start < deadlineMs) {
    try {
      const res = await fetch(`${BASE_URL}/health`);
      if (res.ok) return;
    } catch {
      // Not listening yet — retry.
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`faucet did not become healthy within ${deadlineMs}ms. stderr:\n${getStderr()}`);
}

test('the dropped native-token routes 404; the USDC routes and /health survive', async (t) => {
  const { child, getStderr } = startFaucet();
  t.after(() => child.kill());

  await waitForHealth(15000, getStderr);

  const postJson = (route, address) =>
    fetch(`${BASE_URL}${route}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ address }),
    });

  // DROP: the local-anvil EVM leg and both native-token legs (toon-meta#310
  // §4.6), plus every Mina route including the USDC one (ADR 0065). Gone
  // entirely — 404, not just unconfigured. Body content is irrelevant here: a
  // removed route 404s before any handler runs.
  for (const route of [
    '/api/request',
    '/api/solana/request',
    '/api/mina/request',
    '/api/mina/usdc-request',
  ]) {
    const res = await postJson(route, 'x');
    assert.equal(res.status, 404, `${route} must be gone from the service, got ${res.status}`);
  }

  // KEEP: the USDC-only legs are still mounted — unconfigured means 503, not
  // 404. A well-formed-but-unconfigured address is used so the "not
  // configured" check is what answers, not address validation.
  const surviving = [
    ['/api/solana/usdc-request', '11111111111111111111111111111111'],
    ['/api/base-sepolia/request', 'x'],
  ];
  for (const [route, address] of surviving) {
    const res = await postJson(route, address);
    assert.notEqual(res.status, 404, `${route} must still exist, got 404`);
    assert.equal(res.status, 503, `${route} unconfigured should 503, got ${res.status}`);
  }

  // GET /health and GET /api/info are KEPT unchanged.
  const health = await fetch(`${BASE_URL}/health`);
  assert.equal(health.status, 200);

  const info = await (await fetch(`${BASE_URL}/api/info`)).json();
  assert.equal(info.chains.evm, undefined, '/api/info must not advertise the retired evm leg');
  assert.equal(info.chains.solana.route, '/api/solana/usdc-request');
  assert.equal(info.chains.solana.enabled, false);
  assert.equal(info.chains.mina, undefined, '/api/info must not advertise a Mina leg (ADR 0065)');
  assert.ok(info.chains.baseSepolia);

  // The local-anvil EVM leg's vestigial top-level fields must be gone
  // entirely — they described a leg this service no longer serves.
  for (const field of ['ethAmount', 'tokenAmount', 'tokenAddress', 'faucetBalances', 'ready']) {
    assert.equal(
      Object.prototype.hasOwnProperty.call(info, field),
      false,
      `/api/info must not carry the vestigial EVM field "${field}"`
    );
  }
});
